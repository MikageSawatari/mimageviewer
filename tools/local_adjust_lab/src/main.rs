use std::collections::{BTreeSet, VecDeque};
use std::fs::OpenOptions;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock, mpsc};
use std::time::{Duration, Instant};

use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64};
use eframe::egui::{
    self, Color32, ColorImage, ComboBox, Key, Pos2, Rect, Sense, TextureHandle, TextureOptions,
};
use flate2::Compression;
use flate2::read::DeflateDecoder;
use flate2::write::DeflateEncoder;
use image::{RgbImage, RgbaImage, imageops::FilterType};
use local_adjust_core::{
    BloomParams, BlurParams, ChromaticAberrationParams, ClarityParams, ColorRangeMask,
    DehazeParams, EdgeSmoothParams, FilmGrainParams, HalftoneParams, HighlightsShadowsParams,
    HslParams, LineKind, LinearGradientMask, LocalAdjustmentLayer, LocalEffect, LocalMask,
    LookParams, LookPreset, ManualMaskOverride, MaskShape, MosaicParams, RadialGradientMask,
    RangeMask, RasterMask, RasterVectorMask, RegionMask, RgbaImageBuf, RgbaImageRef, ShapeOp,
    SharpenParams, SoftFocusParams, StarGlowParams, ToneCurveParams, ToneParams, VignetteParams,
    apply_layers, evaluate_layer_mask,
};
use serde::{Deserialize, Serialize};

const PANEL_W: f32 = 340.0;
const TOOL_PANEL_W: f32 = 300.0;
const PANEL_MARGIN_X: f32 = 16.0;
const PANEL_MARGIN_Y: f32 = 60.0;
const PANEL_BOTTOM_MARGIN: f32 = 20.0;
const PANEL_MIN_BODY_H: f32 = 160.0;
const ZOOM_MIN: f32 = 0.05;
const ZOOM_MAX: f32 = 12.0;
const NUDGE_PIXELS: f32 = 1.0;
const NUDGE_PIXELS_FAST: f32 = 10.0;
const ROTATE_DEG_STEP: f32 = 0.1;
const ROTATE_DEG_STEP_FAST: f32 = 1.0;
const FREEHAND_MIN_DISTANCE_SQ: f32 = 4.0;
const POLYGON_CLOSE_RADIUS_PX: f32 = 12.0;
const POLYGON_VERTEX_MIN_DISTANCE_PX: f32 = 3.0;
const EDGE_OVERLAY_REPAINT_MS: u64 = 90;
const EDGE_BRUSH_INCLUDE_BOUNDARY_RADIUS: usize = 2;
const BRUSH_STROKE_SPACING_RATIO: f32 = 0.35;
const BRUSH_STROKE_MIN_SPACING: f32 = 1.0;
const BRUSH_STROKE_MAX_SPACING: f32 = 12.0;
const BRUSH_STROKE_MAX_STAMPS_PER_FRAME: usize = 96;
const RESULT_RENDER_DRAG_RECHECK_MS: u64 = 90;
const MASK_PREVIEW_DRAG_INTERVAL_MS: u64 = 16;
const MASK_PREVIEW_TILE_SIZE: usize = 256;
const REGION_BOUNDARY_ANIM_INTERVAL_MS: u64 = 160;
const U2NETP_INPUT_SIZE: usize = 320;
const MAX_UNDO_SNAPSHOTS_NORMAL: usize = 24;
const MAX_UNDO_SNAPSHOTS_LARGE: usize = 8;
const LARGE_UNDO_PIXEL_COUNT: usize = 2_500_000;
const REGION_SEGMENT_MAX_LABELS: usize = 2048;

fn main() -> eframe::Result<()> {
    let initial_path = std::env::args_os().nth(1).map(PathBuf::from);
    let mut options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("mIV Local Adjust Lab")
            .with_inner_size([1360.0, 880.0])
            .with_drag_and_drop(true),
        vsync: false,
        ..Default::default()
    };
    options.wgpu_options.present_mode = eframe::wgpu::PresentMode::AutoNoVsync;
    options.wgpu_options.desired_maximum_frame_latency = Some(1);
    eframe::run_native(
        "mIV Local Adjust Lab",
        options,
        Box::new(move |cc| Ok(Box::new(LocalAdjustLabApp::new(cc, initial_path)))),
    )
}

fn configure_lab_fonts(ctx: &egui::Context) {
    let mut fonts = egui::FontDefinitions::default();
    for path in [
        r"C:\Windows\Fonts\YuGothM.ttc",
        r"C:\Windows\Fonts\meiryo.ttc",
        r"C:\Windows\Fonts\msgothic.ttc",
    ] {
        let Ok(data) = std::fs::read(path) else {
            continue;
        };
        fonts.font_data.insert(
            "miv_lab_japanese".to_owned(),
            Arc::new(egui::FontData::from_owned(data)),
        );
        for family in [egui::FontFamily::Proportional, egui::FontFamily::Monospace] {
            let family_fonts = fonts.families.entry(family).or_default();
            family_fonts.retain(|name| name != "miv_lab_japanese");
            family_fonts.insert(0, "miv_lab_japanese".to_owned());
        }
        break;
    }
    ctx.set_fonts(fonts);
    apply_lab_dark_theme(ctx);
}

fn lab_dark_visuals() -> egui::Visuals {
    let mut visuals = egui::Visuals::dark();
    visuals.override_text_color = Some(Color32::WHITE);
    visuals.window_fill = Color32::from_rgb(20, 20, 22);
    visuals.panel_fill = Color32::from_rgb(18, 18, 20);
    visuals.extreme_bg_color = Color32::from_rgb(8, 8, 10);
    visuals.faint_bg_color = Color32::from_rgb(32, 32, 34);
    visuals.widgets.inactive.bg_fill = Color32::from_rgb(56, 56, 56);
    visuals.widgets.hovered.bg_fill = Color32::from_rgb(74, 74, 74);
    visuals.widgets.active.bg_fill = Color32::from_rgb(84, 84, 84);
    visuals.selection.bg_fill = Color32::from_rgb(45, 96, 140);
    visuals.selection.stroke.color = Color32::WHITE;
    visuals
}

fn apply_lab_dark_theme(ctx: &egui::Context) {
    // egui 0.33 can resolve popups against the OS light style unless the theme
    // preference itself is pinned. Keep the lab dark even inside ComboBox popups.
    ctx.set_theme(egui::ThemePreference::Dark);
    ctx.style_mut(|style| {
        style.visuals = lab_dark_visuals();
    });
}

fn apply_lab_dark_ui(ui: &mut egui::Ui) {
    ui.style_mut().visuals = lab_dark_visuals();
}

fn lab_combo_box<R>(
    ui: &mut egui::Ui,
    id_salt: impl std::hash::Hash,
    selected_text: impl Into<egui::WidgetText>,
    add_contents: impl FnOnce(&mut egui::Ui) -> R,
) -> egui::InnerResponse<Option<R>> {
    ComboBox::from_id_salt(id_salt)
        .selected_text(selected_text)
        .height(420.0)
        .show_ui(ui, |ui| {
            apply_lab_dark_ui(ui);
            add_contents(ui)
        })
}

fn animated_overlay_color(ctx: &egui::Context, alpha: u8) -> Color32 {
    let t = ctx.input(|i| i.time);
    let phase = ((t * 3.0).sin() * 0.5 + 0.5) as f32;
    let r = 255_u8;
    let g = (72.0 + 168.0 * phase).round() as u8;
    let b = (220.0 - 156.0 * phase).round() as u8;
    Color32::from_rgba_unmultiplied(r, g, b, alpha)
}

struct LoadedImage {
    path: PathBuf,
    source: RgbaImageBuf,
}

struct RenderPending {
    generation: u64,
    rx: mpsc::Receiver<Result<RgbaImageBuf, String>>,
    started_at: Instant,
}

struct SegmentationPending {
    layer_idx: usize,
    generation: u64,
    rx: mpsc::Receiver<Result<GeneratedMask, String>>,
    started_at: Instant,
}

enum GeneratedMask {
    Subject(RasterMask),
    Regions(RegionMask),
}

const LAB_SIDECAR_VERSION: u32 = 1;

#[derive(Debug, Serialize, Deserialize)]
struct LabSidecar {
    version: u32,
    image_file: String,
    image_width: usize,
    image_height: usize,
    #[serde(default)]
    crop_enabled: bool,
    #[serde(default)]
    crop_overlay: bool,
    #[serde(default)]
    crop_rect: Option<CropRect>,
    layers: Vec<StoredLayer>,
}

#[derive(Debug, Serialize, Deserialize)]
struct StoredLayer {
    name: String,
    enabled: bool,
    opacity: f32,
    mask: StoredMask,
    #[serde(default)]
    manual_override: StoredManualOverride,
    mask_inverted: bool,
    mask_expand_px: f32,
    mask_feather_px: f32,
    effect: LocalEffect,
}

#[derive(Debug, Serialize, Deserialize)]
enum StoredMask {
    Full,
    Raster(StoredRasterVectorMask),
    LinearGradient(LinearGradientMask),
    RadialGradient(RadialGradientMask),
    LumaRange(RangeMask),
    ColorRange(ColorRangeMask),
    Subject(StoredSoftMask),
    Segmentation(StoredRegionMask),
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct StoredManualOverride {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    add: Option<StoredRasterVectorMask>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    subtract: Option<StoredRasterVectorMask>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct StoredRasterVectorMask {
    width: usize,
    height: usize,
    bitmap_1bit_deflate_b64: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    shapes: Vec<MaskShape>,
}

#[derive(Debug, Serialize, Deserialize)]
struct StoredSoftMask {
    width: usize,
    height: usize,
    alpha_u8_deflate_b64: String,
}

#[derive(Debug, Serialize, Deserialize)]
struct StoredRegionMask {
    width: usize,
    height: usize,
    labels_u32le_deflate_b64: String,
    selected: Vec<bool>,
}

#[derive(Debug, Clone, Copy)]
struct MaskDirtyRect {
    min_x: usize,
    min_y: usize,
    max_x: usize,
    max_y: usize,
}

struct MaskTilePreview {
    width: usize,
    height: usize,
    tile_size: usize,
    cols: usize,
    rows: usize,
    tiles: Vec<Option<TextureHandle>>,
}

impl MaskTilePreview {
    fn new(width: usize, height: usize, tile_size: usize) -> Self {
        let cols = width.div_ceil(tile_size).max(1);
        let rows = height.div_ceil(tile_size).max(1);
        Self {
            width,
            height,
            tile_size,
            cols,
            rows,
            tiles: vec![None; cols * rows],
        }
    }

    fn matches_size(&self, width: usize, height: usize) -> bool {
        self.width == width && self.height == height && self.tile_size == MASK_PREVIEW_TILE_SIZE
    }
}

#[derive(Debug, Default, Clone)]
struct PerfStats {
    ui_frames: u64,
    frame_gap_samples: u64,
    frame_gap_ms_total: f64,
    frame_gap_ms_max: f64,
    app_update_ms_total: f64,
    app_update_ms_max: f64,
    eframe_cpu_samples: u64,
    eframe_cpu_ms_total: f64,
    eframe_cpu_ms_max: f64,
    brush_frames: u64,
    brush_input_points: u64,
    brush_stamps: u64,
    brush_changed_stamps: u64,
    brush_ms_total: f64,
    brush_ms_max: f64,
    brush_input_gap_px_max: f32,
    mask_updates: u64,
    mask_eval_ms_total: f64,
    mask_texture_ms_total: f64,
    mask_total_ms_total: f64,
    mask_total_ms_max: f64,
    mask_tiles_updated: u64,
    render_jobs: u64,
    render_ms_total: f64,
    render_ms_max: f64,
}

impl PerfStats {
    fn has_activity(&self) -> bool {
        self.brush_frames > 0 || self.mask_updates > 0 || self.render_jobs > 0
    }
}

struct EdgePreviewCache {
    key: EdgePreviewKey,
    texture: TextureHandle,
}

struct EdgeMaskCache {
    key: EdgeMaskKey,
    mask: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq)]
struct EdgeMaskKey {
    image_path: PathBuf,
    source_size: [usize; 2],
    threshold: u8,
    ink_threshold: u8,
    gap_px: u8,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
struct CropRect {
    min_x: f32,
    min_y: f32,
    max_x: f32,
    max_y: f32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CropHandle {
    Body,
    North,
    South,
    West,
    East,
    NorthWest,
    NorthEast,
    SouthWest,
    SouthEast,
}

#[derive(Debug, Clone, Copy)]
struct CropDrag {
    handle: CropHandle,
    base: CropRect,
}

#[derive(Debug, Clone, PartialEq)]
struct EdgePreviewKey {
    image_path: PathBuf,
    source_size: [usize; 2],
    preview_size: [usize; 2],
    threshold: u8,
    ink_threshold: u8,
    gap_px: u8,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum MaskTool {
    Select,
    Brush,
    EdgeBrush,
    GapFillBrush,
    Lasso,
    Polygon,
    Line,
    VertLine,
    HorizLine,
    Rect,
    Ellipse,
}

impl MaskTool {
    fn label(self) -> &'static str {
        match self {
            Self::Select => "選択 [S]",
            Self::Brush => "筆 [B]",
            Self::EdgeBrush => "境界筆 [A]",
            Self::GapFillBrush => "隙間補完 [G]",
            Self::Lasso => "囲み [L]",
            Self::Polygon => "多角形 [P]",
            Self::Line => "直線 [I]",
            Self::VertLine => "縦線 [V]",
            Self::HorizLine => "横線 [H]",
            Self::Rect => "矩形 [R]",
            Self::Ellipse => "楕円 [O]",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ShapeHandle {
    Body,
    LineStart,
    LineEnd,
    Corner(u8),
    Radius,
}

#[derive(Debug, Clone, Copy)]
struct ShapeDrag {
    shape_idx: usize,
    handle: ShapeHandle,
    base: MaskShape,
    origin: [f32; 2],
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BitmapMaskOp {
    Expand,
    Shrink,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum OverrideEditTarget {
    Add,
    Subtract,
}

impl OverrideEditTarget {
    fn label(self) -> &'static str {
        match self {
            Self::Add => "追加補正",
            Self::Subtract => "削除補正",
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
    fn from_mask(mask: &LocalMask) -> Self {
        match mask {
            LocalMask::Full => Self::Full,
            LocalMask::Raster(_) | LocalMask::RasterVector(_) => Self::Raster,
            LocalMask::LinearGradient(_) => Self::LinearGradient,
            LocalMask::RadialGradient(_) => Self::RadialGradient,
            LocalMask::LumaRange(_) => Self::LumaRange,
            LocalMask::ColorRange(_) => Self::ColorRange,
            LocalMask::Subject(_) => Self::Subject,
            LocalMask::Segmentation(_) => Self::Segmentation,
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::Full => "全体",
            Self::Raster => "手動マスク",
            Self::LinearGradient => "線形グラデーション",
            Self::RadialGradient => "円形グラデーション",
            Self::LumaRange => "輝度範囲",
            Self::ColorRange => "カラー範囲",
            Self::Subject => "被写体選択",
            Self::Segmentation => "領域分割",
        }
    }

    fn description(self) -> &'static str {
        match self {
            Self::Full => "画像全体に同じ効果をかけます。",
            Self::Raster => "ブラシ、境界筆、多角形、図形で手動作成します。",
            Self::LinearGradient => "ドラッグした線に沿って段階的に効果をかけます。",
            Self::RadialGradient => "円形の内側から外側へ段階的に効果をかけます。",
            Self::LumaRange => "明るさの範囲でマスクを作ります。",
            Self::ColorRange => "スポイトで拾った色に近い範囲をマスクにします。",
            Self::Subject => "AI等で被写体と背景を分ける1枚のマットを作ります。",
            Self::Segmentation => "画像を細かい領域候補に分け、クリックで個別にON/OFFします。",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum EffectKind {
    None,
    Tone,
    ToneCurve,
    Clarity,
    HighlightsShadows,
    Dehaze,
    Blur,
    SoftFocus,
    Mosaic,
    Sharpen,
    Hsl,
    Look,
    Bloom,
    Vignette,
    FilmGrain,
    ChromaticAberration,
    Halftone,
    StarGlow,
    EdgeSmooth,
}

impl EffectKind {
    fn from_effect(effect: &LocalEffect) -> Self {
        match effect {
            LocalEffect::None => Self::None,
            LocalEffect::Tone(_) => Self::Tone,
            LocalEffect::ToneCurve(_) => Self::ToneCurve,
            LocalEffect::Clarity(_) => Self::Clarity,
            LocalEffect::HighlightsShadows(_) => Self::HighlightsShadows,
            LocalEffect::Dehaze(_) => Self::Dehaze,
            LocalEffect::Blur(_) => Self::Blur,
            LocalEffect::SoftFocus(_) => Self::SoftFocus,
            LocalEffect::Mosaic(_) => Self::Mosaic,
            LocalEffect::Sharpen(_) => Self::Sharpen,
            LocalEffect::Hsl(_) => Self::Hsl,
            LocalEffect::Look(_) => Self::Look,
            LocalEffect::Bloom(_) => Self::Bloom,
            LocalEffect::Vignette(_) => Self::Vignette,
            LocalEffect::FilmGrain(_) => Self::FilmGrain,
            LocalEffect::ChromaticAberration(_) => Self::ChromaticAberration,
            LocalEffect::Halftone(_) => Self::Halftone,
            LocalEffect::StarGlow(_) => Self::StarGlow,
            LocalEffect::EdgeSmooth(_) => Self::EdgeSmooth,
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::None => "効果なし",
            Self::Tone => "色調補正",
            Self::ToneCurve => "トーンカーブ",
            Self::Clarity => "明瞭度",
            Self::HighlightsShadows => "ハイライト/シャドウ",
            Self::Dehaze => "かすみ除去",
            Self::Blur => "ぼかし",
            Self::SoftFocus => "ソフトフォーカス",
            Self::Mosaic => "モザイク",
            Self::Sharpen => "シャープ",
            Self::Hsl => "色相/HSL",
            Self::Look => "ルック",
            Self::Bloom => "ブルーム",
            Self::Vignette => "ビネット",
            Self::FilmGrain => "フィルム粒子",
            Self::ChromaticAberration => "色収差",
            Self::Halftone => "ハーフトーン",
            Self::StarGlow => "クロス光",
            Self::EdgeSmooth => "エッジ保持ぼかし",
        }
    }
}

struct LocalAdjustLabApp {
    image: Option<LoadedImage>,
    source_texture: Option<TextureHandle>,
    result_texture: Option<TextureHandle>,
    mask_tiles: Option<MaskTilePreview>,
    mask_dirty_tiles: Option<BTreeSet<(usize, usize)>>,
    edge_mask: Option<EdgeMaskCache>,
    edge_preview: Option<EdgePreviewCache>,
    layers: Vec<LocalAdjustmentLayer>,
    selected_layer: usize,
    result: Option<RgbaImageBuf>,
    pending: Option<RenderPending>,
    segmentation_pending: Option<SegmentationPending>,
    generation: u64,
    mask_copy_source: usize,
    result_dirty: bool,
    mask_dirty: bool,
    last_edit: Instant,
    last_mask_preview_update: Instant,
    tool: MaskTool,
    paint_mode: bool,
    override_edit_panel: Option<OverrideEditTarget>,
    brush_radius: f32,
    gap_fill_distance: f32,
    edge_brush_include_boundary: bool,
    edge_brush_tolerance: f32,
    edge_brush_seed: Option<[u8; 3]>,
    prev_paint_pos: Option<Pos2>,
    last_paint_pos: Option<Pos2>,
    radial_gradient_drag_active: bool,
    boundary_edge_threshold: f32,
    boundary_ink_threshold: f32,
    boundary_gap_px: f32,
    edge_snap_radius: f32,
    region_color_tolerance: f32,
    region_min_area: usize,
    line_width: f32,
    lasso_points: Vec<[f32; 2]>,
    shape_drag_start: Option<[f32; 2]>,
    shape_drag_end: Option<[f32; 2]>,
    selected_shape: Option<usize>,
    shape_drag: Option<ShapeDrag>,
    undo_stack: Vec<Vec<LocalAdjustmentLayer>>,
    redo_stack: Vec<Vec<LocalAdjustmentLayer>>,
    show_source: bool,
    show_mask: bool,
    preview_to_selected_layer: bool,
    crop_enabled: bool,
    crop_overlay: bool,
    crop_edit_mode: bool,
    crop_rect: Option<CropRect>,
    crop_drag: Option<CropDrag>,
    add_layer_dialog_open: bool,
    add_layer_mask_kind: MaskKind,
    status: String,
    view_zoom: f32,
    view_pan: egui::Vec2,
    pan_drag_start: Option<(Pos2, egui::Vec2)>,
    perf_stats: PerfStats,
    perf_started_at: Instant,
    perf_last_log: Instant,
    perf_last_update_start: Option<Instant>,
    perf_log_path: PathBuf,
    panel_last_rect: Option<Rect>,
    tool_panel_last_rect: Option<Rect>,
}

impl LocalAdjustLabApp {
    fn new(cc: &eframe::CreationContext<'_>, initial_path: Option<PathBuf>) -> Self {
        configure_lab_fonts(&cc.egui_ctx);
        let now = Instant::now();
        let mut app = Self {
            image: None,
            source_texture: None,
            result_texture: None,
            mask_tiles: None,
            mask_dirty_tiles: None,
            edge_mask: None,
            edge_preview: None,
            layers: Vec::new(),
            selected_layer: 0,
            result: None,
            pending: None,
            segmentation_pending: None,
            generation: 0,
            mask_copy_source: 0,
            result_dirty: false,
            mask_dirty: false,
            last_edit: Instant::now(),
            last_mask_preview_update: Instant::now(),
            tool: MaskTool::Brush,
            paint_mode: true,
            override_edit_panel: None,
            brush_radius: 36.0,
            gap_fill_distance: 10.0,
            edge_brush_include_boundary: false,
            edge_brush_tolerance: 42.0,
            edge_brush_seed: None,
            prev_paint_pos: None,
            last_paint_pos: None,
            radial_gradient_drag_active: false,
            boundary_edge_threshold: 24.0,
            boundary_ink_threshold: 28.0,
            boundary_gap_px: 2.0,
            edge_snap_radius: 16.0,
            region_color_tolerance: 42.0,
            region_min_area: 64,
            line_width: 28.0,
            lasso_points: Vec::new(),
            shape_drag_start: None,
            shape_drag_end: None,
            selected_shape: None,
            shape_drag: None,
            undo_stack: Vec::new(),
            redo_stack: Vec::new(),
            show_source: false,
            show_mask: true,
            preview_to_selected_layer: false,
            crop_enabled: false,
            crop_overlay: true,
            crop_edit_mode: false,
            crop_rect: None,
            crop_drag: None,
            add_layer_dialog_open: false,
            add_layer_mask_kind: MaskKind::Raster,
            status: "JPEG / PNG をドロップしてください。".to_string(),
            view_zoom: 1.0,
            view_pan: egui::Vec2::ZERO,
            pan_drag_start: None,
            perf_stats: PerfStats::default(),
            perf_started_at: now,
            perf_last_log: now,
            perf_last_update_start: None,
            perf_log_path: perf_log_path(),
            panel_last_rect: None,
            tool_panel_last_rect: None,
        };
        if let Some(path) = initial_path {
            app.load_path(&cc.egui_ctx, &path);
        }
        app
    }

    fn load_path(&mut self, ctx: &egui::Context, path: &Path) {
        match load_image(path) {
            Ok(loaded) => {
                let color_image = color_image_from_rgba(&loaded.source);
                self.source_texture = Some(ctx.load_texture(
                    "local_adjust_source",
                    color_image,
                    TextureOptions::LINEAR,
                ));
                self.result_texture = None;
                self.mask_tiles = None;
                self.mask_dirty_tiles = None;
                self.edge_mask = None;
                self.edge_preview = None;
                self.layers = Vec::new();
                self.selected_layer = 0;
                self.result = None;
                self.pending = None;
                self.segmentation_pending = None;
                self.mask_copy_source = 0;
                self.image = Some(loaded);
                self.undo_stack.clear();
                self.redo_stack.clear();
                self.view_zoom = 1.0;
                self.view_pan = egui::Vec2::ZERO;
                self.pan_drag_start = None;
                self.crop_enabled = false;
                self.crop_overlay = true;
                self.crop_edit_mode = false;
                self.crop_rect = None;
                self.crop_drag = None;
                self.prev_paint_pos = None;
                self.last_paint_pos = None;
                self.override_edit_panel = None;
                self.radial_gradient_drag_active = false;
                self.edge_brush_seed = None;
                let load_status = format!("読み込み: {}", path.display());
                self.status = load_status.clone();
                let sidecar_path = sidecar_path_for_image(path);
                match self.load_settings_sidecar_from_path(&sidecar_path) {
                    Ok(true) => {}
                    Ok(false) => self.status = load_status,
                    Err(e) => self.status = format!("{load_status} / 設定読込失敗: {e}"),
                }
                self.mark_dirty();
            }
            Err(e) => {
                self.status = format!("読み込み失敗: {e}");
            }
        }
    }

    fn mark_dirty(&mut self) {
        self.generation = self.generation.wrapping_add(1);
        self.result_dirty = true;
        self.mask_dirty = true;
        self.mask_dirty_tiles = None;
        self.last_edit = Instant::now();
    }

    fn mark_dirty_tiles(&mut self, new_tiles: BTreeSet<(usize, usize)>) {
        self.generation = self.generation.wrapping_add(1);
        self.result_dirty = true;
        if new_tiles.is_empty() {
            self.mask_dirty = true;
            self.mask_dirty_tiles = None;
            self.last_edit = Instant::now();
            return;
        };
        if self.mask_dirty && self.mask_dirty_tiles.is_none() {
            self.last_edit = Instant::now();
            return;
        }
        let tiles = self.mask_dirty_tiles.get_or_insert_with(BTreeSet::new);
        tiles.extend(new_tiles);
        self.mask_dirty = true;
        self.last_edit = Instant::now();
    }

    fn push_undo_snapshot(&mut self) {
        self.undo_stack.push(self.layers.clone());
        let limit = self.undo_snapshot_limit();
        while self.undo_stack.len() > limit {
            self.undo_stack.remove(0);
        }
        self.redo_stack.clear();
    }

    fn undo_snapshot_limit(&self) -> usize {
        let pixels = self
            .image_dims()
            .map(|(w, h)| w.saturating_mul(h))
            .unwrap_or(0);
        if pixels >= LARGE_UNDO_PIXEL_COUNT {
            MAX_UNDO_SNAPSHOTS_LARGE
        } else {
            MAX_UNDO_SNAPSHOTS_NORMAL
        }
    }

    fn undo(&mut self) {
        if let Some(layers) = self.undo_stack.pop() {
            self.redo_stack.push(self.layers.clone());
            let limit = self.undo_snapshot_limit();
            while self.redo_stack.len() > limit {
                self.redo_stack.remove(0);
            }
            self.layers = layers;
            self.selected_layer = self.selected_layer.min(self.layers.len().saturating_sub(1));
            self.shape_drag = None;
            self.reset_override_edit_state_for_selected_layer();
            self.mark_dirty();
            self.status = "元に戻しました。".to_string();
        }
    }

    fn redo(&mut self) {
        if let Some(layers) = self.redo_stack.pop() {
            self.undo_stack.push(self.layers.clone());
            let limit = self.undo_snapshot_limit();
            while self.undo_stack.len() > limit {
                self.undo_stack.remove(0);
            }
            self.layers = layers;
            self.selected_layer = self.selected_layer.min(self.layers.len().saturating_sub(1));
            self.shape_drag = None;
            self.reset_override_edit_state_for_selected_layer();
            self.mark_dirty();
            self.status = "やり直しました。".to_string();
        }
    }

    fn switch_tool(&mut self, tool: MaskTool) {
        if self.tool == tool {
            return;
        }
        self.tool = tool;
        self.lasso_points.clear();
        self.shape_drag_start = None;
        self.shape_drag_end = None;
        self.shape_drag = None;
        self.prev_paint_pos = None;
        self.last_paint_pos = None;
        self.edge_brush_seed = None;
        if tool != MaskTool::Select {
            self.selected_shape = None;
        }
        if matches!(tool, MaskTool::EdgeBrush | MaskTool::Polygon) {
            self.ensure_edge_mask_cache();
        }
        self.mask_dirty = true;
    }

    fn update_selected_shape(&mut self, f: impl FnOnce(MaskShape) -> MaskShape) -> bool {
        let Some(shape_idx) = self.selected_shape else {
            return false;
        };
        self.push_undo_snapshot();
        if let Some(mask) = self.selected_edit_raster_vector_mask_mut()
            && let Some(slot) = mask.shapes.get_mut(shape_idx)
        {
            *slot = f(*slot);
            self.mark_dirty();
            return true;
        }
        false
    }

    fn image_dims(&self) -> Option<(usize, usize)> {
        self.image
            .as_ref()
            .map(|image| (image.source.width, image.source.height))
    }

    fn ensure_crop_rect(&mut self) -> Option<CropRect> {
        let (w, h) = self.image_dims()?;
        let current = self.crop_rect.unwrap_or_else(|| CropRect::full(w, h));
        let sanitized = current.sanitized(w, h);
        self.crop_rect = Some(sanitized);
        Some(sanitized)
    }

    fn effective_crop_rect(&self) -> Option<CropRect> {
        if !self.crop_enabled {
            return None;
        }
        let (w, h) = self.image_dims()?;
        Some(
            self.crop_rect
                .unwrap_or_else(|| CropRect::full(w, h))
                .sanitized(w, h),
        )
    }

    fn selected_layer_mut(&mut self) -> Option<&mut LocalAdjustmentLayer> {
        self.layers.get_mut(self.selected_layer)
    }

    fn selected_layer_ref(&self) -> Option<&LocalAdjustmentLayer> {
        self.layers.get(self.selected_layer)
    }

    fn record_brush_perf(
        &mut self,
        elapsed: Duration,
        input_points: usize,
        stamps: usize,
        changed_stamps: usize,
        max_input_gap_px: f32,
    ) {
        self.perf_stats.brush_frames += 1;
        self.perf_stats.brush_input_points += input_points as u64;
        self.perf_stats.brush_stamps += stamps as u64;
        self.perf_stats.brush_changed_stamps += changed_stamps as u64;
        let ms = elapsed.as_secs_f64() * 1000.0;
        self.perf_stats.brush_ms_total += ms;
        self.perf_stats.brush_ms_max = self.perf_stats.brush_ms_max.max(ms);
        self.perf_stats.brush_input_gap_px_max =
            self.perf_stats.brush_input_gap_px_max.max(max_input_gap_px);
    }

    fn flush_perf_log(&mut self) {
        let period = self.perf_last_log.elapsed();
        if period < Duration::from_secs(1) {
            return;
        }
        if !self.perf_stats.has_activity() {
            self.perf_stats.ui_frames = 0;
            self.perf_stats.frame_gap_samples = 0;
            self.perf_stats.frame_gap_ms_total = 0.0;
            self.perf_stats.frame_gap_ms_max = 0.0;
            self.perf_stats.app_update_ms_total = 0.0;
            self.perf_stats.app_update_ms_max = 0.0;
            self.perf_stats.eframe_cpu_samples = 0;
            self.perf_stats.eframe_cpu_ms_total = 0.0;
            self.perf_stats.eframe_cpu_ms_max = 0.0;
            self.perf_last_log = Instant::now();
            return;
        }
        let stats = std::mem::take(&mut self.perf_stats);
        let period_sec = period.as_secs_f64().max(0.001);
        let ui_fps = stats.ui_frames as f64 / period_sec;
        let avg_frame_gap_ms = avg_ms(stats.frame_gap_ms_total, stats.frame_gap_samples);
        let avg_app_update_ms = avg_ms(stats.app_update_ms_total, stats.ui_frames);
        let avg_eframe_cpu_ms = avg_ms(stats.eframe_cpu_ms_total, stats.eframe_cpu_samples);
        let avg_brush_ms = avg_ms(stats.brush_ms_total, stats.brush_frames);
        let avg_input_points = avg_count(stats.brush_input_points, stats.brush_frames);
        let avg_stamps = avg_count(stats.brush_stamps, stats.brush_frames);
        let avg_mask_total_ms = avg_ms(stats.mask_total_ms_total, stats.mask_updates);
        let avg_mask_eval_ms = avg_ms(stats.mask_eval_ms_total, stats.mask_updates);
        let avg_mask_texture_ms = avg_ms(stats.mask_texture_ms_total, stats.mask_updates);
        let avg_mask_tiles = avg_count(stats.mask_tiles_updated, stats.mask_updates);
        let avg_render_ms = avg_ms(stats.render_ms_total, stats.render_jobs);
        let line = format!(
            "t={:.1}s period_ms={:.1} ui_frames={} ui_fps={:.1} frame_gap_ms_avg={:.2} frame_gap_ms_max={:.2} app_update_ms_avg={:.2} app_update_ms_max={:.2} eframe_cpu_ms_avg={:.2} eframe_cpu_ms_max={:.2} brush_frames={} input/frame={:.2} stamps/frame={:.2} changed_stamps={} brush_ms_avg={:.2} brush_ms_max={:.2} input_gap_px_max={:.1} mask_updates={} mask_tiles/update={:.2} mask_ms_avg={:.2} mask_ms_max={:.2} mask_eval_ms_avg={:.2} mask_texture_ms_avg={:.2} render_jobs={} render_ms_avg={:.2} render_ms_max={:.2}",
            self.perf_started_at.elapsed().as_secs_f32(),
            period_sec * 1000.0,
            stats.ui_frames,
            ui_fps,
            avg_frame_gap_ms,
            stats.frame_gap_ms_max,
            avg_app_update_ms,
            stats.app_update_ms_max,
            avg_eframe_cpu_ms,
            stats.eframe_cpu_ms_max,
            stats.brush_frames,
            avg_input_points,
            avg_stamps,
            stats.brush_changed_stamps,
            avg_brush_ms,
            stats.brush_ms_max,
            stats.brush_input_gap_px_max,
            stats.mask_updates,
            avg_mask_tiles,
            avg_mask_total_ms,
            stats.mask_total_ms_max,
            avg_mask_eval_ms,
            avg_mask_texture_ms,
            stats.render_jobs,
            avg_render_ms,
            stats.render_ms_max,
        );
        if let Some(parent) = self.perf_log_path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        if let Ok(mut file) = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.perf_log_path)
        {
            let _ = writeln!(file, "{line}");
        }
        eprintln!("{line}");
        self.perf_last_log = Instant::now();
    }

    fn preview_layer_count(&self) -> usize {
        if self.preview_to_selected_layer && !self.layers.is_empty() {
            self.selected_layer.min(self.layers.len() - 1) + 1
        } else {
            self.layers.len()
        }
    }

    fn preview_layers(&self) -> Vec<LocalAdjustmentLayer> {
        self.layers[..self.preview_layer_count()].to_vec()
    }

    fn selected_mask_kind(&self) -> Option<MaskKind> {
        self.selected_layer_ref()
            .map(|layer| MaskKind::from_mask(&layer.mask))
    }

    fn reset_override_edit_state_for_selected_layer(&mut self) {
        self.override_edit_panel = None;
        self.selected_shape = None;
        if self.selected_mask_kind() == Some(MaskKind::Raster) {
            self.reset_brush_stroke();
            self.mask_dirty = true;
        } else {
            self.switch_tool(MaskTool::Select);
        }
    }

    fn add_layer_with_mask(&mut self, mask_kind: MaskKind) {
        let (w, h) = self.image_dims().unwrap_or((1, 1));
        let layer = layer_with_mask(
            format!("部分補正 {}", self.layers.len() + 1),
            mask_kind,
            w,
            h,
        );
        self.push_undo_snapshot();
        self.layers.push(layer);
        self.selected_layer = self.layers.len().saturating_sub(1);
        self.selected_shape = None;
        self.tool = if mask_kind == MaskKind::Raster {
            MaskTool::Brush
        } else {
            MaskTool::Select
        };
        self.override_edit_panel = None;
        self.add_layer_mask_kind = mask_kind;
        self.add_layer_dialog_open = false;
        self.mark_dirty();
    }

    fn toggle_override_edit_panel(&mut self, target: OverrideEditTarget) {
        if self.override_edit_panel == Some(target) {
            self.reset_override_edit_state_for_selected_layer();
            return;
        }
        self.override_edit_panel = Some(target);
        self.selected_shape = None;
        if self.tool == MaskTool::Select {
            self.switch_tool(MaskTool::Brush);
        } else {
            self.reset_brush_stroke();
            self.mask_dirty = true;
        }
    }

    fn duplicate_layer(&mut self) {
        let Some(layer) = self.selected_layer_ref().cloned() else {
            return;
        };
        self.push_undo_snapshot();
        let mut copy = layer;
        copy.name = format!("{} copy", copy.name);
        let insert_at = (self.selected_layer + 1).min(self.layers.len());
        self.layers.insert(insert_at, copy);
        self.selected_layer = insert_at;
        self.reset_override_edit_state_for_selected_layer();
        self.mark_dirty();
    }

    fn copy_mask_from_layer(&mut self, source_idx: usize) {
        if source_idx == self.selected_layer || source_idx >= self.layers.len() {
            return;
        }
        let Some(source) = self.layers.get(source_idx).cloned() else {
            return;
        };
        self.push_undo_snapshot();
        if let Some(target) = self.selected_layer_mut() {
            copy_mask_fields_from_layer(target, &source);
        }
        self.selected_shape = None;
        self.status = format!("レイヤー {} からマスクをコピーしました。", source_idx + 1);
        self.mark_dirty();
    }

    fn clear_selected_manual_override(&mut self) {
        let Some(layer) = self.selected_layer_ref() else {
            return;
        };
        if layer.manual_override.is_empty() {
            return;
        }
        self.push_undo_snapshot();
        if let Some(layer) = self.selected_layer_mut() {
            layer.manual_override = ManualMaskOverride::default();
        }
        self.selected_shape = None;
        self.status = "追加/削除の手動補正をクリアしました。".to_string();
        self.mark_dirty();
    }

    fn remove_selected_layer(&mut self) {
        if self.layers.is_empty() {
            return;
        }
        self.push_undo_snapshot();
        self.layers.remove(self.selected_layer);
        self.selected_layer = self.selected_layer.min(self.layers.len().saturating_sub(1));
        self.reset_override_edit_state_for_selected_layer();
        self.mark_dirty();
    }

    fn move_selected_layer(&mut self, delta: isize) {
        let len = self.layers.len();
        if len < 2 {
            return;
        }
        let new_idx = (self.selected_layer as isize + delta).clamp(0, len as isize - 1) as usize;
        if new_idx == self.selected_layer {
            return;
        }
        self.push_undo_snapshot();
        self.layers.swap(self.selected_layer, new_idx);
        self.selected_layer = new_idx;
        self.selected_shape = None;
        self.mark_dirty();
    }

    fn selected_raster_vector_mask_mut(&mut self) -> Option<&mut RasterVectorMask> {
        let (w, h) = self.image_dims()?;
        let layer = self.selected_layer_mut()?;
        if matches!(layer.mask, LocalMask::Raster(_)) {
            let old_mask = std::mem::replace(&mut layer.mask, LocalMask::Full);
            if let LocalMask::Raster(mask) = old_mask {
                let alpha = if mask.width == w && mask.height == h && mask.alpha.len() == w * h {
                    mask.alpha
                } else {
                    vec![0.0; w.saturating_mul(h)]
                };
                layer.mask = LocalMask::RasterVector(RasterVectorMask {
                    width: w,
                    height: h,
                    alpha,
                    shapes: Vec::new(),
                });
            }
        }
        match &mut layer.mask {
            LocalMask::RasterVector(mask) => {
                if mask.width != w || mask.height != h {
                    *mask = RasterVectorMask::empty(w, h);
                }
                Some(mask)
            }
            _ => None,
        }
    }

    fn selected_raster_vector_mask_ref(&self) -> Option<&RasterVectorMask> {
        match &self.selected_layer_ref()?.mask {
            LocalMask::RasterVector(mask) => Some(mask),
            _ => None,
        }
    }

    fn selected_edit_raster_vector_mask_mut(&mut self) -> Option<&mut RasterVectorMask> {
        let selected_is_manual = self
            .selected_layer_ref()
            .map(|layer| MaskKind::from_mask(&layer.mask) == MaskKind::Raster)
            .unwrap_or(false);
        if selected_is_manual {
            return self.selected_raster_vector_mask_mut();
        }
        let (w, h) = self.image_dims()?;
        let target = self.override_edit_panel?;
        let layer = self.selected_layer_mut()?;
        let slot = match target {
            OverrideEditTarget::Add => &mut layer.manual_override.add,
            OverrideEditTarget::Subtract => &mut layer.manual_override.subtract,
        };
        let needs_reset = slot
            .as_ref()
            .map(|mask| mask.width != w || mask.height != h || mask.alpha.len() != w * h)
            .unwrap_or(true);
        if needs_reset {
            *slot = Some(RasterVectorMask::empty(w, h));
        }
        slot.as_mut()
    }

    fn selected_edit_raster_vector_mask_ref(&self) -> Option<&RasterVectorMask> {
        let layer = self.selected_layer_ref()?;
        if MaskKind::from_mask(&layer.mask) == MaskKind::Raster {
            return self.selected_raster_vector_mask_ref();
        }
        match self.override_edit_panel? {
            OverrideEditTarget::Add => layer.manual_override.add.as_ref(),
            OverrideEditTarget::Subtract => layer.manual_override.subtract.as_ref(),
        }
    }

    fn poll_render(&mut self, ctx: &egui::Context) {
        let Some(pending) = &self.pending else {
            return;
        };
        match pending.rx.try_recv() {
            Ok(Ok(result)) => {
                let generation = pending.generation;
                let render_ms = pending.started_at.elapsed().as_secs_f64() * 1000.0;
                self.pending = None;
                self.perf_stats.render_jobs += 1;
                self.perf_stats.render_ms_total += render_ms;
                self.perf_stats.render_ms_max = self.perf_stats.render_ms_max.max(render_ms);
                if generation == self.generation {
                    let color_image = color_image_from_rgba(&result);
                    if let Some(texture) = &mut self.result_texture {
                        texture.set(color_image, TextureOptions::LINEAR);
                    } else {
                        self.result_texture = Some(ctx.load_texture(
                            "local_adjust_result",
                            color_image,
                            TextureOptions::LINEAR,
                        ));
                    }
                    self.status = format!("再合成完了: generation {generation}");
                    self.result = Some(result);
                } else {
                    self.status = "古い再合成結果を破棄しました。".to_string();
                }
            }
            Ok(Err(e)) => {
                self.pending = None;
                self.status = format!("再合成失敗: {e}");
            }
            Err(mpsc::TryRecvError::Empty) => {}
            Err(mpsc::TryRecvError::Disconnected) => {
                self.pending = None;
                self.status = "再合成 worker が停止しました。".to_string();
            }
        }
    }

    fn poll_segmentation(&mut self, ctx: &egui::Context) {
        let recv_result = {
            let Some(pending) = self.segmentation_pending.as_ref() else {
                return;
            };
            pending.rx.try_recv()
        };
        match recv_result {
            Ok(Ok(generated)) => {
                let Some(pending) = self.segmentation_pending.take() else {
                    return;
                };
                let elapsed_ms = pending.started_at.elapsed().as_secs_f64() * 1000.0;
                let layer_idx = pending.layer_idx;
                if self.generation != pending.generation {
                    self.status = "セグメンテーション結果を破棄しました。画像またはレイヤーが変更されています。"
                        .to_string();
                    return;
                }
                let layer_matches = matches!(
                    (
                        self.layers.get(layer_idx).map(|layer| &layer.mask),
                        &generated
                    ),
                    (Some(LocalMask::Subject(_)), GeneratedMask::Subject(_))
                        | (Some(LocalMask::Segmentation(_)), GeneratedMask::Regions(_))
                );
                if !layer_matches {
                    self.status =
                        "生成結果を破棄しました。対象レイヤーが変更されています。".to_string();
                    return;
                }
                self.push_undo_snapshot();
                let status = match generated {
                    GeneratedMask::Subject(mask) => {
                        if let Some(layer) = self.layers.get_mut(layer_idx) {
                            layer.mask = LocalMask::Subject(mask);
                        }
                        format!("被写体マスク生成完了: {elapsed_ms:.0}ms")
                    }
                    GeneratedMask::Regions(mask) => {
                        let label_count = mask.label_count();
                        if let Some(layer) = self.layers.get_mut(layer_idx) {
                            layer.mask = LocalMask::Segmentation(mask);
                        }
                        format!("領域分割完了: {label_count} 領域 / {elapsed_ms:.0}ms")
                    }
                };
                self.selected_layer = layer_idx;
                self.selected_shape = None;
                self.status = status;
                self.mark_dirty();
                ctx.request_repaint();
            }
            Ok(Err(e)) => {
                self.segmentation_pending = None;
                self.status = format!("被写体マスク生成失敗: {e}");
            }
            Err(mpsc::TryRecvError::Empty) => {}
            Err(mpsc::TryRecvError::Disconnected) => {
                self.segmentation_pending = None;
                self.status = "セグメンテーション worker が停止しました。".to_string();
            }
        }
    }

    fn start_subject_segmentation(&mut self, ctx: &egui::Context) {
        if self.segmentation_pending.is_some() {
            self.status = "被写体マスク生成中です。".to_string();
            return;
        }
        let layer_idx = self.selected_layer;
        if !matches!(
            self.layers.get(layer_idx).map(|layer| &layer.mask),
            Some(LocalMask::Subject(_))
        ) {
            self.status = "被写体選択レイヤーを選択してください。".to_string();
            return;
        }
        let Some(image) = &self.image else {
            return;
        };
        let model_path = segmentation_model_path();
        if !model_path.is_file() {
            self.status = format!("モデルが見つかりません: {}", model_path.display());
            return;
        }
        let source = image.source.clone();
        let (tx, rx) = mpsc::channel();
        let spawn_result = std::thread::Builder::new()
            .name("local-adjust-lab-segmentation".to_string())
            .spawn(move || {
                let result =
                    run_u2netp_segmentation(&source, &model_path).map(GeneratedMask::Subject);
                let _ = tx.send(result);
            });
        match spawn_result {
            Ok(_) => {
                self.segmentation_pending = Some(SegmentationPending {
                    layer_idx,
                    generation: self.generation,
                    rx,
                    started_at: Instant::now(),
                });
                self.status = "被写体マスク生成中...".to_string();
                ctx.request_repaint();
            }
            Err(e) => {
                self.status = format!("セグメンテーション worker 起動失敗: {e}");
            }
        }
    }

    fn start_region_segmentation(&mut self, ctx: &egui::Context, use_subject: bool) {
        if self.segmentation_pending.is_some() {
            self.status = "マスク生成中です。".to_string();
            return;
        }
        let layer_idx = self.selected_layer;
        if !matches!(
            self.layers.get(layer_idx).map(|layer| &layer.mask),
            Some(LocalMask::Segmentation(_))
        ) {
            self.status = "領域分割レイヤーを選択してください。".to_string();
            return;
        }
        let Some(image) = &self.image else {
            return;
        };
        let source = image.source.clone();
        let subject = if use_subject {
            self.subject_mask_candidate()
        } else {
            None
        };
        if use_subject && subject.is_none() {
            self.status = "利用できる被写体選択マスクがありません。".to_string();
            return;
        }
        let color_tolerance = self.region_color_tolerance;
        let min_area = self.region_min_area.max(1);
        let edge_threshold = self.boundary_edge_threshold.round().clamp(0.0, 255.0) as u8;
        let ink_threshold = self.boundary_ink_threshold.round().clamp(0.0, 255.0) as u8;
        let gap_px = self.boundary_gap_px.round().clamp(0.0, 8.0) as usize;
        let (tx, rx) = mpsc::channel();
        let spawn_result = std::thread::Builder::new()
            .name("local-adjust-lab-region-segmentation".to_string())
            .spawn(move || {
                let result = build_region_segmentation(
                    &source,
                    subject.as_ref(),
                    color_tolerance,
                    min_area,
                    edge_threshold,
                    ink_threshold,
                    gap_px,
                )
                .map(GeneratedMask::Regions);
                let _ = tx.send(result);
            });
        match spawn_result {
            Ok(_) => {
                self.segmentation_pending = Some(SegmentationPending {
                    layer_idx,
                    generation: self.generation,
                    rx,
                    started_at: Instant::now(),
                });
                self.status = if use_subject {
                    "被写体内を領域分割中...".to_string()
                } else {
                    "画像全体を領域分割中...".to_string()
                };
                ctx.request_repaint();
            }
            Err(e) => {
                self.status = format!("領域分割 worker 起動失敗: {e}");
            }
        }
    }

    fn subject_mask_candidate(&self) -> Option<RasterMask> {
        let (w, h) = self.image_dims()?;
        self.layers.iter().find_map(|layer| match &layer.mask {
            LocalMask::Subject(mask)
                if mask.width == w
                    && mask.height == h
                    && mask.alpha.iter().any(|&alpha| alpha > 0.02) =>
            {
                Some(mask.clone())
            }
            _ => None,
        })
    }

    fn maybe_start_render(&mut self, ctx: &egui::Context) {
        if !self.result_dirty || self.pending.is_some() {
            return;
        }
        if ctx.input(|i| i.pointer.primary_down()) {
            ctx.request_repaint_after(Duration::from_millis(RESULT_RENDER_DRAG_RECHECK_MS));
            return;
        }
        if self.last_edit.elapsed() < Duration::from_millis(120) {
            ctx.request_repaint_after(Duration::from_millis(50));
            return;
        }
        let Some(image) = &self.image else {
            return;
        };
        let source = image.source.clone();
        let layers = self.preview_layers();
        let generation = self.generation;
        let limited_preview = self.preview_to_selected_layer && layers.len() < self.layers.len();
        let (tx, rx) = mpsc::channel();
        let spawn_result = std::thread::Builder::new()
            .name("local-adjust-lab-render".to_string())
            .spawn(move || {
                let result = apply_layers(source.as_ref(), &layers).map_err(|e| e.to_string());
                let _ = tx.send(result);
            });
        match spawn_result {
            Ok(_) => {
                self.pending = Some(RenderPending {
                    generation,
                    rx,
                    started_at: Instant::now(),
                });
                self.result_dirty = false;
                self.status = if limited_preview {
                    format!("再合成中: generation {generation} (選択レイヤーまで)")
                } else {
                    format!("再合成中: generation {generation}...")
                };
                ctx.request_repaint();
            }
            Err(e) => {
                self.result_dirty = true;
                self.status = format!("再合成 worker 起動失敗: {e}");
            }
        }
    }

    fn ensure_mask_texture(&mut self, ctx: &egui::Context) {
        let Some(image) = &self.image else {
            return;
        };
        let width = image.source.width;
        let height = image.source.height;
        if self.selected_layer >= self.layers.len() {
            return;
        }
        let region_animation = self.show_mask
            && matches!(
                self.layers
                    .get(self.selected_layer)
                    .map(|layer| &layer.mask),
                Some(LocalMask::Segmentation(_))
            );
        if !self.mask_dirty && region_animation {
            let elapsed = self.last_mask_preview_update.elapsed();
            let interval = Duration::from_millis(REGION_BOUNDARY_ANIM_INTERVAL_MS);
            if elapsed >= interval {
                self.mask_dirty = true;
                self.mask_dirty_tiles = None;
            } else {
                ctx.request_repaint_after(interval.saturating_sub(elapsed));
                return;
            }
        }
        if !self.mask_dirty {
            return;
        }
        let use_fast_tile_eval =
            can_build_mask_tiles_from_layer(&self.layers[self.selected_layer], width, height);
        if !use_fast_tile_eval
            && ctx.input(|i| i.pointer.primary_down())
            && self.last_mask_preview_update.elapsed()
                < Duration::from_millis(MASK_PREVIEW_DRAG_INTERVAL_MS)
        {
            ctx.request_repaint_after(Duration::from_millis(MASK_PREVIEW_DRAG_INTERVAL_MS));
            return;
        }
        let total_start = Instant::now();
        let eval_start = Instant::now();
        let mask = if use_fast_tile_eval {
            None
        } else {
            match evaluate_layer_mask(image.source.as_ref(), &self.layers[self.selected_layer]) {
                Ok(mask) => Some(mask),
                Err(e) => {
                    self.status = format!("マスクプレビュー失敗: {e}");
                    return;
                }
            }
        };
        let eval_ms = eval_start.elapsed().as_secs_f64() * 1000.0;
        let time_sec = ctx.input(|i| i.time) as f32;
        let texture_start = Instant::now();
        let needs_new_cache = self
            .mask_tiles
            .as_ref()
            .map(|cache| !cache.matches_size(width, height))
            .unwrap_or(true);
        if needs_new_cache {
            self.mask_tiles = Some(MaskTilePreview::new(width, height, MASK_PREVIEW_TILE_SIZE));
            self.mask_dirty_tiles = None;
        }
        let dirty_tiles = self.mask_dirty_tiles.clone();
        let mut updated_tiles = 0_u64;
        if let Some(cache) = &mut self.mask_tiles {
            let tile_iter: Box<dyn Iterator<Item = (usize, usize)>> = if let Some(tiles) =
                dirty_tiles
            {
                Box::new(tiles.into_iter())
            } else {
                Box::new((0..cache.rows).flat_map(|row| (0..cache.cols).map(move |col| (col, row))))
            };
            for (col, row) in tile_iter {
                if col >= cache.cols || row >= cache.rows {
                    continue;
                }
                let tile_idx = row * cache.cols + col;
                let tile_x = col * cache.tile_size;
                let tile_y = row * cache.tile_size;
                let tile_w = cache.tile_size.min(width - tile_x);
                let tile_h = cache.tile_size.min(height - tile_y);
                let tile_image = if let Some(mask) = mask.as_ref() {
                    build_mask_tile_image(
                        mask,
                        &self.layers[self.selected_layer],
                        width,
                        tile_x,
                        tile_y,
                        tile_w,
                        tile_h,
                    )
                } else {
                    build_mask_tile_image_from_layer(
                        &self.layers[self.selected_layer],
                        width,
                        tile_x,
                        tile_y,
                        tile_w,
                        tile_h,
                        time_sec,
                    )
                };
                if let Some(texture) = cache.tiles[tile_idx].as_mut() {
                    texture.set(tile_image, TextureOptions::NEAREST);
                } else {
                    let texture = ctx.load_texture(
                        format!("local_adjust_mask_tile_{col}_{row}"),
                        tile_image,
                        TextureOptions::NEAREST,
                    );
                    cache.tiles[tile_idx] = Some(texture);
                }
                updated_tiles += 1;
            }
        }
        self.last_mask_preview_update = Instant::now();
        self.mask_dirty = false;
        self.mask_dirty_tiles = None;
        let texture_ms = texture_start.elapsed().as_secs_f64() * 1000.0;
        let total_ms = total_start.elapsed().as_secs_f64() * 1000.0;
        self.perf_stats.mask_updates += 1;
        self.perf_stats.mask_eval_ms_total += eval_ms;
        self.perf_stats.mask_texture_ms_total += texture_ms;
        self.perf_stats.mask_total_ms_total += total_ms;
        self.perf_stats.mask_total_ms_max = self.perf_stats.mask_total_ms_max.max(total_ms);
        self.perf_stats.mask_tiles_updated += updated_tiles;
        if region_animation {
            ctx.request_repaint_after(Duration::from_millis(REGION_BOUNDARY_ANIM_INTERVAL_MS));
        }
    }

    fn preview_indicator(&self, ctx: &egui::Context) -> (&'static str, Color32) {
        if self.image.is_none() {
            return ("待機", Color32::from_gray(130));
        }
        if self.pending.is_some() {
            let phase = ((ctx.input(|i| i.time) * 4.5).sin() * 0.5 + 0.5) as f32;
            let g = (150.0 + 90.0 * phase).round() as u8;
            return ("再合成中", Color32::from_rgb(255, g, 60));
        }
        if self.segmentation_pending.is_some() {
            let phase = ((ctx.input(|i| i.time) * 4.5).sin() * 0.5 + 0.5) as f32;
            let g = (150.0 + 90.0 * phase).round() as u8;
            return ("AI推論中", Color32::from_rgb(255, g, 60));
        }
        if self.result_dirty {
            return ("反映待ち", Color32::from_rgb(255, 210, 80));
        }
        if self.mask_dirty {
            return ("マスク更新", Color32::from_rgb(90, 210, 255));
        }
        if self.preview_to_selected_layer && self.preview_layer_count() < self.layers.len() {
            return ("選択まで", Color32::from_rgb(120, 190, 255));
        }
        ("最新", Color32::from_rgb(90, 220, 120))
    }

    fn save_result(&mut self) {
        let Some(image) = &self.image else {
            return;
        };
        let full_result;
        let result =
            if self.preview_to_selected_layer && self.preview_layer_count() < self.layers.len() {
                self.status = "全レイヤーで保存用に再合成中...".to_string();
                full_result = match apply_layers(image.source.as_ref(), &self.layers) {
                    Ok(result) => result,
                    Err(e) => {
                        self.status = format!("保存用再合成失敗: {e}");
                        return;
                    }
                };
                &full_result
            } else {
                let Some(result) = self.result.as_ref() else {
                    self.status = "No rendered result to save yet.".to_string();
                    return;
                };
                result
            };
        let crop = self.effective_crop_rect();
        let cropped_result;
        let result_to_save = if let Some(crop) = crop {
            cropped_result = crop_rgba_image(result, crop);
            &cropped_result
        } else {
            result
        };
        match save_result_png(&image.path, result_to_save) {
            Ok(path) => {
                self.status = if crop.is_some() {
                    format!("crop して保存しました: {}", path.display())
                } else if self.preview_to_selected_layer {
                    format!("全レイヤーで保存しました: {}", path.display())
                } else {
                    format!("保存しました: {}", path.display())
                };
            }
            Err(e) => self.status = format!("保存失敗: {e}"),
        }
    }

    fn save_settings_sidecar(&mut self) {
        let Some(image) = &self.image else {
            self.status = "画像を読み込んでください。".to_string();
            return;
        };
        let sidecar = match self.build_sidecar(image) {
            Ok(sidecar) => sidecar,
            Err(e) => {
                self.status = format!("設定保存の準備に失敗: {e}");
                return;
            }
        };
        let path = sidecar_path_for_image(&image.path);
        let json = match serde_json::to_string_pretty(&sidecar) {
            Ok(json) => json,
            Err(e) => {
                self.status = format!("設定JSON作成失敗: {e}");
                return;
            }
        };
        match std::fs::write(&path, json) {
            Ok(()) => self.status = format!("設定を保存しました: {}", path.display()),
            Err(e) => self.status = format!("設定保存失敗: {e}"),
        }
    }

    fn build_sidecar(&self, image: &LoadedImage) -> Result<LabSidecar, String> {
        let layers = self
            .layers
            .iter()
            .map(stored_layer_from_local)
            .collect::<Result<Vec<_>, _>>()?;
        Ok(LabSidecar {
            version: LAB_SIDECAR_VERSION,
            image_file: image
                .path
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or("")
                .to_string(),
            image_width: image.source.width,
            image_height: image.source.height,
            crop_enabled: self.crop_enabled,
            crop_overlay: self.crop_overlay,
            crop_rect: self.crop_rect,
            layers,
        })
    }

    fn load_settings_sidecar_for_current_image(&mut self) -> bool {
        let Some(image) = &self.image else {
            self.status = "画像を読み込んでください。".to_string();
            return false;
        };
        let path = sidecar_path_for_image(&image.path);
        match self.load_settings_sidecar_from_path(&path) {
            Ok(true) => true,
            Ok(false) => {
                self.status = format!("設定ファイルはありません: {}", path.display());
                false
            }
            Err(e) => {
                self.status = format!("設定読込失敗: {e}");
                false
            }
        }
    }

    fn load_settings_sidecar_from_path(&mut self, path: &Path) -> Result<bool, String> {
        let text = match std::fs::read_to_string(path) {
            Ok(text) => text,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(false),
            Err(e) => return Err(e.to_string()),
        };
        let sidecar: LabSidecar = serde_json::from_str(&text).map_err(|e| e.to_string())?;
        if sidecar.version != LAB_SIDECAR_VERSION {
            return Err(format!("未対応の .miv version: {}", sidecar.version));
        }
        let Some((w, h)) = self.image_dims() else {
            return Err("画像が読み込まれていません。".to_string());
        };
        if sidecar.image_width != w || sidecar.image_height != h {
            return Err(format!(
                "画像サイズが一致しません: sidecar={}x{}, image={}x{}",
                sidecar.image_width, sidecar.image_height, w, h
            ));
        }
        let layers = sidecar
            .layers
            .iter()
            .map(local_layer_from_stored)
            .collect::<Result<Vec<_>, _>>()?;
        self.layers = layers;
        self.selected_layer = self.selected_layer.min(self.layers.len().saturating_sub(1));
        self.shape_drag = None;
        self.reset_override_edit_state_for_selected_layer();
        self.crop_enabled = sidecar.crop_enabled;
        self.crop_overlay = sidecar.crop_overlay;
        self.crop_rect = sidecar.crop_rect.map(|crop| crop.sanitized(w, h));
        self.crop_drag = None;
        self.undo_stack.clear();
        self.redo_stack.clear();
        self.status = format!("設定を読み込みました: {}", path.display());
        self.mark_dirty();
        Ok(true)
    }

    fn brush_stroke_points(&mut self, p: Pos2) -> Vec<Pos2> {
        let spacing = (self.brush_radius.max(1.0) * BRUSH_STROKE_SPACING_RATIO)
            .clamp(BRUSH_STROKE_MIN_SPACING, BRUSH_STROKE_MAX_SPACING);
        let Some(last) = self.last_paint_pos else {
            self.last_paint_pos = Some(p);
            return vec![p];
        };
        let points = if let Some(prev) = self.prev_paint_pos {
            let extrapolated_next = p + (p - last);
            catmull_rom_stroke_points(prev, last, p, extrapolated_next, spacing)
        } else {
            interpolated_stroke_points(Some(last), p, spacing)
        };
        self.prev_paint_pos = Some(last);
        self.last_paint_pos = Some(p);
        points
    }

    fn begin_brush_stroke(&mut self) {
        self.push_undo_snapshot();
        self.prev_paint_pos = None;
        self.last_paint_pos = None;
        self.edge_brush_seed = None;
    }

    fn reset_brush_stroke(&mut self) {
        self.prev_paint_pos = None;
        self.last_paint_pos = None;
        self.edge_brush_seed = None;
    }

    fn paint_raster_stamp(&mut self, p: Pos2, add: bool) -> bool {
        let Some((w, h)) = self.image_dims() else {
            return false;
        };
        let radius = self.brush_radius.max(1.0);
        let Some(mask) = self.selected_edit_raster_vector_mask_mut() else {
            return false;
        };
        let alpha = &mut mask.alpha;
        let min_y = (p.y - radius).floor().max(0.0) as usize;
        let max_y = (p.y + radius).ceil().min(h as f32 - 1.0) as usize;
        let r2 = radius * radius;
        let target = if add { 1.0 } else { 0.0 };
        let mut changed = false;
        for y in min_y..=max_y {
            let dy = y as f32 + 0.5 - p.y;
            let span_sq = r2 - dy * dy;
            if span_sq < 0.0 {
                continue;
            }
            let span = span_sq.sqrt();
            let min_x = (p.x - span - 0.5).ceil().max(0.0) as usize;
            let max_x = (p.x + span - 0.5).floor().min(w as f32 - 1.0) as usize;
            if min_x > max_x {
                continue;
            }
            let row_start = y * w + min_x;
            let row_end = y * w + max_x;
            for value in &mut alpha[row_start..=row_end] {
                if (*value - target).abs() > f32::EPSILON {
                    *value = target;
                    changed = true;
                }
            }
        }
        changed
    }

    fn source_rgb_at(&self, p: Pos2) -> Option<[u8; 3]> {
        let image = &self.image.as_ref()?.source;
        let x = p.x.floor().clamp(0.0, image.width.saturating_sub(1) as f32) as usize;
        let y =
            p.y.floor()
                .clamp(0.0, image.height.saturating_sub(1) as f32) as usize;
        let idx = (y * image.width + x) * 4;
        Some([
            image.pixels.get(idx).copied()?,
            image.pixels.get(idx + 1).copied()?,
            image.pixels.get(idx + 2).copied()?,
        ])
    }

    fn paint_edge_brush_stamp(&mut self, p: Pos2, add: bool) -> bool {
        let Some(seed) = self.edge_brush_seed else {
            return false;
        };
        if self.ensure_edge_mask_cache().is_none() {
            return false;
        }
        let Some(image) = self.image.as_ref().map(|image| &image.source) else {
            return false;
        };
        let Some(edge_mask) = self.edge_mask.as_ref().map(|cache| cache.mask.as_slice()) else {
            return false;
        };
        let w = image.width;
        let h = image.height;
        if w == 0 || h == 0 {
            return false;
        }

        let radius = self.brush_radius.max(1.0);
        let brush_alpha = 1.0;
        let tolerance = self.edge_brush_tolerance.clamp(0.0, 255.0).round() as i16;
        let min_x = (p.x - radius).floor().max(0.0) as usize;
        let max_x = (p.x + radius).ceil().min(w as f32 - 1.0) as usize;
        let min_y = (p.y - radius).floor().max(0.0) as usize;
        let max_y = (p.y + radius).ceil().min(h as f32 - 1.0) as usize;
        let r2 = radius * radius;
        let start_x = p.x.floor().clamp(min_x as f32, max_x as f32) as usize;
        let start_y = p.y.floor().clamp(min_y as f32, max_y as f32) as usize;
        if !edge_brush_pixel_allowed(image, edge_mask, start_x, start_y, seed, tolerance) {
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
            let dx = x as f32 + 0.5 - p.x;
            let dy = y as f32 + 0.5 - p.y;
            if dx * dx + dy * dy > r2 {
                continue;
            }
            if !edge_brush_pixel_allowed(image, edge_mask, x, y, seed, tolerance) {
                continue;
            }
            targets.push(y * w + x);
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
        if self.edge_brush_include_boundary {
            include_adjacent_boundary_pixels(
                image,
                &mut targets,
                &mut target_map,
                min_x,
                max_x,
                min_y,
                max_y,
                bw,
                p,
                r2,
                edge_mask,
            );
        }

        let mut changed = false;
        if let Some(mask) = self.selected_edit_raster_vector_mask_mut() {
            for idx in targets {
                if idx >= mask.alpha.len() {
                    continue;
                }
                let before = mask.alpha[idx];
                if add {
                    mask.alpha[idx] = mask.alpha[idx].max(brush_alpha);
                } else {
                    mask.alpha[idx] = 0.0;
                }
                changed |= (mask.alpha[idx] - before).abs() > f32::EPSILON;
            }
        }
        changed
    }

    fn paint_gap_fill_brush_stamp(&mut self, p: Pos2, add: bool) -> bool {
        if !add {
            return self.paint_raster_stamp(p, false);
        }
        let Some((w, h)) = self.image_dims() else {
            return false;
        };
        let radius = self.brush_radius.max(1.0);
        let gap = self.gap_fill_distance.round().clamp(1.0, 64.0) as usize;
        let min_x = (p.x - radius).floor().max(0.0) as usize;
        let max_x = (p.x + radius).ceil().min(w as f32 - 1.0) as usize;
        let min_y = (p.y - radius).floor().max(0.0) as usize;
        let max_y = (p.y + radius).ceil().min(h as f32 - 1.0) as usize;
        let r2 = radius * radius;

        let Some(mask) = self.selected_edit_raster_vector_mask_mut() else {
            return false;
        };
        let src = mask.alpha.clone();
        let mut targets = Vec::new();
        for y in min_y..=max_y {
            for x in min_x..=max_x {
                let dx = x as f32 + 0.5 - p.x;
                let dy = y as f32 + 0.5 - p.y;
                if dx * dx + dy * dy > r2 {
                    continue;
                }
                let idx = y * w + x;
                if src[idx] > 0.5 {
                    continue;
                }
                if gap_between_masked_pixels(&src, w, h, x, y, gap) {
                    targets.push(idx);
                }
            }
        }
        if targets.is_empty() {
            return false;
        }
        let mut changed = false;
        for idx in targets {
            let before = mask.alpha[idx];
            mask.alpha[idx] = 1.0;
            changed |= (mask.alpha[idx] - before).abs() > f32::EPSILON;
        }
        changed
    }

    fn snap_point_to_edge(&self, p: Pos2, radius: f32) -> Pos2 {
        let Some(image) = self.image.as_ref().map(|image| &image.source) else {
            return p;
        };
        let w = image.width;
        let h = image.height;
        if w == 0 || h == 0 {
            return p;
        }
        let radius = radius.max(1.0);
        let min_x = (p.x - radius).floor().max(0.0) as usize;
        let max_x = (p.x + radius).ceil().min(w as f32 - 1.0) as usize;
        let min_y = (p.y - radius).floor().max(0.0) as usize;
        let max_y = (p.y + radius).ceil().min(h as f32 - 1.0) as usize;
        let r2 = radius * radius;
        let edge_threshold = self.boundary_edge_threshold.max(0.0);
        let ink_threshold = self.boundary_ink_threshold.max(0.0);
        let gap_px = self.boundary_gap_px.clamp(0.0, 8.0).round() as usize;
        let mut best = None;
        for y in min_y..=max_y {
            for x in min_x..=max_x {
                let dx = x as f32 + 0.5 - p.x;
                let dy = y as f32 + 0.5 - p.y;
                if dx * dx + dy * dy > r2 {
                    continue;
                }
                let strength = boundary_strength_at(image, x, y);
                if !boundary_pixel_at(image, x, y, edge_threshold, ink_threshold, gap_px) {
                    continue;
                }
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
        if let Some((x, y, _)) = best {
            Pos2::new(x as f32 + 0.5, y as f32 + 0.5)
        } else {
            p
        }
    }

    fn apply_bitmap_mask_op(&mut self, op: BitmapMaskOp) {
        let Some((w, h)) = self.image_dims() else {
            return;
        };
        self.push_undo_snapshot();
        if let Some(mask) = self.selected_edit_raster_vector_mask_mut() {
            mask.alpha = match op {
                BitmapMaskOp::Expand => dilate_alpha(&mask.alpha, w, h),
                BitmapMaskOp::Shrink => erode_alpha(&mask.alpha, w, h),
            };
            self.mark_dirty();
        }
    }

    fn edge_overlay_enabled(&self) -> bool {
        matches!(self.tool, MaskTool::EdgeBrush | MaskTool::Polygon)
    }

    fn current_edge_mask_key(&self) -> Option<EdgeMaskKey> {
        let image = self.image.as_ref()?;
        Some(EdgeMaskKey {
            image_path: image.path.clone(),
            source_size: [image.source.width, image.source.height],
            threshold: self.boundary_edge_threshold.clamp(0.0, 255.0).round() as u8,
            ink_threshold: self.boundary_ink_threshold.clamp(0.0, 255.0).round() as u8,
            gap_px: self.boundary_gap_px.clamp(0.0, 8.0).round() as u8,
        })
    }

    fn ensure_edge_mask_cache(&mut self) -> Option<&EdgeMaskCache> {
        let key = self.current_edge_mask_key()?;
        let needs_rebuild = self
            .edge_mask
            .as_ref()
            .map(|cache| cache.key != key)
            .unwrap_or(true);
        if needs_rebuild {
            let image = &self.image.as_ref()?.source;
            let start = Instant::now();
            let mask =
                build_boundary_mask(image, key.threshold, key.ink_threshold, key.gap_px as usize);
            let elapsed_ms = start.elapsed().as_millis();
            self.edge_mask = Some(EdgeMaskCache { key, mask });
            self.edge_preview = None;
            self.status = format!("境界マップ準備完了: {elapsed_ms}ms");
        }
        self.edge_mask.as_ref()
    }

    fn ensure_edge_preview_texture(&mut self, ctx: &egui::Context) -> Option<&TextureHandle> {
        self.ensure_edge_mask_cache()?;
        let image = self.image.as_ref()?;
        let edge_cache = self.edge_mask.as_ref()?;
        let threshold = edge_cache.key.threshold;
        let ink_threshold = edge_cache.key.ink_threshold;
        let gap_px = edge_cache.key.gap_px;
        let preview_size = edge_preview_size(image.source.width, image.source.height);
        let key = EdgePreviewKey {
            image_path: image.path.clone(),
            source_size: [image.source.width, image.source.height],
            preview_size,
            threshold,
            ink_threshold,
            gap_px,
        };
        let needs_rebuild = self
            .edge_preview
            .as_ref()
            .map(|cache| cache.key != key)
            .unwrap_or(true);
        if needs_rebuild {
            let color_image = build_edge_preview_image(
                &image.source,
                &edge_cache.mask,
                preview_size,
                threshold,
                ink_threshold,
            );
            let texture = ctx.load_texture(
                "local_adjust_edge_preview",
                color_image,
                TextureOptions::LINEAR,
            );
            self.edge_preview = Some(EdgePreviewCache { key, texture });
        }
        self.edge_preview.as_ref().map(|cache| &cache.texture)
    }

    fn paint_lasso(&mut self) {
        if self.lasso_points.len() < 3 {
            self.lasso_points.clear();
            return;
        }
        self.push_undo_snapshot();
        let points = std::mem::take(&mut self.lasso_points);
        let add = self.paint_mode;
        let brush_alpha = 1.0;
        if let Some(mask) = self.selected_edit_raster_vector_mask_mut() {
            fill_polygon_alpha(
                &mut mask.alpha,
                mask.width,
                mask.height,
                &points,
                add,
                brush_alpha,
            );
        }
        self.mark_dirty();
    }

    fn commit_shape(&mut self, shape: MaskShape) {
        self.push_undo_snapshot();
        let add = self.paint_mode;
        if let Some(mask) = self.selected_edit_raster_vector_mask_mut() {
            mask.shapes
                .push(shape.with_op(if add { ShapeOp::Add } else { ShapeOp::Subtract }));
            self.selected_shape = Some(mask.shapes.len() - 1);
            self.status = "オブジェクトを追加しました。選択ツールで調整できます。".to_string();
        }
        self.mark_dirty();
    }

    fn pick_color(&mut self, p: Pos2) {
        let Some(image) = &self.image else {
            return;
        };
        let x = p.x.round().clamp(0.0, image.source.width as f32 - 1.0) as usize;
        let y = p.y.round().clamp(0.0, image.source.height as f32 - 1.0) as usize;
        let i = (y * image.source.width + x) * 4;
        let rgb = [
            image.source.pixels[i],
            image.source.pixels[i + 1],
            image.source.pixels[i + 2],
        ];
        let Some(layer) = self.selected_layer_mut() else {
            return;
        };
        layer.mask = match layer.mask {
            LocalMask::ColorRange(mut mask) => {
                mask.target_rgb = rgb;
                mask.initialized = true;
                LocalMask::ColorRange(mask)
            }
            _ => LocalMask::ColorRange(ColorRangeMask {
                initialized: true,
                target_rgb: rgb,
                ..Default::default()
            }),
        };
        self.status = format!("色を取得: #{:02X}{:02X}{:02X}", rgb[0], rgb[1], rgb[2]);
        self.mark_dirty();
    }

    fn toggle_region_at(&mut self, p: Pos2, selected: bool) {
        let x;
        let y;
        {
            let Some(image) = &self.image else {
                return;
            };
            x = p.x.round().clamp(0.0, image.source.width as f32 - 1.0) as usize;
            y = p.y.round().clamp(0.0, image.source.height as f32 - 1.0) as usize;
        }
        let Some(layer) = self.selected_layer_mut() else {
            return;
        };
        let LocalMask::Segmentation(mask) = &mut layer.mask else {
            return;
        };
        if x >= mask.width || y >= mask.height {
            return;
        }
        let label = mask.labels[y * mask.width + x] as usize;
        if label == 0 {
            return;
        }
        if let Some(slot) = mask.selected.get_mut(label) {
            if *slot != selected {
                *slot = selected;
                self.status = if selected {
                    format!("領域 {label} を選択しました。")
                } else {
                    format!("領域 {label} を解除しました。")
                };
                self.mark_dirty();
            }
        }
    }

    fn draw_controls(&mut self, ui: &mut egui::Ui) {
        apply_lab_dark_ui(ui);

        let btn_w = ((PANEL_W - 20.0 - 4.0) / 2.0).max(96.0);
        let btn_size = egui::vec2(btn_w, 24.0);
        ui.label(egui::RichText::new("表示:").color(Color32::from_gray(200)));
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 4.0;
            if panel_toggle_button(ui, "元画像 [Q]", self.show_source, Some(btn_size), false)
                .clicked()
            {
                self.show_source = !self.show_source;
            }
            if panel_toggle_button(ui, "マスク [W]", self.show_mask, Some(btn_size), false)
                .clicked()
            {
                self.show_mask = !self.show_mask;
            }
        });

        ui.separator();
        self.draw_crop_controls(ui, btn_size);

        ui.separator();
        self.draw_layer_list(ui, PANEL_W);
        if self.layers.is_empty() {
            return;
        }

        ui.separator();
        ui.label(egui::RichText::new("選択中レイヤー:").color(Color32::from_gray(200)));
        let mut changed = false;
        if let Some(layer) = self.selected_layer_mut() {
            changed |= ui.text_edit_singleline(&mut layer.name).changed();
            changed |= ui.checkbox(&mut layer.enabled, "有効").changed();
            changed |= ui
                .add(egui::Slider::new(&mut layer.opacity, 0.0..=1.0).text("不透明度"))
                .changed();

            ui.separator();
            changed |= draw_effect_kind_selector(ui, layer);
        }
        if changed {
            self.mark_dirty();
        }

        ui.separator();
        self.draw_manual_tool_selector(ui, btn_size);
        ui.separator();
        if ui
            .add_sized(
                egui::vec2(btn_w * 2.0 + 4.0, 24.0),
                egui::Button::new("表示リセット"),
            )
            .clicked()
        {
            self.view_zoom = 1.0;
            self.view_pan = egui::Vec2::ZERO;
            self.pan_drag_start = None;
        }
        if ui
            .add_sized(
                egui::vec2(btn_w * 2.0 + 4.0, 24.0),
                egui::Button::new("結果保存"),
            )
            .clicked()
        {
            self.save_result();
        }
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 4.0;
            if ui
                .add_sized(btn_size, egui::Button::new("設定保存"))
                .on_hover_text("画像ファイル名に .miv を付けたサイドカーファイルへ保存します。")
                .clicked()
            {
                self.save_settings_sidecar();
            }
            if ui
                .add_sized(btn_size, egui::Button::new("設定読込"))
                .on_hover_text("画像横の .miv サイドカーファイルからレイヤー設定を読み込みます。")
                .clicked()
            {
                self.load_settings_sidecar_for_current_image();
            }
        });
        ui.add(
            egui::Label::new(
                egui::RichText::new(
                    "ホイール/Ctrl+ホイール:ズーム  Space+ドラッグ/中ボタン:パン\n\
                     Q:元画像  W:マスク表示\n\
                     境界筆[A]:境界で止めながら近い色を塗る  Ctrl中は境界表示+通常筆\n\
                     隙間補完[G]:細い未塗り部分を補完\n\
                     Crop編集:黄色枠/ハンドルをドラッグ、保存時に最後段で切り出し\n\
                     Ctrl:境界表示/多角形吸着\n\
                     右クリック/Enterで確定  矢印:移動  [/]:回転\n\
                     Delete:選択削除  Ctrl+Z:戻す  Ctrl+Y/Ctrl+Shift+Z:やり直し",
                )
                .size(10.5)
                .color(Color32::from_gray(180)),
            )
            .wrap(),
        );
    }

    fn draw_crop_controls(&mut self, ui: &mut egui::Ui, btn_size: egui::Vec2) {
        let Some((w, h)) = self.image_dims() else {
            return;
        };
        ui.label(egui::RichText::new("最後段 crop:").color(Color32::from_gray(200)));
        ui.horizontal(|ui| {
            if panel_toggle_button(ui, "有効", self.crop_enabled, Some(btn_size), false).clicked()
            {
                self.crop_enabled = !self.crop_enabled;
                if self.crop_enabled {
                    self.ensure_crop_rect();
                } else {
                    self.crop_drag = None;
                }
            }
            if panel_toggle_button(ui, "表示", self.crop_overlay, Some(btn_size), false).clicked()
            {
                self.crop_overlay = !self.crop_overlay;
            }
        });
        ui.horizontal(|ui| {
            if panel_toggle_button(ui, "編集", self.crop_edit_mode, Some(btn_size), false).clicked()
            {
                self.crop_edit_mode = !self.crop_edit_mode;
                if self.crop_edit_mode {
                    self.crop_enabled = true;
                    self.ensure_crop_rect();
                }
            }
            if ui.add_sized(btn_size, egui::Button::new("全体")).clicked() {
                self.crop_enabled = true;
                self.crop_rect = Some(CropRect::full(w, h));
                self.crop_drag = None;
            }
        });
        if self.crop_enabled {
            let mut crop = self
                .ensure_crop_rect()
                .unwrap_or_else(|| CropRect::full(w, h));
            let mut x = crop.min_x.round() as i32;
            let mut y = crop.min_y.round() as i32;
            let mut cw = crop.width().round() as i32;
            let mut ch = crop.height().round() as i32;
            let mut changed = false;
            ui.horizontal(|ui| {
                changed |= ui
                    .add(egui::DragValue::new(&mut x).range(0..=w.saturating_sub(1) as i32))
                    .changed();
                ui.label("X");
                changed |= ui
                    .add(egui::DragValue::new(&mut y).range(0..=h.saturating_sub(1) as i32))
                    .changed();
                ui.label("Y");
            });
            ui.horizontal(|ui| {
                changed |= ui
                    .add(egui::DragValue::new(&mut cw).range(1..=w.max(1) as i32))
                    .changed();
                ui.label("W");
                changed |= ui
                    .add(egui::DragValue::new(&mut ch).range(1..=h.max(1) as i32))
                    .changed();
                ui.label("H");
            });
            if changed {
                cw = cw.max(1).min(w as i32);
                ch = ch.max(1).min(h as i32);
                x = x.clamp(0, w.saturating_sub(1) as i32);
                y = y.clamp(0, h.saturating_sub(1) as i32);
                if x + cw > w as i32 {
                    x = w as i32 - cw;
                }
                if y + ch > h as i32 {
                    y = h as i32 - ch;
                }
                crop = CropRect {
                    min_x: x.max(0) as f32,
                    min_y: y.max(0) as f32,
                    max_x: (x + cw).max(1) as f32,
                    max_y: (y + ch).max(1) as f32,
                }
                .sanitized(w, h);
                self.crop_rect = Some(crop);
                self.crop_drag = None;
            }
            ui.label(
                egui::RichText::new(
                    "保存時に最終結果を切り出します。上流のマスク座標は変わりません。",
                )
                .size(10.0)
                .color(Color32::from_gray(170)),
            );
        }
    }

    fn draw_manual_tool_selector(&mut self, ui: &mut egui::Ui, btn_size: egui::Vec2) {
        let mask_kind = self
            .selected_layer_ref()
            .map(|layer| MaskKind::from_mask(&layer.mask))
            .unwrap_or(MaskKind::Raster);
        let editing_base_manual = mask_kind == MaskKind::Raster;
        ui.label(
            egui::RichText::new(if editing_base_manual {
                "手動マスク:"
            } else {
                "マスク補正:"
            })
            .color(Color32::from_gray(200)),
        );
        if editing_base_manual {
            self.draw_manual_mask_tool_panel(ui, btn_size);
            return;
        }

        let (has_add, has_subtract) = self
            .selected_layer_ref()
            .map(|layer| {
                (
                    layer.manual_override.add.is_some(),
                    layer.manual_override.subtract.is_some(),
                )
            })
            .unwrap_or((false, false));
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 4.0;
            let add_label = if has_add {
                "追加補正あり"
            } else {
                "追加補正"
            };
            if panel_toggle_button(
                ui,
                add_label,
                self.override_edit_panel == Some(OverrideEditTarget::Add),
                Some(btn_size),
                true,
            )
            .on_hover_text("ベースマスクに手動で足す2値マスクを編集します。")
            .clicked()
            {
                self.toggle_override_edit_panel(OverrideEditTarget::Add);
            }
            let subtract_label = if has_subtract {
                "削除補正あり"
            } else {
                "削除補正"
            };
            if panel_toggle_button(
                ui,
                subtract_label,
                self.override_edit_panel == Some(OverrideEditTarget::Subtract),
                Some(btn_size),
                false,
            )
            .on_hover_text("ベースマスクから手動で除外する2値マスクを編集します。")
            .clicked()
            {
                self.toggle_override_edit_panel(OverrideEditTarget::Subtract);
            }
        });

        if let Some(target) = self.override_edit_panel {
            egui::Frame::new()
                .fill(Color32::from_rgba_unmultiplied(34, 34, 36, 170))
                .stroke(egui::Stroke::new(
                    1.0,
                    Color32::from_rgba_unmultiplied(255, 255, 255, 30),
                ))
                .corner_radius(4.0)
                .inner_margin(6.0)
                .show(ui, |ui| {
                    ui.label(
                        egui::RichText::new(format!("{}を編集中", target.label()))
                            .strong()
                            .color(Color32::WHITE),
                    );
                    ui.label(
                        egui::RichText::new("追加は1.0、削除は0.0でベースマスクを上書きします。")
                            .size(10.0)
                            .color(Color32::from_gray(170)),
                    );
                    self.draw_manual_mask_tool_panel(ui, btn_size);
                });
        } else {
            ui.horizontal(|ui| {
                ui.add_space(2.0);
                ui.label(
                    egui::RichText::new("必要なときだけ追加補正/削除補正を開いて手描きします。")
                        .size(10.0)
                        .color(Color32::from_gray(170)),
                );
            });
        }
    }

    fn draw_manual_mask_tool_panel(&mut self, ui: &mut egui::Ui, btn_size: egui::Vec2) {
        ui.label(egui::RichText::new("描画 / 消去:").color(Color32::from_gray(200)));
        ui.label(
            egui::RichText::new(format!("選択ツール: {}", self.tool.label()))
                .size(11.0)
                .color(Color32::from_gray(180)),
        );
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 4.0;
            if panel_toggle_button(ui, "描画 [D]", self.paint_mode, Some(btn_size), true).clicked()
            {
                self.paint_mode = true;
            }
            if panel_toggle_button(ui, "消去 [F]", !self.paint_mode, Some(btn_size), false)
                .clicked()
            {
                self.paint_mode = false;
            }
        });

        ui.separator();
        ui.label(egui::RichText::new("ビットマップ:").color(Color32::from_gray(200)));
        for row in [
            &[
                (MaskTool::Brush, "筆 [B]"),
                (MaskTool::EdgeBrush, "境界筆 [A]"),
            ][..],
            &[
                (MaskTool::GapFillBrush, "隙間補完 [G]"),
                (MaskTool::Lasso, "囲み [L]"),
            ][..],
            &[(MaskTool::Polygon, "多角形 [P]")][..],
        ] {
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 4.0;
                for &(tool, label) in row {
                    if panel_toggle_button(ui, label, self.tool == tool, Some(btn_size), false)
                        .clicked()
                    {
                        self.switch_tool(tool);
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
                self.apply_bitmap_mask_op(BitmapMaskOp::Expand);
            }
            if ui
                .add_sized(btn_size, egui::Button::new("1px縮小"))
                .clicked()
            {
                self.apply_bitmap_mask_op(BitmapMaskOp::Shrink);
            }
        });
        ui.label(egui::RichText::new("オブジェクト:").color(Color32::from_gray(200)));
        for row in [
            [(MaskTool::Select, "選択 [S]"), (MaskTool::Line, "直線 [I]")],
            [
                (MaskTool::VertLine, "縦線 [V]"),
                (MaskTool::HorizLine, "横線 [H]"),
            ],
            [
                (MaskTool::Rect, "矩形 [R]"),
                (MaskTool::Ellipse, "楕円 [O]"),
            ],
        ] {
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 4.0;
                for (tool, label) in row {
                    if panel_toggle_button(ui, label, self.tool == tool, Some(btn_size), false)
                        .clicked()
                    {
                        self.switch_tool(tool);
                    }
                }
            });
        }
    }

    fn draw_canvas(&mut self, ui: &mut egui::Ui) {
        let mut canvas_size = ui.available_size();
        canvas_size.x = canvas_size.x.max(64.0);
        canvas_size.y = canvas_size.y.max(64.0);
        let (canvas_rect, response) = ui.allocate_exact_size(canvas_size, Sense::click_and_drag());
        ui.painter()
            .rect_filled(canvas_rect, 0.0, Color32::from_rgb(28, 28, 30));

        let Some(image) = &self.image else {
            ui.painter().text(
                canvas_rect.center(),
                egui::Align2::CENTER_CENTER,
                "JPEG / PNG をここにドロップしてください。",
                egui::FontId::proportional(16.0),
                Color32::from_gray(210),
            );
            return;
        };
        let Some(source_texture) = &self.source_texture else {
            return;
        };
        let active_texture_id = if self.show_source {
            source_texture.id()
        } else {
            self.result_texture
                .as_ref()
                .map(|texture| texture.id())
                .unwrap_or_else(|| source_texture.id())
        };
        let img_w = image.source.width;
        let img_h = image.source.height;
        let rect = image_rect_for_canvas(canvas_rect, img_w, img_h, self.view_zoom, self.view_pan);
        let pointer_screen =
            ui.input(|i| i.pointer.interact_pos().or_else(|| i.pointer.hover_pos()));
        let panel_blocks_pointer = pointer_screen
            .map(|p| {
                self.panel_last_rect.map(|r| r.contains(p)).unwrap_or(false)
                    || self
                        .tool_panel_last_rect
                        .map(|r| r.contains(p))
                        .unwrap_or(false)
            })
            .unwrap_or(false);
        let pan_mode = ui.input(|i| {
            i.key_down(Key::Space) || i.pointer.button_down(egui::PointerButton::Middle)
        });
        let view_input_used =
            self.handle_view_navigation(ui, canvas_rect, rect, img_w, img_h, panel_blocks_pointer);

        let uv = Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0));
        ui.painter()
            .image(active_texture_id, rect, uv, Color32::WHITE);
        if self.show_mask {
            self.draw_mask_tile_preview(ui, rect);
        }
        let ctrl_down = ui.input(|i| i.modifiers.ctrl);
        if ctrl_down && self.edge_overlay_enabled() {
            if let Some(edge_texture) = self.ensure_edge_preview_texture(ui.ctx()) {
                ui.painter().image(
                    edge_texture.id(),
                    rect,
                    uv,
                    Color32::from_rgba_unmultiplied(0, 0, 0, 150),
                );
                ui.painter().image(
                    edge_texture.id(),
                    rect,
                    uv,
                    animated_overlay_color(ui.ctx(), 230),
                );
            }
            ui.ctx()
                .request_repaint_after(Duration::from_millis(EDGE_OVERLAY_REPAINT_MS));
        }

        self.draw_shape_overlay(ui, rect, pointer_screen, !pan_mode && !panel_blocks_pointer);
        if !pan_mode && !panel_blocks_pointer {
            self.draw_brush_cursor(ui, rect, pointer_screen);
        }
        let gradient_handle_used = if pan_mode {
            false
        } else {
            self.draw_gradient_handles(ui, rect)
        };
        let crop_used = self.draw_crop_overlay(
            ui,
            rect,
            img_w,
            img_h,
            pointer_screen,
            !panel_blocks_pointer && !pan_mode,
        );
        let secondary_pressed = ui.input(|i| i.pointer.secondary_pressed());

        if !view_input_used
            && !panel_blocks_pointer
            && !crop_used
            && (response.hovered() || response.dragged() || response.clicked() || secondary_pressed)
        {
            let pointer = ui.input(|i| i.pointer.interact_pos());
            if let Some(pointer_screen) = pointer {
                let pos = screen_to_image(rect, img_w, img_h, pointer_screen);
                if !gradient_handle_used {
                    if let Some(pos) = pos {
                        let input_positions =
                            canvas_input_positions(ui, rect, img_w, img_h, pointer_screen);
                        self.handle_canvas_input(ui, rect, response, pos, &input_positions);
                    }
                }
            }
        }
    }

    fn draw_mask_tile_preview(&self, ui: &mut egui::Ui, image_rect: Rect) {
        let Some(cache) = &self.mask_tiles else {
            return;
        };
        let uv = Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0));
        let width = cache.width.max(1) as f32;
        let height = cache.height.max(1) as f32;
        for row in 0..cache.rows {
            for col in 0..cache.cols {
                let idx = row * cache.cols + col;
                let Some(texture) = cache.tiles.get(idx).and_then(|texture| texture.as_ref())
                else {
                    continue;
                };
                let x0 = col * cache.tile_size;
                let y0 = row * cache.tile_size;
                let x1 = (x0 + cache.tile_size).min(cache.width);
                let y1 = (y0 + cache.tile_size).min(cache.height);
                let tile_rect = Rect::from_min_max(
                    Pos2::new(
                        image_rect.left() + image_rect.width() * x0 as f32 / width,
                        image_rect.top() + image_rect.height() * y0 as f32 / height,
                    ),
                    Pos2::new(
                        image_rect.left() + image_rect.width() * x1 as f32 / width,
                        image_rect.top() + image_rect.height() * y1 as f32 / height,
                    ),
                );
                ui.painter()
                    .image(texture.id(), tile_rect, uv, Color32::WHITE);
            }
        }
    }

    fn draw_overlay_panel(&mut self, ctx: &egui::Context, full_rect: Rect) {
        let panel_pos = egui::pos2(
            full_rect.min.x + PANEL_MARGIN_X,
            full_rect.min.y + PANEL_MARGIN_Y,
        );
        let fallback_rect = Rect::from_min_size(
            panel_pos,
            egui::vec2(
                PANEL_W + 20.0,
                (full_rect.height() - PANEL_MARGIN_Y - PANEL_BOTTOM_MARGIN).max(360.0),
            ),
        );
        let sink_rect = self
            .panel_last_rect
            .unwrap_or(fallback_rect)
            .expand2(egui::vec2(4.0, 8.0));

        egui::Area::new(egui::Id::new("local_adjust_lab_panel"))
            .fixed_pos(panel_pos)
            .order(egui::Order::Foreground)
            .interactable(true)
            .show(ctx, |ui| {
                ui.interact(
                    sink_rect,
                    egui::Id::new("local_adjust_lab_panel_sink"),
                    Sense::click_and_drag(),
                );
                let frame_response = egui::Frame::popup(ui.style())
                    .fill(Color32::from_rgba_unmultiplied(20, 20, 20, 230))
                    .stroke(egui::Stroke::new(
                        1.0,
                        Color32::from_rgba_unmultiplied(255, 255, 255, 40),
                    ))
                    .corner_radius(6.0)
                    .show(ui, |ui| {
                        ui.set_min_width(PANEL_W);
                        ui.set_max_width(PANEL_W);
                        apply_lab_dark_ui(ui);

                        ui.horizontal(|ui| {
                            ui.label(
                                egui::RichText::new("部分補正レイヤー")
                                    .size(15.0)
                                    .strong()
                                    .color(Color32::WHITE),
                            );
                            ui.with_layout(
                                egui::Layout::right_to_left(egui::Align::Center),
                                |ui| {
                                    ui.label(
                                        egui::RichText::new(format!(
                                            "{:.0}%",
                                            self.view_zoom * 100.0
                                        ))
                                        .size(11.0)
                                        .color(Color32::from_gray(180)),
                                    );
                                },
                            );
                        });
                        ui.horizontal(|ui| {
                            ui.add_sized(
                                egui::vec2(PANEL_W - 112.0, 18.0),
                                egui::Label::new(
                                    egui::RichText::new(&self.status)
                                        .size(11.0)
                                        .color(Color32::from_gray(190)),
                                )
                                .wrap(),
                            );
                            let (indicator, color) = self.preview_indicator(ctx);
                            ui.with_layout(
                                egui::Layout::right_to_left(egui::Align::Center),
                                |ui| {
                                    ui.label(
                                        egui::RichText::new(format!("● {indicator}"))
                                            .size(11.0)
                                            .strong()
                                            .color(color),
                                    );
                                },
                            );
                        });
                        ui.separator();

                        let body_height =
                            (full_rect.max.y - ui.cursor().top() - PANEL_BOTTOM_MARGIN)
                                .max(PANEL_MIN_BODY_H);
                        ui.allocate_ui_with_layout(
                            egui::vec2(PANEL_W, body_height),
                            egui::Layout::top_down(egui::Align::LEFT),
                            |ui| {
                                ui.set_min_width(PANEL_W);
                                ui.set_max_width(PANEL_W);
                                ui.set_min_height(body_height);
                                egui::ScrollArea::vertical()
                                    .max_height(body_height)
                                    .auto_shrink([false, false])
                                    .show(ui, |ui| {
                                        ui.set_min_width(PANEL_W);
                                        ui.set_max_width(PANEL_W);
                                        self.draw_controls(ui);
                                    });
                            },
                        );
                    });
                self.panel_last_rect = Some(frame_response.response.rect);
            });
    }

    fn draw_tool_panel(&mut self, ctx: &egui::Context, full_rect: Rect) {
        if self.image.is_none() {
            self.tool_panel_last_rect = None;
            return;
        }

        let x =
            (full_rect.max.x - TOOL_PANEL_W - PANEL_MARGIN_X).max(full_rect.min.x + PANEL_MARGIN_X);
        let panel_pos = egui::pos2(x, full_rect.min.y + PANEL_MARGIN_Y);
        let panel_height = (full_rect.max.y - panel_pos.y - PANEL_BOTTOM_MARGIN).max(160.0);
        let body_height = (panel_height - 14.0).max(120.0);
        let fallback_rect = Rect::from_min_size(panel_pos, egui::vec2(TOOL_PANEL_W, panel_height));
        let sink_rect = self
            .tool_panel_last_rect
            .unwrap_or(fallback_rect)
            .expand2(egui::vec2(4.0, 8.0));

        egui::Area::new(egui::Id::new("local_adjust_lab_tool_panel"))
            .fixed_pos(panel_pos)
            .order(egui::Order::Foreground)
            .interactable(true)
            .show(ctx, |ui| {
                ui.interact(
                    sink_rect,
                    egui::Id::new("local_adjust_lab_tool_panel_sink"),
                    Sense::click_and_drag(),
                );
                let frame_response = egui::Frame::popup(ui.style())
                    .fill(Color32::from_rgba_unmultiplied(20, 20, 20, 230))
                    .stroke(egui::Stroke::new(
                        1.0,
                        Color32::from_rgba_unmultiplied(255, 255, 255, 40),
                    ))
                    .corner_radius(6.0)
                    .show(ui, |ui| {
                        ui.set_min_width(TOOL_PANEL_W);
                        ui.set_max_width(TOOL_PANEL_W);
                        ui.set_min_height(body_height);
                        ui.set_max_height(body_height);
                        apply_lab_dark_ui(ui);
                        egui::ScrollArea::vertical()
                            .max_height(body_height)
                            .auto_shrink([false, false])
                            .show(ui, |ui| {
                                ui.set_min_width(TOOL_PANEL_W);
                                ui.set_max_width(TOOL_PANEL_W);
                                self.draw_mask_panel(ui);
                            });
                    });
                self.tool_panel_last_rect = Some(frame_response.response.rect);
            });
    }

    fn draw_layer_list(&mut self, ui: &mut egui::Ui, panel_w: f32) {
        let btn_w = ((panel_w - 20.0 - 4.0) / 2.0).max(96.0);
        let btn_size = egui::vec2(btn_w, 24.0);
        ui.label(
            egui::RichText::new("レイヤー")
                .size(14.0)
                .strong()
                .color(Color32::WHITE),
        );
        if ui
            .add_sized(
                egui::vec2(btn_w * 2.0 + 4.0, 24.0),
                egui::Button::new("+ 部分補正レイヤー"),
            )
            .clicked()
        {
            self.add_layer_dialog_open = true;
        }
        if ui
            .checkbox(
                &mut self.preview_to_selected_layer,
                "選択レイヤーまでプレビュー",
            )
            .changed()
        {
            self.mark_dirty();
        }
        if self.preview_to_selected_layer && !self.layers.is_empty() {
            ui.label(
                egui::RichText::new(format!(
                    "表示中: 1〜{} / {}",
                    self.preview_layer_count(),
                    self.layers.len()
                ))
                .size(10.0)
                .color(Color32::from_gray(170)),
            );
        }

        let mut clicked_layer = None;
        let mut layer_enabled_changed = false;
        let image_ref = self.image.as_ref().map(|image| image.source.as_ref());
        for idx in 0..self.layers.len() {
            let selected = idx == self.selected_layer;
            let frame_response = egui::Frame::new()
                .fill(if selected {
                    Color32::from_rgba_unmultiplied(58, 96, 150, 170)
                } else {
                    Color32::from_rgba_unmultiplied(52, 52, 54, 120)
                })
                .stroke(egui::Stroke::new(
                    1.0,
                    if selected {
                        Color32::from_rgba_unmultiplied(150, 195, 255, 130)
                    } else {
                        Color32::from_rgba_unmultiplied(255, 255, 255, 24)
                    },
                ))
                .corner_radius(4.0)
                .inner_margin(6.0)
                .show(ui, |ui| {
                    ui.set_min_width(panel_w - 12.0);
                    let layer = &mut self.layers[idx];
                    let mut row_clicked = false;
                    ui.horizontal(|ui| {
                        ui.spacing_mut().item_spacing.x = 8.0;
                        if ui.checkbox(&mut layer.enabled, "").changed() {
                            layer_enabled_changed = true;
                        }
                        if draw_layer_mask_thumbnail(ui, layer, image_ref, selected).clicked() {
                            row_clicked = true;
                        }
                        ui.vertical(|ui| {
                            ui.spacing_mut().item_spacing.y = 2.0;
                            let text_color = if layer.enabled {
                                Color32::WHITE
                            } else {
                                Color32::from_gray(145)
                            };
                            let mask_line = MaskKind::from_mask(&layer.mask).label();
                            let effect_line = effect_summary(&layer.effect);
                            ui.label(egui::RichText::new(mask_line).strong().color(text_color));
                            ui.label(egui::RichText::new(effect_line).size(11.0).color(
                                if layer.enabled {
                                    Color32::from_gray(205)
                                } else {
                                    Color32::from_gray(125)
                                },
                            ));
                            if !layer.enabled {
                                ui.label(
                                    egui::RichText::new("OFF")
                                        .size(10.0)
                                        .color(Color32::from_gray(150)),
                                );
                            }
                        });
                        let spacer_w = ui.available_width().max(0.0);
                        if spacer_w > 4.0 {
                            let (_, spacer_response) =
                                ui.allocate_exact_size(egui::vec2(spacer_w, 48.0), Sense::click());
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
        if layer_enabled_changed {
            self.mark_dirty();
        }
        if let Some(idx) = clicked_layer {
            let old_selected = self.selected_layer;
            self.selected_layer = idx;
            self.mask_dirty = true;
            if old_selected != idx {
                self.reset_override_edit_state_for_selected_layer();
            }
            if self.preview_to_selected_layer && old_selected != idx {
                self.mark_dirty();
            }
        }

        ui.horizontal(|ui| {
            if ui
                .add_sized(egui::vec2(58.0, 22.0), egui::Button::new("↑"))
                .clicked()
            {
                self.move_selected_layer(-1);
            }
            if ui
                .add_sized(egui::vec2(58.0, 22.0), egui::Button::new("↓"))
                .clicked()
            {
                self.move_selected_layer(1);
            }
        });
        ui.horizontal(|ui| {
            if ui.add_sized(btn_size, egui::Button::new("複製")).clicked() {
                self.duplicate_layer();
            }
            if ui
                .add_sized(
                    btn_size,
                    egui::Button::new("削除").fill(Color32::from_rgb(120, 50, 50)),
                )
                .clicked()
            {
                self.remove_selected_layer();
            }
        });
    }

    fn draw_mask_actions(&mut self, ui: &mut egui::Ui) {
        ui.label(
            egui::RichText::new("マスク操作")
                .size(14.0)
                .strong()
                .color(Color32::WHITE),
        );
        let has_override = self
            .selected_layer_ref()
            .map(|layer| !layer.manual_override.is_empty())
            .unwrap_or(false);
        if ui
            .add_enabled(has_override, egui::Button::new("手動補正をクリア"))
            .on_hover_text("ベースマスクは残し、追加補正/削除補正だけを空にします。")
            .clicked()
        {
            self.clear_selected_manual_override();
        }
        ui.label(
            egui::RichText::new(
                "グラデーションや被写体マットを保ったまま、追加/削除の2値マスクで部分的に上書きできます。",
            )
            .size(10.0)
            .color(Color32::from_gray(170)),
        );

        let copy_options: Vec<(usize, String)> = self
            .layers
            .iter()
            .enumerate()
            .filter(|(idx, _)| *idx != self.selected_layer)
            .map(|(idx, layer)| {
                (
                    idx,
                    format!(
                        "{}: {} / {}",
                        layer.name,
                        MaskKind::from_mask(&layer.mask).label(),
                        effect_summary(&layer.effect)
                    ),
                )
            })
            .collect();
        if copy_options.is_empty() {
            ui.label(
                egui::RichText::new("コピー元にできる別レイヤーはまだありません。")
                    .size(10.0)
                    .color(Color32::from_gray(170)),
            );
            return;
        }
        if !copy_options
            .iter()
            .any(|(idx, _)| *idx == self.mask_copy_source)
        {
            self.mask_copy_source = copy_options[0].0;
        }
        let selected_text = copy_options
            .iter()
            .find(|(idx, _)| *idx == self.mask_copy_source)
            .map(|(_, label)| label.as_str())
            .unwrap_or("コピー元レイヤー");
        lab_combo_box(ui, "mask_copy_source_layer", selected_text, |ui| {
            for (idx, label) in &copy_options {
                ui.selectable_value(&mut self.mask_copy_source, *idx, label);
            }
        });
        if ui
            .button("コピー元のマスクを適用")
            .on_hover_text("マスク種類、マスク本体、追加/削除補正、反転、拡張/縮小、ぼかし境界だけをコピーします。加工内容と不透明度は現在のレイヤーのままです。")
            .clicked()
        {
            self.copy_mask_from_layer(self.mask_copy_source);
        }
    }

    fn draw_mask_panel(&mut self, ui: &mut egui::Ui) {
        if self.layers.is_empty() {
            ui.label(
                egui::RichText::new("左側のレイヤーパネルからレイヤーを追加してください。")
                    .size(11.0)
                    .color(Color32::from_gray(180)),
            );
            return;
        }
        let selected_mask_kind = self
            .selected_layer_ref()
            .map(|layer| MaskKind::from_mask(&layer.mask));
        let manual_edit_controls_visible =
            selected_mask_kind == Some(MaskKind::Raster) || self.override_edit_panel.is_some();
        if manual_edit_controls_visible {
            self.draw_tool_controls(ui);
        } else {
            ui.label(
                egui::RichText::new("追加補正/削除補正を開くと、手描きツール設定を表示します。")
                    .size(11.0)
                    .color(Color32::from_gray(180)),
            );
        }
        if selected_mask_kind != Some(MaskKind::Raster) && manual_edit_controls_visible {
            let help = if self.override_edit_panel.is_some() {
                "補正パネルが開いている間は、筆/図形ツールで追加補正または削除補正を編集します。ベースマスクを調整する場合は補正パネルを閉じます。"
            } else {
                match selected_mask_kind {
                    Some(MaskKind::LinearGradient) => {
                        "選択ツールでは画像上のドラッグで生成/調整します。筆などに切り替えると追加/削除補正を描けます。"
                    }
                    Some(MaskKind::RadialGradient) => {
                        "選択ツールでは画像上のドラッグで生成/調整します。筆などに切り替えると追加/削除補正を描けます。"
                    }
                    Some(MaskKind::ColorRange) => {
                        "選択ツールでは画像上クリックでスポイト指定します。筆などに切り替えると追加/削除補正を描けます。"
                    }
                    Some(MaskKind::LumaRange) => {
                        "輝度範囲はスライダーで調整します。筆などで追加/削除補正を描けます。"
                    }
                    Some(MaskKind::Full) => "全体マスクに対して削除補正などを描けます。",
                    Some(MaskKind::Subject) => {
                        "被写体/背景マットを保ったまま、筆などで追加/削除補正を描けます。"
                    }
                    Some(MaskKind::Segmentation) => {
                        "選択ツールでは領域候補をクリック/ドラッグでON/OFFします。筆などでは追加/削除補正を描けます。"
                    }
                    None | Some(MaskKind::Raster) => "",
                }
            };
            ui.label(
                egui::RichText::new(help)
                    .size(11.0)
                    .color(Color32::from_gray(180)),
            );
        }

        ui.separator();
        self.draw_mask_actions(ui);
        ui.separator();
        ui.label(
            egui::RichText::new("マスク設定")
                .size(14.0)
                .strong()
                .color(Color32::WHITE),
        );
        let dims = self.image_dims().unwrap_or((1, 1));
        let mut changed = false;
        if let Some(layer) = self.selected_layer_mut() {
            changed |= ui
                .checkbox(&mut layer.mask_inverted, "マスク反転")
                .changed();
            changed |= ui
                .add(egui::Slider::new(&mut layer.mask_expand_px, -32.0..=32.0).text("拡張/縮小"))
                .changed();
            changed |= ui
                .add(egui::Slider::new(&mut layer.mask_feather_px, 0.0..=64.0).text("ぼかし境界"))
                .changed();
            ui.separator();
            changed |= draw_mask_controls(ui, layer, dims);
        }
        if changed {
            self.mark_dirty();
        }
        if selected_mask_kind == Some(MaskKind::Subject) {
            ui.separator();
            self.draw_subject_controls(ui);
        }
        if selected_mask_kind == Some(MaskKind::Segmentation) {
            ui.separator();
            self.draw_region_controls(ui);
        }

        ui.separator();
        if let Some(layer) = self.selected_layer_mut() {
            if draw_effect_params(ui, layer) {
                self.mark_dirty();
            }
        }
    }

    fn draw_subject_controls(&mut self, ui: &mut egui::Ui) {
        ui.label(
            egui::RichText::new("被写体選択")
                .size(14.0)
                .strong()
                .color(Color32::WHITE),
        );
        let pending = self.segmentation_pending.is_some();
        let model_path = segmentation_model_path();
        if model_path.is_file() {
            ui.label(
                egui::RichText::new("U²-Netp: 利用可能")
                    .size(11.0)
                    .color(Color32::from_rgb(120, 220, 150)),
            );
        } else {
            ui.label(
                egui::RichText::new(format!("モデル未配置: {}", model_path.display()))
                    .size(10.0)
                    .color(Color32::from_rgb(255, 170, 100)),
            );
        }
        if ui
            .add_enabled(
                !pending,
                egui::Button::new(if pending {
                    "被写体マスク生成中..."
                } else {
                    "被写体マスク生成"
                }),
            )
            .clicked()
        {
            self.start_subject_segmentation(ui.ctx());
        }
        ui.horizontal(|ui| {
            if ui.button("被写体を選択").clicked()
                && let Some(layer) = self.selected_layer_mut()
            {
                layer.mask_inverted = false;
                self.mark_dirty();
            }
            if ui.button("背景を選択").clicked()
                && let Some(layer) = self.selected_layer_mut()
            {
                layer.mask_inverted = true;
                self.mark_dirty();
            }
        });
        ui.label(
            egui::RichText::new(
                "生成結果は前景マットです。背景を加工したい場合は「背景を選択」またはマスク反転を使います。",
            )
            .size(10.0)
            .color(Color32::from_gray(170)),
        );
    }

    fn draw_region_controls(&mut self, ui: &mut egui::Ui) {
        ui.label(
            egui::RichText::new("領域分割")
                .size(14.0)
                .strong()
                .color(Color32::WHITE),
        );
        let pending = self.segmentation_pending.is_some();
        let subject_available = self.subject_mask_candidate().is_some();
        let mut changed = false;
        changed |= ui
            .add(egui::Slider::new(&mut self.region_color_tolerance, 4.0..=120.0).text("色差許容"))
            .on_hover_text("大きいほど近い色が同じ領域にまとまり、小さいほど細かく分かれます。")
            .changed();
        let mut min_area = self.region_min_area as i32;
        if ui
            .add(egui::Slider::new(&mut min_area, 1..=2048).text("最小領域"))
            .on_hover_text(
                "この面積より小さい候補を捨てます。大きいほど細かいノイズ領域が減ります。",
            )
            .changed()
        {
            self.region_min_area = min_area.max(1) as usize;
            changed = true;
        }
        if changed {
            self.status = "領域分割の設定を変更しました。再生成してください。".to_string();
        }
        if ui
            .add_enabled(!pending, egui::Button::new("画像全体を領域分割"))
            .clicked()
        {
            self.start_region_segmentation(ui.ctx(), false);
        }
        if ui
            .add_enabled(
                !pending && subject_available,
                egui::Button::new("被写体内を領域分割"),
            )
            .clicked()
        {
            self.start_region_segmentation(ui.ctx(), true);
        }
        if !subject_available {
            ui.label(
                egui::RichText::new("被写体選択レイヤーを生成すると、被写体内だけを分割できます。")
                    .size(10.0)
                    .color(Color32::from_gray(170)),
            );
        }
        ui.label(
            egui::RichText::new(
                "色差許容: 小=細かく分割 / 大=似た色を結合。最小領域: 小=細部も残す / 大=細かい破片を捨てる。",
            )
            .size(10.0)
            .color(Color32::from_gray(170)),
        );
        ui.label(
            egui::RichText::new(
                "境界線などで未所属になった細い隙間は、生成時に近い領域へ自動で割り当てます。",
            )
            .size(10.0)
            .color(Color32::from_gray(170)),
        );

        let mut mark_dirty = false;
        ui.horizontal(|ui| {
            if panel_toggle_button(ui, "追加", self.paint_mode, None, true).clicked() {
                self.paint_mode = true;
            }
            if panel_toggle_button(ui, "解除", !self.paint_mode, None, false).clicked() {
                self.paint_mode = false;
            }
        });
        if let Some(LocalMask::Segmentation(mask)) =
            self.selected_layer_mut().map(|layer| &mut layer.mask)
        {
            ui.horizontal(|ui| {
                if ui.button("全選択").clicked() {
                    for selected in mask.selected.iter_mut().skip(1) {
                        *selected = true;
                    }
                    mark_dirty = true;
                }
                if ui.button("全解除").clicked() {
                    for selected in mask.selected.iter_mut().skip(1) {
                        *selected = false;
                    }
                    mark_dirty = true;
                }
                if ui.button("選択反転").clicked() {
                    for selected in mask.selected.iter_mut().skip(1) {
                        *selected = !*selected;
                    }
                    mark_dirty = true;
                }
            });
            let selected_count = mask.selected.iter().skip(1).filter(|&&v| v).count();
            ui.label(
                egui::RichText::new(format!(
                    "領域: {} / 選択: {}",
                    mask.label_count(),
                    selected_count
                ))
                .size(11.0)
                .color(Color32::from_gray(190)),
            );
        }
        if mark_dirty {
            self.mark_dirty();
        }
        ui.label(
            egui::RichText::new(
                "画像上の色分け領域をクリックまたはドラッグして、追加/解除します。選択中の領域はピンクと明るい境界で表示します。",
            )
            .size(10.0)
            .color(Color32::from_gray(170)),
        );
    }

    fn draw_tool_controls(&mut self, ui: &mut egui::Ui) {
        ui.label(
            egui::RichText::new("ツール設定")
                .size(14.0)
                .strong()
                .color(Color32::WHITE),
        );
        ui.label(
            egui::RichText::new(self.tool.label())
                .size(11.0)
                .color(Color32::from_gray(180)),
        );
        ui.separator();

        match self.tool {
            MaskTool::Brush | MaskTool::EdgeBrush | MaskTool::GapFillBrush => {
                ui.add(egui::Slider::new(&mut self.brush_radius, 1.0..=160.0).text("筆サイズ"));
            }
            MaskTool::Line | MaskTool::VertLine | MaskTool::HorizLine => {
                ui.add(egui::Slider::new(&mut self.line_width, 1.0..=160.0).text("線幅"));
            }
            _ => {}
        }

        match self.tool {
            MaskTool::EdgeBrush => {
                ui.add(
                    egui::Slider::new(&mut self.boundary_edge_threshold, 0.0..=120.0)
                        .text("境界しきい値"),
                );
                ui.add(
                    egui::Slider::new(&mut self.boundary_ink_threshold, 0.0..=120.0)
                        .text("線内部しきい値"),
                );
                ui.add(
                    egui::Slider::new(&mut self.boundary_gap_px, 0.0..=4.0)
                        .text("境界ギャップ補完"),
                );
                ui.add(
                    egui::Slider::new(&mut self.edge_brush_tolerance, 0.0..=160.0).text("色差許容"),
                );
                ui.checkbox(&mut self.edge_brush_include_boundary, "境界線を含む");
                ui.label(
                    egui::RichText::new(
                        "開始点から連結している近い色だけを、境界で止めて塗ります。Ctrl中は境界を表示しながら通常筆です。",
                    )
                    .size(10.0)
                    .color(Color32::from_gray(170)),
                );
                ui.label(
                    egui::RichText::new(
                        "境界線を含む場合は、塗った領域に接する検出線だけを少し追加します。",
                    )
                    .size(10.0)
                    .color(Color32::from_gray(170)),
                );
            }
            MaskTool::GapFillBrush => {
                ui.add(egui::Slider::new(&mut self.gap_fill_distance, 1.0..=48.0).text("隙間幅"));
                ui.label(
                    egui::RichText::new(
                        "左右または上下のマスクに挟まれた細い未塗り部分を補完します。",
                    )
                    .size(10.0)
                    .color(Color32::from_gray(170)),
                );
            }
            MaskTool::Polygon => {
                ui.add(
                    egui::Slider::new(&mut self.boundary_edge_threshold, 0.0..=120.0)
                        .text("境界しきい値"),
                );
                ui.add(
                    egui::Slider::new(&mut self.boundary_ink_threshold, 0.0..=120.0)
                        .text("線内部しきい値"),
                );
                ui.add(
                    egui::Slider::new(&mut self.boundary_gap_px, 0.0..=4.0)
                        .text("境界ギャップ補完"),
                );
                ui.add(
                    egui::Slider::new(&mut self.edge_snap_radius, 2.0..=64.0)
                        .text("吸着半径(画面px)"),
                );
                ui.label(
                    egui::RichText::new("Ctrl中は候補点が近くの境界へ吸着します。")
                        .size(10.0)
                        .color(Color32::from_gray(170)),
                );
            }
            MaskTool::Brush => {
                ui.label(
                    egui::RichText::new("通常の円形ブラシです。")
                        .size(10.0)
                        .color(Color32::from_gray(170)),
                );
            }
            MaskTool::Line | MaskTool::VertLine | MaskTool::HorizLine => {
                ui.label(
                    egui::RichText::new("線幅は直線系ツールで共通です。")
                        .size(10.0)
                        .color(Color32::from_gray(170)),
                );
            }
            _ => {
                ui.label(
                    egui::RichText::new("このツールに追加パラメータはありません。")
                        .size(10.0)
                        .color(Color32::from_gray(170)),
                );
            }
        }
    }

    fn draw_add_layer_dialog(&mut self, ctx: &egui::Context) {
        if !self.add_layer_dialog_open {
            return;
        }
        let mut open = self.add_layer_dialog_open;
        let mut selected = self.add_layer_mask_kind;
        let mut create_requested = false;
        let mut cancel_requested = false;
        let dialog_frame = egui::Frame::window(ctx.style().as_ref())
            .fill(Color32::from_rgba_unmultiplied(24, 24, 26, 245))
            .stroke(egui::Stroke::new(
                1.0,
                Color32::from_rgba_unmultiplied(255, 255, 255, 70),
            ));
        egui::Window::new("部分補正レイヤーを追加")
            .order(egui::Order::Debug)
            .frame(dialog_frame)
            .anchor(egui::Align2::CENTER_CENTER, egui::Vec2::ZERO)
            .collapsible(false)
            .resizable(false)
            .default_width(360.0)
            .open(&mut open)
            .show(ctx, |ui| {
                apply_lab_dark_ui(ui);
                ui.label("マスク種類を選んで追加します。追加後は種類を変えず、内容を編集します。");
                ui.separator();
                for kind in [
                    MaskKind::Raster,
                    MaskKind::LinearGradient,
                    MaskKind::RadialGradient,
                    MaskKind::LumaRange,
                    MaskKind::ColorRange,
                    MaskKind::Full,
                    MaskKind::Subject,
                    MaskKind::Segmentation,
                ] {
                    ui.radio_value(&mut selected, kind, kind.label());
                    if selected == kind {
                        ui.label(
                            egui::RichText::new(kind.description())
                                .size(11.0)
                                .color(Color32::from_gray(180)),
                        );
                    }
                }
                ui.separator();
                ui.horizontal(|ui| {
                    if ui.button("追加").clicked() {
                        create_requested = true;
                    }
                    if ui.button("キャンセル").clicked() {
                        cancel_requested = true;
                    }
                });
            });
        self.add_layer_mask_kind = selected;
        self.add_layer_dialog_open = open && !cancel_requested;
        if create_requested {
            self.add_layer_with_mask(selected);
        }
    }

    fn handle_view_navigation(
        &mut self,
        ui: &mut egui::Ui,
        canvas_rect: Rect,
        image_rect: Rect,
        img_w: usize,
        img_h: usize,
        panel_blocks_pointer: bool,
    ) -> bool {
        if panel_blocks_pointer {
            self.pan_drag_start = None;
            return false;
        }

        let (hover_pos, interact_pos, scroll_delta, space_held, middle_down, primary_down) = ui
            .input(|i| {
                (
                    i.pointer.hover_pos(),
                    i.pointer.interact_pos(),
                    i.raw_scroll_delta,
                    i.key_down(Key::Space),
                    i.pointer.button_down(egui::PointerButton::Middle),
                    i.pointer.primary_down(),
                )
            });

        if let Some(pos) = hover_pos
            && canvas_rect.contains(pos)
            && scroll_delta.length_sq() > 0.0
            && scroll_delta.y.abs() > 0.0
        {
            let old_zoom = self.view_zoom;
            let factor = (scroll_delta.y / 240.0).exp();
            let new_zoom = (old_zoom * factor).clamp(ZOOM_MIN, ZOOM_MAX);
            self.zoom_around(canvas_rect, img_w, img_h, pos, new_zoom);
        }

        let pan_requested = middle_down || (space_held && primary_down);
        if pan_requested
            && let Some(pos) = interact_pos
            && (canvas_rect.contains(pos) || image_rect.contains(pos))
        {
            if self.pan_drag_start.is_none() {
                self.pan_drag_start = Some((pos, self.view_pan));
            }
            if let Some((start_pos, start_pan)) = self.pan_drag_start {
                self.view_pan = start_pan + (pos - start_pos);
            }
            return true;
        }

        if !pan_requested {
            self.pan_drag_start = None;
        }
        false
    }

    fn zoom_around(
        &mut self,
        canvas_rect: Rect,
        img_w: usize,
        img_h: usize,
        pointer: Pos2,
        new_zoom: f32,
    ) {
        let old_zoom = self.view_zoom;
        if (new_zoom - old_zoom).abs() < f32::EPSILON {
            return;
        }
        let fit_scale = fit_scale_for_canvas(canvas_rect, img_w, img_h);
        let old_scale = fit_scale * old_zoom.max(ZOOM_MIN);
        let new_scale = fit_scale * new_zoom.max(ZOOM_MIN);
        let center = canvas_rect.center();
        let img_center = egui::vec2(img_w as f32 * 0.5, img_h as f32 * 0.5);
        let pointer_v = pointer.to_vec2();
        let image_point = (pointer_v - center.to_vec2() - self.view_pan) / old_scale + img_center;
        self.view_pan = pointer_v - center.to_vec2() - (image_point - img_center) * new_scale;
        self.view_zoom = new_zoom;
    }

    fn handle_canvas_input(
        &mut self,
        ui: &mut egui::Ui,
        rect: Rect,
        response: egui::Response,
        pos: Pos2,
        input_positions: &[Pos2],
    ) {
        let primary_down = ui.input(|i| i.pointer.primary_down());
        let primary_pressed = ui.input(|i| i.pointer.primary_pressed());
        let primary_released = ui.input(|i| i.pointer.primary_released());
        let secondary_pressed = ui.input(|i| i.pointer.secondary_pressed());
        let modifiers = ui.input(|i| i.modifiers);
        let dims = self.image_dims();
        let brush_radius = self.brush_radius.max(1.0);
        if self.tool == MaskTool::Select {
            if self.shape_drag.is_some()
                || self.selected_shape.is_some()
                || self.hit_test_shapes([pos.x, pos.y]).is_some()
            {
                self.handle_select_tool(
                    pos,
                    primary_pressed,
                    primary_down,
                    primary_released,
                    modifiers,
                );
                return;
            }
            let override_editing_active = self.override_edit_panel.is_some()
                && self
                    .selected_layer_ref()
                    .map(|layer| MaskKind::from_mask(&layer.mask) != MaskKind::Raster)
                    .unwrap_or(false);
            if override_editing_active {
                return;
            }
            match self.selected_layer_ref().map(|layer| &layer.mask) {
                Some(LocalMask::LinearGradient(_)) => {
                    if primary_down {
                        if response.drag_started() {
                            self.push_undo_snapshot();
                        }
                        self.drag_gradient_line(pos, response.drag_started());
                    }
                    return;
                }
                Some(LocalMask::RadialGradient(mask)) => {
                    let initialized = mask.initialized;
                    if secondary_pressed && initialized {
                        self.push_undo_snapshot();
                        if let Some(layer) = self.selected_layer_mut() {
                            layer.mask = LocalMask::RadialGradient(RadialGradientMask::default());
                        }
                        self.radial_gradient_drag_active = false;
                        self.status = "円形グラデーションをクリアしました。".to_string();
                        self.mark_dirty();
                        return;
                    }
                    if primary_down && (!initialized || self.radial_gradient_drag_active) {
                        let started = !self.radial_gradient_drag_active;
                        if started {
                            self.push_undo_snapshot();
                            self.radial_gradient_drag_active = true;
                        }
                        self.drag_gradient_line(pos, started);
                    }
                    return;
                }
                Some(LocalMask::ColorRange(_)) => {
                    if response.clicked() {
                        self.push_undo_snapshot();
                        self.pick_color(pos);
                    }
                    return;
                }
                Some(LocalMask::Segmentation(_)) => {
                    if primary_down {
                        if primary_pressed || response.drag_started() || response.clicked() {
                            self.push_undo_snapshot();
                        }
                        self.toggle_region_at(pos, self.paint_mode);
                    }
                    return;
                }
                Some(LocalMask::Full | LocalMask::LumaRange(_) | LocalMask::Subject(_)) => {
                    return;
                }
                _ => {}
            }
        }
        match self.tool {
            MaskTool::Brush => {
                if primary_down {
                    if primary_pressed || response.drag_started() || response.clicked() {
                        self.begin_brush_stroke();
                    }
                    let brush_start = Instant::now();
                    let positions = brush_input_positions(input_positions, pos);
                    let input_count = positions.len();
                    let mut changed = false;
                    let mut stamps = 0;
                    let mut changed_stamps = 0;
                    let mut max_input_gap: f32 = 0.0;
                    let mut dirty_tiles = BTreeSet::new();
                    for &input_pos in &positions {
                        max_input_gap = max_input_gap.max(
                            self.last_paint_pos
                                .map(|last| last.distance(input_pos))
                                .unwrap_or(0.0),
                        );
                        let stamp_points = self.brush_stroke_points(input_pos);
                        stamps += stamp_points.len();
                        for stamp_pos in stamp_points {
                            let stamp_changed = self.paint_raster_stamp(stamp_pos, self.paint_mode);
                            changed |= stamp_changed;
                            changed_stamps += usize::from(stamp_changed);
                            if stamp_changed
                                && let Some((w, h)) = dims
                                && let Some(rect) =
                                    brush_dirty_rect_for_point(stamp_pos, brush_radius, w, h)
                            {
                                insert_dirty_tiles_for_rect(&mut dirty_tiles, rect, w, h);
                            }
                        }
                    }
                    self.record_brush_perf(
                        brush_start.elapsed(),
                        input_count,
                        stamps,
                        changed_stamps,
                        max_input_gap,
                    );
                    if changed {
                        if !dirty_tiles.is_empty() {
                            self.mark_dirty_tiles(dirty_tiles);
                        } else {
                            self.mark_dirty();
                        }
                    }
                    ui.ctx().request_repaint();
                }
                if primary_released {
                    self.reset_brush_stroke();
                }
            }
            MaskTool::EdgeBrush => {
                if primary_down {
                    if primary_pressed || response.drag_started() || response.clicked() {
                        self.begin_brush_stroke();
                    }
                    let brush_start = Instant::now();
                    let positions = brush_input_positions(input_positions, pos);
                    let input_count = positions.len();
                    let mut changed = false;
                    let mut stamps = 0;
                    let mut changed_stamps = 0;
                    let mut max_input_gap: f32 = 0.0;
                    let mut dirty_tiles = BTreeSet::new();
                    if modifiers.ctrl {
                        self.edge_brush_seed = None;
                        for &input_pos in &positions {
                            max_input_gap = max_input_gap.max(
                                self.last_paint_pos
                                    .map(|last| last.distance(input_pos))
                                    .unwrap_or(0.0),
                            );
                            let stamp_points = self.brush_stroke_points(input_pos);
                            stamps += stamp_points.len();
                            for stamp_pos in stamp_points {
                                let stamp_changed =
                                    self.paint_raster_stamp(stamp_pos, self.paint_mode);
                                changed |= stamp_changed;
                                changed_stamps += usize::from(stamp_changed);
                                if stamp_changed
                                    && let Some((w, h)) = dims
                                    && let Some(rect) =
                                        brush_dirty_rect_for_point(stamp_pos, brush_radius, w, h)
                                {
                                    insert_dirty_tiles_for_rect(&mut dirty_tiles, rect, w, h);
                                }
                            }
                        }
                    } else {
                        for &input_pos in &positions {
                            max_input_gap = max_input_gap.max(
                                self.last_paint_pos
                                    .map(|last| last.distance(input_pos))
                                    .unwrap_or(0.0),
                            );
                            let stamp_points = self.brush_stroke_points(input_pos);
                            stamps += stamp_points.len();
                            for stamp_pos in stamp_points {
                                if self.edge_brush_seed.is_none() {
                                    self.edge_brush_seed = self.source_rgb_at(stamp_pos);
                                }
                                let stamp_changed =
                                    self.paint_edge_brush_stamp(stamp_pos, self.paint_mode);
                                changed |= stamp_changed;
                                changed_stamps += usize::from(stamp_changed);
                                if stamp_changed
                                    && let Some((w, h)) = dims
                                    && let Some(rect) =
                                        brush_dirty_rect_for_point(stamp_pos, brush_radius, w, h)
                                {
                                    insert_dirty_tiles_for_rect(&mut dirty_tiles, rect, w, h);
                                }
                            }
                        }
                    }
                    self.record_brush_perf(
                        brush_start.elapsed(),
                        input_count,
                        stamps,
                        changed_stamps,
                        max_input_gap,
                    );
                    if changed {
                        if !dirty_tiles.is_empty() {
                            self.mark_dirty_tiles(dirty_tiles);
                        } else {
                            self.mark_dirty();
                        }
                    }
                    ui.ctx().request_repaint();
                }
                if primary_released {
                    self.reset_brush_stroke();
                }
            }
            MaskTool::GapFillBrush => {
                if primary_down {
                    if primary_pressed || response.drag_started() || response.clicked() {
                        self.begin_brush_stroke();
                    }
                    let brush_start = Instant::now();
                    let positions = brush_input_positions(input_positions, pos);
                    let input_count = positions.len();
                    let mut changed = false;
                    let mut stamps = 0;
                    let mut changed_stamps = 0;
                    let mut max_input_gap: f32 = 0.0;
                    let mut dirty_tiles = BTreeSet::new();
                    for &input_pos in &positions {
                        max_input_gap = max_input_gap.max(
                            self.last_paint_pos
                                .map(|last| last.distance(input_pos))
                                .unwrap_or(0.0),
                        );
                        let stamp_points = self.brush_stroke_points(input_pos);
                        stamps += stamp_points.len();
                        for stamp_pos in stamp_points {
                            let stamp_changed =
                                self.paint_gap_fill_brush_stamp(stamp_pos, self.paint_mode);
                            changed |= stamp_changed;
                            changed_stamps += usize::from(stamp_changed);
                            if stamp_changed
                                && let Some((w, h)) = dims
                                && let Some(rect) =
                                    brush_dirty_rect_for_point(stamp_pos, brush_radius, w, h)
                            {
                                insert_dirty_tiles_for_rect(&mut dirty_tiles, rect, w, h);
                            }
                        }
                    }
                    self.record_brush_perf(
                        brush_start.elapsed(),
                        input_count,
                        stamps,
                        changed_stamps,
                        max_input_gap,
                    );
                    if changed {
                        if !dirty_tiles.is_empty() {
                            self.mark_dirty_tiles(dirty_tiles);
                        } else {
                            self.mark_dirty();
                        }
                    }
                    ui.ctx().request_repaint();
                }
                if primary_released {
                    self.reset_brush_stroke();
                }
            }
            MaskTool::Lasso => {
                if primary_down {
                    let p = [pos.x, pos.y];
                    push_freehand_point(&mut self.lasso_points, p);
                }
                if primary_released {
                    self.paint_lasso();
                }
            }
            MaskTool::Polygon => {
                if secondary_pressed {
                    if self.lasso_points.len() >= 3 {
                        self.paint_lasso();
                    }
                } else if primary_pressed {
                    let scale = self
                        .image_dims()
                        .map(|(w, _)| rect.width() / w.max(1) as f32)
                        .unwrap_or(1.0);
                    let snap_radius_image = self.edge_snap_radius / scale.max(0.001);
                    let snapped = if modifiers.ctrl {
                        self.snap_point_to_edge(pos, snap_radius_image)
                    } else {
                        pos
                    };
                    let p = [snapped.x, snapped.y];
                    if should_close_polygon(&self.lasso_points, p, scale) {
                        self.paint_lasso();
                    } else {
                        push_polygon_vertex(&mut self.lasso_points, p, scale);
                    }
                }
            }
            MaskTool::Select => {
                self.handle_select_tool(
                    pos,
                    primary_pressed,
                    primary_down,
                    primary_released,
                    modifiers,
                );
            }
            MaskTool::Line
            | MaskTool::VertLine
            | MaskTool::HorizLine
            | MaskTool::Rect
            | MaskTool::Ellipse => {
                if primary_down {
                    let p = [pos.x, pos.y];
                    if self.shape_drag_start.is_none() {
                        self.shape_drag_start = Some(p);
                    }
                    self.shape_drag_end = Some(p);
                }
                if primary_released {
                    if let (Some(start), Some(end)) = (self.shape_drag_start, self.shape_drag_end)
                        && let Some(shape) =
                            make_shape(self.tool, start, end, self.line_width, self.image_dims())
                    {
                        self.commit_shape(shape);
                    }
                    self.shape_drag_start = None;
                    self.shape_drag_end = None;
                }
            }
        }
    }

    fn handle_select_tool(
        &mut self,
        pos: Pos2,
        primary_pressed: bool,
        primary_down: bool,
        primary_released: bool,
        modifiers: egui::Modifiers,
    ) {
        let p = [pos.x, pos.y];
        if primary_pressed {
            if let Some((idx, handle)) = self.hit_test_shapes(p) {
                self.push_undo_snapshot();
                self.selected_shape = Some(idx);
                if let Some(mask) = self.selected_edit_raster_vector_mask_ref()
                    && let Some(shape) = mask.shapes.get(idx).copied()
                {
                    self.shape_drag = Some(ShapeDrag {
                        shape_idx: idx,
                        handle,
                        base: shape,
                        origin: p,
                    });
                }
            } else {
                self.selected_shape = None;
                self.shape_drag = None;
                self.mask_dirty = true;
            }
        }
        if primary_down && let Some(drag) = self.shape_drag {
            let new_shape = apply_shape_drag(drag, p, modifiers);
            if let Some(mask) = self.selected_edit_raster_vector_mask_mut()
                && let Some(slot) = mask.shapes.get_mut(drag.shape_idx)
            {
                *slot = new_shape;
                self.mark_dirty();
            }
        }
        if primary_released {
            self.shape_drag = None;
        }
    }

    fn hit_test_shapes(&self, p: [f32; 2]) -> Option<(usize, ShapeHandle)> {
        let mask = self.selected_edit_raster_vector_mask_ref()?;
        if let Some(sel) = self.selected_shape
            && let Some(shape) = mask.shapes.get(sel)
            && let Some(handle) = hit_shape_handles(*shape, p)
        {
            return Some((sel, handle));
        }
        for (idx, shape) in mask.shapes.iter().enumerate().rev() {
            if shape_contains(*shape, p) {
                return Some((idx, ShapeHandle::Body));
            }
        }
        None
    }

    fn draw_shape_overlay(
        &mut self,
        ui: &mut egui::Ui,
        rect: Rect,
        pointer_screen: Option<Pos2>,
        show_polygon_candidate: bool,
    ) {
        let Some((img_w, img_h)) = self.image_dims() else {
            return;
        };
        let painter = ui.painter().clone();
        let to_screen = |p: [f32; 2]| -> Pos2 {
            Pos2::new(
                rect.left() + p[0] / img_w.max(1) as f32 * rect.width(),
                rect.top() + p[1] / img_h.max(1) as f32 * rect.height(),
            )
        };
        if let Some(mask) = self.selected_edit_raster_vector_mask_ref() {
            for (idx, shape) in mask.shapes.iter().copied().enumerate() {
                let selected = self.selected_shape == Some(idx);
                let color = if shape.op().is_add() {
                    Color32::from_rgb(255, 180, 64)
                } else {
                    Color32::from_rgb(80, 210, 255)
                };
                draw_shape_outline(&painter, shape, &to_screen, color, selected);
                if selected {
                    draw_shape_handles(&painter, shape, &to_screen);
                }
            }
        }

        if self.lasso_points.len() >= 2 {
            let pts: Vec<Pos2> = self.lasso_points.iter().map(|&p| to_screen(p)).collect();
            painter.add(egui::Shape::line(
                pts.clone(),
                egui::Stroke::new(1.5, Color32::from_rgb(255, 220, 80)),
            ));
            painter.line_segment(
                [*pts.last().unwrap(), pts[0]],
                egui::Stroke::new(1.0, Color32::from_rgba_unmultiplied(255, 255, 255, 100)),
            );
            if self.tool == MaskTool::Polygon {
                for (idx, p) in pts.into_iter().enumerate() {
                    let fill = if idx == 0 {
                        Color32::from_rgb(255, 245, 120)
                    } else {
                        Color32::from_rgb(255, 220, 80)
                    };
                    painter.circle_filled(p, 4.0, fill);
                    painter.circle_stroke(p, 4.0, egui::Stroke::new(1.0, Color32::BLACK));
                }
            }
        } else if self.tool == MaskTool::Polygon && self.lasso_points.len() == 1 {
            let p = to_screen(self.lasso_points[0]);
            painter.circle_filled(p, 4.0, Color32::from_rgb(255, 245, 120));
            painter.circle_stroke(p, 4.0, egui::Stroke::new(1.0, Color32::BLACK));
        }

        if show_polygon_candidate && self.tool == MaskTool::Polygon {
            self.draw_polygon_candidate(ui, rect, pointer_screen, &to_screen);
        }

        if let (Some(start), Some(end)) = (self.shape_drag_start, self.shape_drag_end)
            && let Some(shape) =
                make_shape(self.tool, start, end, self.line_width, self.image_dims())
        {
            draw_shape_outline(
                &painter,
                shape,
                &to_screen,
                Color32::from_rgb(255, 240, 120),
                true,
            );
        }
    }

    fn draw_polygon_candidate(
        &self,
        ui: &mut egui::Ui,
        rect: Rect,
        pointer_screen: Option<Pos2>,
        to_screen: &impl Fn([f32; 2]) -> Pos2,
    ) {
        if self
            .selected_layer_ref()
            .map(|layer| MaskKind::from_mask(&layer.mask))
            != Some(MaskKind::Raster)
        {
            return;
        }
        let Some((img_w, img_h)) = self.image_dims() else {
            return;
        };
        let Some(screen) = pointer_screen else {
            return;
        };
        if !rect.contains(screen) {
            return;
        }
        let Some(mut candidate) = screen_to_image(rect, img_w, img_h, screen) else {
            return;
        };
        let snapping = ui.input(|i| i.modifiers.ctrl);
        let raw_screen = screen;
        let scale = rect.width() / img_w.max(1) as f32;
        if snapping {
            let snap_radius_image = self.edge_snap_radius / scale.max(0.001);
            candidate = self.snap_point_to_edge(candidate, snap_radius_image);
        }
        let candidate_arr = [candidate.x, candidate.y];
        let candidate_screen = to_screen(candidate_arr);
        let color = if snapping {
            animated_overlay_color(ui.ctx(), 240)
        } else {
            Color32::from_rgb(255, 245, 120)
        };
        let guide_stroke = egui::Stroke::new(1.5, color);
        if let Some(&last) = self.lasso_points.last() {
            ui.painter()
                .line_segment([to_screen(last), candidate_screen], guide_stroke);
        }
        if self.lasso_points.len() >= 2 {
            ui.painter().line_segment(
                [candidate_screen, to_screen(self.lasso_points[0])],
                egui::Stroke::new(1.0, Color32::from_rgba_unmultiplied(255, 255, 255, 100)),
            );
        }
        ui.painter().circle_filled(candidate_screen, 5.0, color);
        ui.painter().circle_stroke(
            candidate_screen,
            5.0,
            egui::Stroke::new(1.25, Color32::BLACK),
        );
        if snapping {
            let search_radius = self.edge_snap_radius.max(2.0);
            ui.painter().circle_stroke(
                raw_screen,
                search_radius,
                egui::Stroke::new(1.0, animated_overlay_color(ui.ctx(), 120)),
            );
            if candidate_screen.distance(raw_screen) > 1.5 {
                ui.painter().line_segment(
                    [raw_screen, candidate_screen],
                    egui::Stroke::new(1.0, animated_overlay_color(ui.ctx(), 190)),
                );
            }
            ui.painter().circle_stroke(
                candidate_screen,
                7.0,
                egui::Stroke::new(1.0, animated_overlay_color(ui.ctx(), 220)),
            );
            ui.ctx()
                .request_repaint_after(Duration::from_millis(EDGE_OVERLAY_REPAINT_MS));
        }
        ui.ctx().set_cursor_icon(egui::CursorIcon::Crosshair);
    }

    fn draw_brush_cursor(&self, ui: &mut egui::Ui, rect: Rect, pointer_screen: Option<Pos2>) {
        if !matches!(
            self.tool,
            MaskTool::Brush | MaskTool::EdgeBrush | MaskTool::GapFillBrush
        ) {
            return;
        }
        if self
            .selected_layer_ref()
            .map(|layer| MaskKind::from_mask(&layer.mask))
            != Some(MaskKind::Raster)
        {
            return;
        }
        let Some((img_w, _img_h)) = self.image_dims() else {
            return;
        };
        let Some(pos) = pointer_screen else {
            return;
        };
        if !rect.contains(pos) {
            return;
        }
        let scale = rect.width() / img_w.max(1) as f32;
        let radius = (self.brush_radius * scale).max(1.0);
        let ctrl_down = ui.input(|i| i.modifiers.ctrl);
        let color = match self.tool {
            MaskTool::Brush => Color32::from_rgb(255, 230, 120),
            MaskTool::EdgeBrush if ctrl_down => Color32::from_rgb(255, 230, 120),
            MaskTool::EdgeBrush => animated_overlay_color(ui.ctx(), 235),
            MaskTool::GapFillBrush => Color32::from_rgb(160, 255, 150),
            _ => Color32::WHITE,
        };
        let stroke = egui::Stroke::new(1.5, color);
        ui.painter().circle_stroke(pos, radius, stroke);
        ui.painter()
            .circle_stroke(pos, radius + 1.0, egui::Stroke::new(1.0, Color32::BLACK));
        if matches!(self.tool, MaskTool::EdgeBrush) {
            ui.ctx()
                .request_repaint_after(Duration::from_millis(EDGE_OVERLAY_REPAINT_MS));
        }
        ui.ctx().set_cursor_icon(egui::CursorIcon::Crosshair);
    }

    fn draw_gradient_handles(&mut self, ui: &mut egui::Ui, rect: Rect) -> bool {
        let Some(layer_idx) = self
            .layers
            .get(self.selected_layer)
            .map(|_| self.selected_layer)
        else {
            return false;
        };
        let mut changed = false;
        let mut used = false;
        let painter = ui.painter().clone();
        let stroke = egui::Stroke::new(2.0, Color32::from_rgb(255, 220, 80));
        let handle_fill = Color32::from_rgb(255, 250, 210);
        let handle_stroke = egui::Stroke::new(2.0, Color32::from_rgb(40, 30, 10));

        match &mut self.layers[layer_idx].mask {
            LocalMask::LinearGradient(mask) => {
                if !mask.initialized {
                    return false;
                }
                let start = norm_to_screen(rect, mask.start);
                let end = norm_to_screen(rect, mask.end);
                painter.line_segment([start, end], stroke);
                let (start_changed, start_used) = drag_norm_handle(
                    ui,
                    rect,
                    ui.id().with(("linear_start", layer_idx)),
                    start,
                    &mut mask.start,
                    "min",
                );
                let (end_changed, end_used) = drag_norm_handle(
                    ui,
                    rect,
                    ui.id().with(("linear_end", layer_idx)),
                    end,
                    &mut mask.end,
                    "max",
                );
                changed |= start_changed || end_changed;
                painter.circle_filled(start, 6.0, handle_fill);
                painter.circle_stroke(start, 6.0, handle_stroke);
                painter.circle_filled(end, 6.0, Color32::from_rgb(255, 190, 110));
                painter.circle_stroke(end, 6.0, handle_stroke);
                used = start_used || end_used;
            }
            LocalMask::RadialGradient(mask) => {
                if !mask.initialized {
                    return false;
                }
                let center = norm_to_screen(rect, mask.center);
                let inner_rx = mask.inner_radius.max(0.0) * rect.width();
                let inner_ry = mask.inner_radius_y.max(0.0) * rect.height();
                let outer_rx = mask.outer_radius.max(mask.inner_radius).max(0.0) * rect.width();
                let outer_ry =
                    mask.outer_radius_y.max(mask.inner_radius_y).max(0.0) * rect.height();
                let inner_x_handle = Pos2::new(center.x + inner_rx, center.y);
                let inner_y_handle = Pos2::new(center.x, center.y + inner_ry);
                let outer_x_handle = Pos2::new(center.x + outer_rx, center.y);
                let outer_y_handle = Pos2::new(center.x, center.y + outer_ry);
                draw_ellipse_stroke(
                    &painter,
                    center,
                    inner_rx,
                    inner_ry,
                    egui::Stroke::new(1.0, Color32::from_rgb(255, 220, 80)),
                );
                draw_ellipse_stroke(&painter, center, outer_rx, outer_ry, stroke);
                let (center_changed, center_used) = drag_norm_handle(
                    ui,
                    rect,
                    ui.id().with(("radial_center", layer_idx)),
                    center,
                    &mut mask.center,
                    "center",
                );
                let inner_x_resp = ui
                    .interact(
                        Rect::from_center_size(inner_x_handle, egui::vec2(24.0, 24.0)),
                        ui.id().with(("radial_inner_x", layer_idx)),
                        Sense::drag(),
                    )
                    .on_hover_text("内側 横");
                if inner_x_resp.dragged()
                    && let Some(pos) = inner_x_resp.interact_pointer_pos()
                {
                    let n = screen_to_norm(rect, pos);
                    mask.inner_radius = (n[0] - mask.center[0])
                        .abs()
                        .min((mask.outer_radius - 0.001).max(0.0));
                    changed = true;
                }
                let inner_y_resp = ui
                    .interact(
                        Rect::from_center_size(inner_y_handle, egui::vec2(24.0, 24.0)),
                        ui.id().with(("radial_inner_y", layer_idx)),
                        Sense::drag(),
                    )
                    .on_hover_text("内側 縦");
                if inner_y_resp.dragged()
                    && let Some(pos) = inner_y_resp.interact_pointer_pos()
                {
                    let n = screen_to_norm(rect, pos);
                    mask.inner_radius_y = (n[1] - mask.center[1])
                        .abs()
                        .min((mask.outer_radius_y - 0.001).max(0.0));
                    changed = true;
                }
                let outer_x_resp = ui
                    .interact(
                        Rect::from_center_size(outer_x_handle, egui::vec2(24.0, 24.0)),
                        ui.id().with(("radial_outer_x", layer_idx)),
                        Sense::drag(),
                    )
                    .on_hover_text("外側 横");
                if outer_x_resp.dragged()
                    && let Some(pos) = outer_x_resp.interact_pointer_pos()
                {
                    let n = screen_to_norm(rect, pos);
                    mask.outer_radius =
                        (n[0] - mask.center[0]).abs().max(mask.inner_radius + 0.001);
                    changed = true;
                }
                let outer_y_resp = ui
                    .interact(
                        Rect::from_center_size(outer_y_handle, egui::vec2(24.0, 24.0)),
                        ui.id().with(("radial_outer_y", layer_idx)),
                        Sense::drag(),
                    )
                    .on_hover_text("外側 縦");
                if outer_y_resp.dragged()
                    && let Some(pos) = outer_y_resp.interact_pointer_pos()
                {
                    let n = screen_to_norm(rect, pos);
                    mask.outer_radius_y = (n[1] - mask.center[1])
                        .abs()
                        .max(mask.inner_radius_y + 0.001);
                    changed = true;
                }
                painter.line_segment(
                    [Pos2::new(center.x - outer_rx, center.y), outer_x_handle],
                    egui::Stroke::new(1.0, Color32::from_rgba_unmultiplied(255, 220, 80, 100)),
                );
                painter.line_segment(
                    [Pos2::new(center.x, center.y - outer_ry), outer_y_handle],
                    egui::Stroke::new(1.0, Color32::from_rgba_unmultiplied(255, 220, 80, 100)),
                );
                painter.circle_filled(center, 6.0, handle_fill);
                painter.circle_stroke(center, 6.0, handle_stroke);
                painter.circle_filled(inner_x_handle, 5.0, Color32::from_rgb(255, 230, 140));
                painter.circle_stroke(inner_x_handle, 5.0, handle_stroke);
                painter.circle_filled(inner_y_handle, 5.0, Color32::from_rgb(255, 230, 140));
                painter.circle_stroke(inner_y_handle, 5.0, handle_stroke);
                painter.circle_filled(outer_x_handle, 6.0, Color32::from_rgb(255, 190, 110));
                painter.circle_stroke(outer_x_handle, 6.0, handle_stroke);
                painter.circle_filled(outer_y_handle, 6.0, Color32::from_rgb(255, 190, 110));
                painter.circle_stroke(outer_y_handle, 6.0, handle_stroke);
                changed |= center_changed;
                used = center_used
                    || inner_x_resp.hovered()
                    || inner_x_resp.dragged()
                    || inner_y_resp.hovered()
                    || inner_y_resp.dragged()
                    || outer_x_resp.hovered()
                    || outer_x_resp.dragged()
                    || outer_y_resp.hovered()
                    || outer_y_resp.dragged();
            }
            _ => {}
        }

        if changed {
            self.mark_dirty();
        }
        used
    }

    fn draw_crop_overlay(
        &mut self,
        ui: &mut egui::Ui,
        image_rect: Rect,
        img_w: usize,
        img_h: usize,
        pointer_screen: Option<Pos2>,
        pointer_allowed: bool,
    ) -> bool {
        if !self.crop_enabled {
            self.crop_drag = None;
            return false;
        }
        let crop = self
            .ensure_crop_rect()
            .unwrap_or_else(|| CropRect::full(img_w, img_h));
        let crop_screen = crop.to_screen_rect(image_rect, img_w, img_h);
        let painter = ui.painter().clone();
        if self.crop_overlay {
            for outside in outside_rects(image_rect, crop_screen) {
                painter.rect_filled(outside, 0.0, Color32::from_rgba_unmultiplied(0, 0, 0, 145));
            }
        }
        painter.rect_stroke(
            crop_screen,
            0.0,
            egui::Stroke::new(2.0, Color32::from_rgb(255, 230, 100)),
            egui::StrokeKind::Inside,
        );
        painter.rect_stroke(
            crop_screen.expand(1.0),
            0.0,
            egui::Stroke::new(1.0, Color32::BLACK),
            egui::StrokeKind::Outside,
        );

        if !self.crop_edit_mode || !pointer_allowed {
            return false;
        }
        let mut used = false;
        let mut handle_used = false;
        let scale_x = img_w.max(1) as f32 / image_rect.width().max(1.0);
        let scale_y = img_h.max(1) as f32 / image_rect.height().max(1.0);
        for (handle, center) in crop_handle_points(crop_screen) {
            if handle == CropHandle::Body {
                continue;
            }
            let handle_rect = Rect::from_center_size(center, egui::vec2(28.0, 28.0));
            let response = ui
                .interact(
                    handle_rect,
                    ui.id().with(("crop_handle", handle as u8)),
                    Sense::drag(),
                )
                .on_hover_text("crop を調整");
            painter.circle_filled(center, 5.5, Color32::from_rgb(255, 245, 180));
            painter.circle_stroke(
                center,
                5.5,
                egui::Stroke::new(1.5, Color32::from_rgb(30, 20, 0)),
            );
            if response.hovered() || response.dragged() {
                used = true;
                handle_used = true;
                ui.ctx().set_cursor_icon(crop_handle_cursor(handle));
            }
            if response.drag_started() {
                self.crop_drag = Some(CropDrag { handle, base: crop });
            }
            if response.dragged()
                && let Some(drag) = self.crop_drag
                && drag.handle == handle
            {
                let delta = response.drag_delta();
                let delta_x = delta.x * scale_x;
                let delta_y = delta.y * scale_y;
                self.crop_rect = Some(drag.base.dragged(handle, delta_x, delta_y, img_w, img_h));
            }
        }
        let active_non_body = self
            .crop_drag
            .map(|drag| drag.handle != CropHandle::Body)
            .unwrap_or(false);
        if !handle_used && !active_non_body {
            let body_response = ui
                .interact(
                    crop_screen,
                    ui.id().with(("crop_handle", CropHandle::Body as u8)),
                    Sense::drag(),
                )
                .on_hover_text("crop を移動");
            if body_response.hovered() || body_response.dragged() {
                used = true;
                ui.ctx().set_cursor_icon(if body_response.dragged() {
                    egui::CursorIcon::Grabbing
                } else {
                    egui::CursorIcon::Grab
                });
            }
            if body_response.drag_started() {
                self.crop_drag = Some(CropDrag {
                    handle: CropHandle::Body,
                    base: crop,
                });
            }
            if body_response.dragged()
                && let Some(drag) = self.crop_drag
                && drag.handle == CropHandle::Body
            {
                let delta = body_response.drag_delta();
                let delta_x = delta.x * scale_x;
                let delta_y = delta.y * scale_y;
                self.crop_rect =
                    Some(
                        drag.base
                            .dragged(CropHandle::Body, delta_x, delta_y, img_w, img_h),
                    );
            }
        }
        if ui.input(|i| !i.pointer.primary_down()) {
            self.crop_drag = None;
        }
        if !used
            && self.crop_edit_mode
            && pointer_screen
                .map(|p| crop_screen.contains(p))
                .unwrap_or(false)
        {
            ui.ctx().set_cursor_icon(egui::CursorIcon::Grab);
        }
        used || self.crop_drag.is_some()
    }

    fn drag_gradient_line(&mut self, image_pos: Pos2, started: bool) {
        let Some((w, h)) = self.image_dims() else {
            return;
        };
        let n = [
            (image_pos.x / w.max(1) as f32).clamp(0.0, 1.0),
            (image_pos.y / h.max(1) as f32).clamp(0.0, 1.0),
        ];
        let Some(layer) = self.selected_layer_mut() else {
            return;
        };
        match &mut layer.mask {
            LocalMask::LinearGradient(mask) => {
                if started || !mask.initialized {
                    mask.initialized = true;
                    mask.start = n;
                    mask.end = n;
                } else {
                    mask.end = n;
                }
                self.mark_dirty();
            }
            LocalMask::RadialGradient(mask) => {
                if started || !mask.initialized {
                    mask.initialized = true;
                    mask.center = n;
                    mask.inner_radius = 0.0;
                    mask.inner_radius_y = 0.0;
                    mask.outer_radius = 0.001;
                    mask.outer_radius_y = 0.001;
                } else {
                    let dx = n[0] - mask.center[0];
                    let dy = n[1] - mask.center[1];
                    let radius = (dx * dx + dy * dy).sqrt().max(0.001);
                    mask.outer_radius = radius;
                    mask.outer_radius_y = radius;
                    mask.inner_radius = (radius * 0.45).min(radius - 0.001).max(0.0);
                    mask.inner_radius_y = mask.inner_radius;
                }
                self.mark_dirty();
            }
            _ => {}
        }
    }
}

impl eframe::App for LocalAdjustLabApp {
    fn update(&mut self, ctx: &egui::Context, frame: &mut eframe::Frame) {
        let update_start = Instant::now();
        self.perf_stats.ui_frames += 1;
        if let Some(prev) = self.perf_last_update_start.replace(update_start) {
            let gap_ms = update_start.duration_since(prev).as_secs_f64() * 1000.0;
            self.perf_stats.frame_gap_samples += 1;
            self.perf_stats.frame_gap_ms_total += gap_ms;
            self.perf_stats.frame_gap_ms_max = self.perf_stats.frame_gap_ms_max.max(gap_ms);
        }
        if let Some(cpu_usage) = frame.info().cpu_usage {
            let cpu_ms = cpu_usage as f64 * 1000.0;
            self.perf_stats.eframe_cpu_samples += 1;
            self.perf_stats.eframe_cpu_ms_total += cpu_ms;
            self.perf_stats.eframe_cpu_ms_max = self.perf_stats.eframe_cpu_ms_max.max(cpu_ms);
        }
        if ctx.input(|i| i.pointer.primary_down()) {
            ctx.request_repaint();
        }
        let dropped: Vec<PathBuf> = ctx.input(|i| {
            i.raw
                .dropped_files
                .iter()
                .filter_map(|file| file.path.clone())
                .collect()
        });
        if let Some(path) = dropped.first() {
            self.load_path(ctx, path);
        }
        if !ctx.input(|i| i.pointer.primary_down()) {
            self.edge_brush_seed = None;
            self.radial_gradient_drag_active = false;
        }

        if ctx.input(|i| i.key_pressed(Key::S) && i.modifiers.ctrl) {
            self.save_result();
        }
        if ctx.input(|i| i.key_pressed(Key::Q) && !i.modifiers.ctrl) {
            self.show_source = !self.show_source;
        }
        if ctx.input(|i| i.key_pressed(Key::W) && !i.modifiers.ctrl) {
            self.show_mask = !self.show_mask;
        }
        let redo_pressed = ctx.input(|i| {
            (i.key_pressed(Key::Y) && i.modifiers.ctrl)
                || (i.key_pressed(Key::Z) && i.modifiers.ctrl && i.modifiers.shift)
        });
        if redo_pressed {
            self.redo();
        } else if ctx.input(|i| i.key_pressed(Key::Z) && i.modifiers.ctrl) {
            if self.tool == MaskTool::Polygon && self.lasso_points.pop().is_some() {
                self.mask_dirty = true;
                self.status = "頂点を戻しました。".to_string();
            } else {
                self.undo();
            }
        }
        if ctx.input(|i| i.key_pressed(Key::Enter))
            && self.tool == MaskTool::Polygon
            && self.lasso_points.len() >= 3
        {
            self.paint_lasso();
        }
        if ctx.input(|i| i.key_pressed(Key::Delete)) {
            if let Some(shape_idx) = self.selected_shape {
                self.push_undo_snapshot();
                if let Some(mask) = self.selected_edit_raster_vector_mask_mut()
                    && shape_idx < mask.shapes.len()
                {
                    mask.shapes.remove(shape_idx);
                    self.selected_shape = None;
                    self.mark_dirty();
                }
            }
        }
        if ctx.input(|i| i.key_pressed(Key::Escape)) {
            self.selected_shape = None;
            self.shape_drag = None;
            self.lasso_points.clear();
            self.shape_drag_start = None;
            self.shape_drag_end = None;
            self.mask_dirty = true;
        }
        let (ctrl_down, shift_down) = ctx.input(|i| (i.modifiers.ctrl, i.modifiers.shift));
        let nudge_step = if ctrl_down {
            NUDGE_PIXELS_FAST
        } else {
            NUDGE_PIXELS
        };
        let mut nudge = egui::Vec2::ZERO;
        if ctx.input(|i| i.key_pressed(Key::ArrowLeft)) {
            nudge.x -= nudge_step;
        }
        if ctx.input(|i| i.key_pressed(Key::ArrowRight)) {
            nudge.x += nudge_step;
        }
        if ctx.input(|i| i.key_pressed(Key::ArrowUp)) {
            nudge.y -= nudge_step;
        }
        if ctx.input(|i| i.key_pressed(Key::ArrowDown)) {
            nudge.y += nudge_step;
        }
        if nudge != egui::Vec2::ZERO {
            self.update_selected_shape(|shape| translate_shape(shape, nudge.x, nudge.y));
        }
        let rotate_step = if ctrl_down {
            ROTATE_DEG_STEP_FAST
        } else {
            ROTATE_DEG_STEP
        };
        let mut rotate_deg: f32 = 0.0;
        if ctx.input(|i| i.key_pressed(Key::OpenBracket)) {
            rotate_deg -= rotate_step;
        }
        if ctx.input(|i| i.key_pressed(Key::CloseBracket)) {
            rotate_deg += rotate_step;
        }
        if rotate_deg != 0.0 {
            let snap = shift_down;
            self.update_selected_shape(|shape| rotate_shape(shape, rotate_deg.to_radians(), snap));
        }
        if ctx.input(|i| i.key_pressed(Key::D)) {
            self.paint_mode = true;
        }
        if ctx.input(|i| i.key_pressed(Key::F)) {
            self.paint_mode = false;
        }
        if ctx.input(|i| i.key_pressed(Key::B)) {
            self.switch_tool(MaskTool::Brush);
        }
        if ctx.input(|i| i.key_pressed(Key::A)) {
            self.switch_tool(MaskTool::EdgeBrush);
        }
        if ctx.input(|i| i.key_pressed(Key::G)) {
            self.switch_tool(MaskTool::GapFillBrush);
        }
        if ctx.input(|i| i.key_pressed(Key::L)) {
            self.switch_tool(MaskTool::Lasso);
        }
        if ctx.input(|i| i.key_pressed(Key::P)) {
            self.switch_tool(MaskTool::Polygon);
        }
        if ctx.input(|i| i.key_pressed(Key::I)) {
            self.switch_tool(MaskTool::Line);
        }
        if ctx.input(|i| i.key_pressed(Key::V)) {
            self.switch_tool(MaskTool::VertLine);
        }
        if ctx.input(|i| i.key_pressed(Key::H)) {
            self.switch_tool(MaskTool::HorizLine);
        }
        if ctx.input(|i| i.key_pressed(Key::R)) {
            self.switch_tool(MaskTool::Rect);
        }
        if ctx.input(|i| i.key_pressed(Key::O)) {
            self.switch_tool(MaskTool::Ellipse);
        }
        if ctx.input(|i| i.key_pressed(Key::S) && !i.modifiers.ctrl) {
            self.switch_tool(MaskTool::Select);
        }

        self.poll_segmentation(ctx);
        self.poll_render(ctx);
        self.maybe_start_render(ctx);
        if self.pending.is_some() || self.segmentation_pending.is_some() {
            ctx.request_repaint_after(Duration::from_millis(16));
        }
        self.ensure_mask_texture(ctx);

        egui::CentralPanel::default()
            .frame(egui::Frame::default().fill(Color32::from_rgb(28, 28, 30)))
            .show(ctx, |ui| {
                let full_rect = ui.max_rect();
                self.draw_canvas(ui);
                self.draw_overlay_panel(ctx, full_rect);
                self.draw_tool_panel(ctx, full_rect);
            });
        self.draw_add_layer_dialog(ctx);
        let app_update_ms = update_start.elapsed().as_secs_f64() * 1000.0;
        self.perf_stats.app_update_ms_total += app_update_ms;
        self.perf_stats.app_update_ms_max = self.perf_stats.app_update_ms_max.max(app_update_ms);
        self.flush_perf_log();
        if self.result_dirty
            || self.mask_dirty
            || self.pending.is_some()
            || self.segmentation_pending.is_some()
        {
            ctx.request_repaint_after(Duration::from_millis(16));
        }
    }
}

fn draw_mask_controls(
    ui: &mut egui::Ui,
    layer: &mut LocalAdjustmentLayer,
    _dims: (usize, usize),
) -> bool {
    let mut changed = false;
    let kind = MaskKind::from_mask(&layer.mask);
    ui.label(
        egui::RichText::new(format!("マスク種類: {}", kind.label())).color(Color32::from_gray(200)),
    );
    ui.label(
        egui::RichText::new(kind.description())
            .size(11.0)
            .color(Color32::from_gray(170)),
    );

    match &mut layer.mask {
        LocalMask::Full => {
            ui.label("画像全体に効果を適用します。");
        }
        LocalMask::Raster(mask) => {
            ui.horizontal(|ui| {
                if ui.button("クリア").clicked() {
                    mask.alpha.fill(0.0);
                    changed = true;
                }
                if ui.button("塗りつぶし").clicked() {
                    mask.alpha.fill(1.0);
                    changed = true;
                }
            });
            ui.label("画像上でブラシを使って編集します。");
        }
        LocalMask::Subject(mask) => {
            ui.horizontal(|ui| {
                if ui.button("クリア").clicked() {
                    mask.alpha.fill(0.0);
                    changed = true;
                }
                if ui.button("塗りつぶし").clicked() {
                    mask.alpha.fill(1.0);
                    changed = true;
                }
            });
            ui.label("AI生成した被写体マスクを通常マスクとして使います。");
        }
        LocalMask::Segmentation(mask) => {
            ui.label("色分けされた領域候補をクリック/ドラッグで選択します。");
            ui.label(format!("領域数: {}", mask.label_count()));
        }
        LocalMask::RasterVector(mask) => {
            ui.horizontal(|ui| {
                if ui.button("ビットマップ消去").clicked() {
                    mask.alpha.fill(0.0);
                    changed = true;
                }
                if ui.button("オブジェクト消去").clicked() {
                    mask.shapes.clear();
                    changed = true;
                }
            });
            ui.label("画像上でブラシ / 境界筆 / 境界調整 / 囲み / オブジェクトを編集します。");
            ui.label(format!("オブジェクト数: {}", mask.shapes.len()));
        }
        LocalMask::LinearGradient(mask) => {
            if !mask.initialized {
                ui.label("画像上でドラッグして範囲を生成します。");
            } else {
                if ui.button("グラデーションをクリア").clicked() {
                    *mask = LinearGradientMask::default();
                    changed = true;
                }
                changed |= ui
                    .add(egui::Slider::new(&mut mask.start[0], 0.0..=1.0).text("開始 X"))
                    .changed();
                changed |= ui
                    .add(egui::Slider::new(&mut mask.start[1], 0.0..=1.0).text("開始 Y"))
                    .changed();
                changed |= ui
                    .add(egui::Slider::new(&mut mask.end[0], 0.0..=1.0).text("終了 X"))
                    .changed();
                changed |= ui
                    .add(egui::Slider::new(&mut mask.end[1], 0.0..=1.0).text("終了 Y"))
                    .changed();
            }
        }
        LocalMask::RadialGradient(mask) => {
            if !mask.initialized {
                ui.label("画像上でドラッグして範囲を生成します。");
            } else {
                if ui.button("グラデーションをクリア").clicked() {
                    *mask = RadialGradientMask::default();
                    changed = true;
                }
                changed |= ui
                    .add(egui::Slider::new(&mut mask.center[0], 0.0..=1.0).text("中心 X"))
                    .changed();
                changed |= ui
                    .add(egui::Slider::new(&mut mask.center[1], 0.0..=1.0).text("中心 Y"))
                    .changed();
                changed |= ui
                    .add(egui::Slider::new(&mut mask.inner_radius, 0.0..=1.5).text("内側 横"))
                    .changed();
                changed |= ui
                    .add(egui::Slider::new(&mut mask.inner_radius_y, 0.0..=1.5).text("内側 縦"))
                    .changed();
                changed |= ui
                    .add(egui::Slider::new(&mut mask.outer_radius, 0.0..=1.5).text("外側 横"))
                    .changed();
                changed |= ui
                    .add(egui::Slider::new(&mut mask.outer_radius_y, 0.0..=1.5).text("外側 縦"))
                    .changed();
                mask.outer_radius = mask.outer_radius.max(mask.inner_radius + 0.001);
                mask.outer_radius_y = mask.outer_radius_y.max(mask.inner_radius_y + 0.001);
            }
        }
        LocalMask::LumaRange(mask) => {
            changed |= draw_range_sliders(ui, mask, "輝度");
        }
        LocalMask::ColorRange(mask) => {
            if !mask.initialized {
                ui.label("画像上をクリックしてスポイトで色を拾います。");
            } else if ui.button("色範囲をクリア").clicked() {
                *mask = ColorRangeMask::default();
                changed = true;
            }
            let mut r = mask.target_rgb[0] as i32;
            let mut g = mask.target_rgb[1] as i32;
            let mut b = mask.target_rgb[2] as i32;
            let mut rgb_changed = false;
            rgb_changed |= ui
                .add(egui::Slider::new(&mut r, 0..=255).text("R"))
                .changed();
            rgb_changed |= ui
                .add(egui::Slider::new(&mut g, 0..=255).text("G"))
                .changed();
            rgb_changed |= ui
                .add(egui::Slider::new(&mut b, 0..=255).text("B"))
                .changed();
            if rgb_changed {
                mask.target_rgb = [r as u8, g as u8, b as u8];
                mask.initialized = true;
            }
            changed |= rgb_changed;
            changed |= ui
                .add(egui::Slider::new(&mut mask.tolerance, 0.0..=1.0).text("許容幅"))
                .changed();
            changed |= ui
                .add(egui::Slider::new(&mut mask.feather, 0.0..=1.0).text("範囲ぼかし"))
                .changed();
        }
    }
    changed
}

fn draw_layer_mask_thumbnail(
    ui: &mut egui::Ui,
    layer: &LocalAdjustmentLayer,
    image: Option<RgbaImageRef<'_>>,
    selected: bool,
) -> egui::Response {
    const SIZE: f32 = 48.0;
    const GRID: usize = 32;
    let (rect, response) = ui.allocate_exact_size(egui::vec2(SIZE, SIZE), Sense::click());
    let painter = ui.painter();
    painter.rect_filled(rect, 4.0, Color32::from_rgba_unmultiplied(8, 8, 10, 180));

    if let Some(image) = image {
        let mut preview_layer = layer.clone();
        preview_layer.opacity = 1.0;
        if let Ok(mask) = evaluate_layer_mask(image, &preview_layer) {
            if let Some((min_x, min_y, max_x, max_y)) =
                mask_active_bounds(&mask, image.width, image.height)
            {
                let inner = rect.shrink(5.0);
                let crop_w = (max_x - min_x + 1).max(1);
                let crop_h = (max_y - min_y + 1).max(1);
                for gy in 0..GRID {
                    for gx in 0..GRID {
                        let sx = min_x + ((gx as f32 + 0.5) * crop_w as f32 / GRID as f32) as usize;
                        let sy = min_y + ((gy as f32 + 0.5) * crop_h as f32 / GRID as f32) as usize;
                        let a = mask
                            [sy.min(image.height - 1) * image.width + sx.min(image.width - 1)]
                        .clamp(0.0, 1.0);
                        if a <= 0.02 {
                            continue;
                        }
                        let x0 = inner.left() + gx as f32 * inner.width() / GRID as f32;
                        let y0 = inner.top() + gy as f32 * inner.height() / GRID as f32;
                        let x1 = inner.left() + (gx + 1) as f32 * inner.width() / GRID as f32;
                        let y1 = inner.top() + (gy + 1) as f32 * inner.height() / GRID as f32;
                        let alpha = (60.0 + a * 185.0).round() as u8;
                        painter.rect_filled(
                            Rect::from_min_max(egui::pos2(x0, y0), egui::pos2(x1, y1)),
                            0.0,
                            Color32::from_rgba_unmultiplied(255, 85, 125, alpha),
                        );
                    }
                }
            } else {
                draw_empty_thumbnail_mark(painter, rect);
            }
        } else {
            draw_empty_thumbnail_mark(painter, rect);
        }
    } else {
        draw_empty_thumbnail_mark(painter, rect);
    }

    painter.rect_stroke(
        rect,
        4.0,
        egui::Stroke::new(
            1.0,
            if selected {
                Color32::from_rgba_unmultiplied(170, 215, 255, 210)
            } else {
                Color32::from_rgba_unmultiplied(255, 255, 255, 45)
            },
        ),
        egui::StrokeKind::Inside,
    );
    response
}

fn draw_empty_thumbnail_mark(painter: &egui::Painter, rect: Rect) {
    painter.line_segment(
        [
            egui::pos2(rect.left() + 11.0, rect.center().y),
            egui::pos2(rect.right() - 11.0, rect.center().y),
        ],
        egui::Stroke::new(1.0, Color32::from_gray(100)),
    );
}

fn mask_active_bounds(
    mask: &[f32],
    width: usize,
    height: usize,
) -> Option<(usize, usize, usize, usize)> {
    let mut min_x = width;
    let mut min_y = height;
    let mut max_x = 0;
    let mut max_y = 0;
    let mut found = false;
    for y in 0..height {
        for x in 0..width {
            if mask[y * width + x] > 0.04 {
                min_x = min_x.min(x);
                min_y = min_y.min(y);
                max_x = max_x.max(x);
                max_y = max_y.max(y);
                found = true;
            }
        }
    }
    if !found {
        return None;
    }
    let pad_x = ((max_x - min_x + 1) as f32 * 0.12).round() as usize + 1;
    let pad_y = ((max_y - min_y + 1) as f32 * 0.12).round() as usize + 1;
    Some((
        min_x.saturating_sub(pad_x),
        min_y.saturating_sub(pad_y),
        (max_x + pad_x).min(width.saturating_sub(1)),
        (max_y + pad_y).min(height.saturating_sub(1)),
    ))
}

fn effect_summary(effect: &LocalEffect) -> String {
    match effect {
        LocalEffect::None => "効果なし".to_string(),
        LocalEffect::Tone(_) => "色調補正".to_string(),
        LocalEffect::ToneCurve(_) => "トーンカーブ".to_string(),
        LocalEffect::Clarity(_) => "明瞭度".to_string(),
        LocalEffect::HighlightsShadows(_) => "ハイライト/シャドウ".to_string(),
        LocalEffect::Dehaze(params) => format!("かすみ除去 {:.0}%", params.amount * 100.0),
        LocalEffect::Blur(params) => format!("ぼかし {:.0}px", params.radius_px),
        LocalEffect::SoftFocus(params) => format!("ソフトフォーカス {:.0}px", params.radius_px),
        LocalEffect::Mosaic(params) => format!("モザイク {}px", params.block_px),
        LocalEffect::Sharpen(params) => format!("シャープ {:.0}%", params.amount * 100.0),
        LocalEffect::Hsl(params) => format!("色相 {:+.0}°", params.hue_degrees),
        LocalEffect::Look(params) => format!("ルック {}", look_preset_label(params.preset)),
        LocalEffect::Bloom(params) => format!("ブルーム {:.0}px", params.radius_px),
        LocalEffect::Vignette(params) => format!("ビネット {:.0}%", params.strength * 100.0),
        LocalEffect::FilmGrain(params) => format!("粒子 {:.0}%", params.amount * 100.0),
        LocalEffect::ChromaticAberration(params) => {
            format!("色収差 {:.1}px", params.offset_px)
        }
        LocalEffect::Halftone(params) => format!("ハーフトーン {}px", params.cell_px),
        LocalEffect::StarGlow(params) => {
            format!("クロス光 {}本 {:.0}px", params.ray_count, params.length_px)
        }
        LocalEffect::EdgeSmooth(params) => {
            format!("エッジ保持ぼかし {:.0}px", params.radius_px)
        }
    }
}

fn look_preset_label(preset: LookPreset) -> &'static str {
    match preset {
        LookPreset::Sunset => "夕焼け",
        LookPreset::Night => "夜景",
        LookPreset::BrightSun => "明るい日光",
        LookPreset::Pale => "淡色",
        LookPreset::Cool => "寒色",
        LookPreset::Warm => "暖色",
        LookPreset::RetroFilm => "レトロ/フィルム",
        LookPreset::TealOrange => "ティール&オレンジ",
        LookPreset::CherryBlossom => "桜色",
        LookPreset::FreshGreen => "新緑",
        LookPreset::Moonlight => "月明かり",
        LookPreset::HighKey => "ハイキー",
        LookPreset::LowKey => "ローキー",
        LookPreset::Sepia => "セピア",
        LookPreset::Cyberpunk => "サイバーパンク",
    }
}

fn draw_range_sliders(ui: &mut egui::Ui, mask: &mut RangeMask, label: &str) -> bool {
    let mut changed = false;
    changed |= ui
        .add(egui::Slider::new(&mut mask.min, 0.0..=1.0).text(format!("{label} 下限")))
        .changed();
    changed |= ui
        .add(egui::Slider::new(&mut mask.max, 0.0..=1.0).text(format!("{label} 上限")))
        .changed();
    changed |= ui
        .add(egui::Slider::new(&mut mask.feather, 0.0..=1.0).text("範囲ぼかし"))
        .changed();
    changed
}

fn draw_effect_kind_selector(ui: &mut egui::Ui, layer: &mut LocalAdjustmentLayer) -> bool {
    let mut changed = false;
    ui.label(egui::RichText::new("加工内容:").color(Color32::from_gray(200)));
    let mut kind = EffectKind::from_effect(&layer.effect);
    let before = kind;
    lab_combo_box(ui, "effect_kind", kind.label(), |ui| {
        for candidate in [
            EffectKind::None,
            EffectKind::Tone,
            EffectKind::ToneCurve,
            EffectKind::Clarity,
            EffectKind::HighlightsShadows,
            EffectKind::Dehaze,
            EffectKind::Blur,
            EffectKind::SoftFocus,
            EffectKind::Mosaic,
            EffectKind::Sharpen,
            EffectKind::Hsl,
            EffectKind::Look,
            EffectKind::Bloom,
            EffectKind::Vignette,
            EffectKind::FilmGrain,
            EffectKind::ChromaticAberration,
            EffectKind::Halftone,
            EffectKind::StarGlow,
            EffectKind::EdgeSmooth,
        ] {
            ui.selectable_value(&mut kind, candidate, candidate.label());
        }
    });
    if kind != before {
        layer.effect = default_effect(kind);
        changed = true;
    }
    changed
}

fn preset_button(ui: &mut egui::Ui, label: &str) -> bool {
    ui.add(egui::Button::new(label).small()).clicked()
}

fn draw_tone_curve_preview(ui: &mut egui::Ui, params: ToneCurveParams) {
    let desired = egui::vec2(ui.available_width().min(220.0), 120.0);
    let (rect, _) = ui.allocate_exact_size(desired, Sense::hover());
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, 4.0, Color32::from_gray(24));
    painter.rect_stroke(
        rect,
        4.0,
        egui::Stroke::new(1.0, Color32::from_gray(70)),
        egui::StrokeKind::Inside,
    );
    for i in 1..4 {
        let t = i as f32 / 4.0;
        let x = egui::lerp(rect.left()..=rect.right(), t);
        let y = egui::lerp(rect.bottom()..=rect.top(), t);
        painter.line_segment(
            [Pos2::new(x, rect.top()), Pos2::new(x, rect.bottom())],
            egui::Stroke::new(1.0, Color32::from_gray(42)),
        );
        painter.line_segment(
            [Pos2::new(rect.left(), y), Pos2::new(rect.right(), y)],
            egui::Stroke::new(1.0, Color32::from_gray(42)),
        );
    }
    painter.line_segment(
        [rect.left_bottom(), rect.right_top()],
        egui::Stroke::new(1.0, Color32::from_gray(68)),
    );
    let mut prev = None;
    for i in 0..=64 {
        let x01 = i as f32 / 64.0;
        let y01 = preview_tone_curve_value(x01, params.points);
        let p = Pos2::new(
            egui::lerp(rect.left()..=rect.right(), x01),
            egui::lerp(rect.bottom()..=rect.top(), y01),
        );
        if let Some(prev) = prev {
            painter.line_segment(
                [prev, p],
                egui::Stroke::new(2.0, Color32::from_rgb(120, 210, 255)),
            );
        }
        prev = Some(p);
    }
}

fn preview_tone_curve_value(x: f32, points: [f32; 5]) -> f32 {
    let x = x.clamp(0.0, 1.0);
    let seg = ((x * 4.0).floor() as usize).min(3);
    let x0 = seg as f32 * 0.25;
    let t = ((x - x0) * 4.0).clamp(0.0, 1.0);
    points[seg].clamp(0.0, 1.0)
        + (points[seg + 1].clamp(0.0, 1.0) - points[seg].clamp(0.0, 1.0)) * t
}

fn draw_effect_params(ui: &mut egui::Ui, layer: &mut LocalAdjustmentLayer) -> bool {
    let mut changed = false;
    let effect_kind = EffectKind::from_effect(&layer.effect);
    ui.horizontal(|ui| {
        ui.label(
            egui::RichText::new("加工パラメータ")
                .size(14.0)
                .strong()
                .color(Color32::WHITE),
        );
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if ui.button("リセット").clicked() {
                layer.effect = default_effect(effect_kind);
                changed = true;
            }
        });
    });
    if changed {
        return true;
    }
    match &mut layer.effect {
        LocalEffect::None => {
            ui.label("加工内容を選ぶと、このレイヤーの効果が有効になります。");
        }
        LocalEffect::Tone(params) => {
            ui.label(egui::RichText::new("プリセット").color(Color32::from_gray(190)));
            ui.horizontal_wrapped(|ui| {
                if preset_button(ui, "明るく") {
                    *params = ToneParams {
                        brightness: 12.0,
                        contrast: 4.0,
                        ..Default::default()
                    };
                    changed = true;
                }
                if preset_button(ui, "鮮やか") {
                    *params = ToneParams {
                        saturation: 18.0,
                        vibrance: 32.0,
                        ..Default::default()
                    };
                    changed = true;
                }
                if preset_button(ui, "自然な彩度+") {
                    *params = ToneParams {
                        vibrance: 45.0,
                        ..Default::default()
                    };
                    changed = true;
                }
                if preset_button(ui, "柔らかく") {
                    *params = ToneParams {
                        contrast: -10.0,
                        vibrance: 12.0,
                        ..Default::default()
                    };
                    changed = true;
                }
                if preset_button(ui, "暖かく") {
                    *params = ToneParams {
                        temperature: 35.0,
                        vibrance: 12.0,
                        ..Default::default()
                    };
                    changed = true;
                }
            });
            changed |= ui
                .add(egui::Slider::new(&mut params.brightness, -100.0..=100.0).text("明るさ"))
                .changed();
            changed |= ui
                .add(egui::Slider::new(&mut params.contrast, -100.0..=100.0).text("コントラスト"))
                .changed();
            changed |= ui
                .add(egui::Slider::new(&mut params.gamma, 0.2..=5.0).text("ガンマ"))
                .changed();
            changed |= ui
                .add(egui::Slider::new(&mut params.saturation, -100.0..=100.0).text("彩度"))
                .changed();
            changed |= ui
                .add(egui::Slider::new(&mut params.vibrance, -100.0..=100.0).text("自然な彩度"))
                .changed();
            changed |= ui
                .add(egui::Slider::new(&mut params.temperature, -100.0..=100.0).text("色温度"))
                .changed();
        }
        LocalEffect::ToneCurve(params) => {
            ui.label(egui::RichText::new("プリセット").color(Color32::from_gray(190)));
            ui.horizontal_wrapped(|ui| {
                if preset_button(ui, "S字") {
                    *params = ToneCurveParams {
                        points: [0.0, 0.18, 0.50, 0.82, 1.0],
                    };
                    changed = true;
                }
                if preset_button(ui, "明るく") {
                    *params = ToneCurveParams {
                        points: [0.0, 0.34, 0.62, 0.86, 1.0],
                    };
                    changed = true;
                }
                if preset_button(ui, "暗く") {
                    *params = ToneCurveParams {
                        points: [0.0, 0.16, 0.40, 0.68, 1.0],
                    };
                    changed = true;
                }
                if preset_button(ui, "フェード") {
                    *params = ToneCurveParams {
                        points: [0.08, 0.28, 0.52, 0.76, 0.96],
                    };
                    changed = true;
                }
            });
            draw_tone_curve_preview(ui, *params);
            ui.label(
                egui::RichText::new("RGB共通の簡易カーブです。チャンネル別カーブは後続候補です。")
                    .size(10.0)
                    .color(Color32::from_gray(170)),
            );
            for (idx, label) in ["黒", "暗部", "中間", "明部", "白"].iter().enumerate() {
                changed |= ui
                    .add(egui::Slider::new(&mut params.points[idx], 0.0..=1.0).text(*label))
                    .changed();
            }
        }
        LocalEffect::Clarity(params) => {
            ui.label(egui::RichText::new("プリセット").color(Color32::from_gray(190)));
            ui.horizontal_wrapped(|ui| {
                if preset_button(ui, "くっきり") {
                    *params = ClarityParams {
                        amount: 0.35,
                        radius_px: 18.0,
                    };
                    changed = true;
                }
                if preset_button(ui, "柔らかく") {
                    *params = ClarityParams {
                        amount: -0.35,
                        radius_px: 20.0,
                    };
                    changed = true;
                }
            });
            changed |= ui
                .add(egui::Slider::new(&mut params.amount, -1.0..=1.0).text("量"))
                .changed();
            changed |= ui
                .add(egui::Slider::new(&mut params.radius_px, 1.0..=80.0).text("半径"))
                .changed();
        }
        LocalEffect::HighlightsShadows(params) => {
            ui.label(egui::RichText::new("プリセット").color(Color32::from_gray(190)));
            ui.horizontal_wrapped(|ui| {
                if preset_button(ui, "シャドウを強調") {
                    *params = HighlightsShadowsParams {
                        shadows: -35.0,
                        highlights: 0.0,
                    };
                    changed = true;
                }
                if preset_button(ui, "シャドウを明るく") {
                    *params = HighlightsShadowsParams {
                        shadows: 45.0,
                        highlights: 0.0,
                    };
                    changed = true;
                }
                if preset_button(ui, "ハイライトを強調") {
                    *params = HighlightsShadowsParams {
                        shadows: 0.0,
                        highlights: -30.0,
                    };
                    changed = true;
                }
                if preset_button(ui, "ハイライトを暗く") {
                    *params = HighlightsShadowsParams {
                        shadows: 0.0,
                        highlights: 35.0,
                    };
                    changed = true;
                }
            });
            changed |= ui
                .add(egui::Slider::new(&mut params.shadows, -100.0..=100.0).text("シャドウ"))
                .changed();
            changed |= ui
                .add(egui::Slider::new(&mut params.highlights, -100.0..=100.0).text("ハイライト"))
                .changed();
        }
        LocalEffect::Dehaze(params) => {
            ui.label(egui::RichText::new("プリセット").color(Color32::from_gray(190)));
            ui.horizontal_wrapped(|ui| {
                if preset_button(ui, "弱く") {
                    *params = DehazeParams {
                        amount: 0.25,
                        radius_px: 10.0,
                        min_transmission: 0.38,
                        saturation: 4.0,
                    };
                    changed = true;
                }
                if preset_button(ui, "標準") {
                    *params = DehazeParams {
                        amount: 0.45,
                        radius_px: 14.0,
                        min_transmission: 0.32,
                        saturation: 8.0,
                    };
                    changed = true;
                }
                if preset_button(ui, "強く") {
                    *params = DehazeParams {
                        amount: 0.70,
                        radius_px: 20.0,
                        min_transmission: 0.25,
                        saturation: 10.0,
                    };
                    changed = true;
                }
            });
            ui.label(
                egui::RichText::new(
                    "写真向けの霧・白っぽさ低減です。AI絵では弱めから確認してください。",
                )
                .size(10.0)
                .color(Color32::from_gray(170)),
            );
            changed |= ui
                .add(egui::Slider::new(&mut params.amount, 0.0..=1.0).text("量"))
                .changed();
            changed |= ui
                .add(egui::Slider::new(&mut params.radius_px, 0.0..=48.0).text("半径"))
                .changed();
            changed |= ui
                .add(
                    egui::Slider::new(&mut params.min_transmission, 0.10..=0.90).text("最小透過率"),
                )
                .changed();
            changed |= ui
                .add(egui::Slider::new(&mut params.saturation, -50.0..=50.0).text("彩度補正"))
                .changed();
        }
        LocalEffect::Blur(params) => {
            ui.label(egui::RichText::new("プリセット").color(Color32::from_gray(190)));
            ui.horizontal_wrapped(|ui| {
                if preset_button(ui, "弱く") {
                    params.radius_px = 6.0;
                    changed = true;
                }
                if preset_button(ui, "背景ぼかし") {
                    params.radius_px = 18.0;
                    changed = true;
                }
                if preset_button(ui, "強く") {
                    params.radius_px = 40.0;
                    changed = true;
                }
            });
            changed |= ui
                .add(egui::Slider::new(&mut params.radius_px, 0.0..=80.0).text("半径"))
                .changed();
        }
        LocalEffect::SoftFocus(params) => {
            ui.label(egui::RichText::new("プリセット").color(Color32::from_gray(190)));
            ui.horizontal_wrapped(|ui| {
                if preset_button(ui, "淡く") {
                    *params = SoftFocusParams {
                        radius_px: 16.0,
                        strength: 0.25,
                    };
                    changed = true;
                }
                if preset_button(ui, "発光") {
                    *params = SoftFocusParams {
                        radius_px: 28.0,
                        strength: 0.45,
                    };
                    changed = true;
                }
            });
            changed |= ui
                .add(egui::Slider::new(&mut params.radius_px, 0.0..=80.0).text("半径"))
                .changed();
            changed |= ui
                .add(egui::Slider::new(&mut params.strength, 0.0..=1.0).text("強さ"))
                .changed();
        }
        LocalEffect::Mosaic(params) => {
            ui.label(egui::RichText::new("プリセット").color(Color32::from_gray(190)));
            ui.horizontal_wrapped(|ui| {
                if preset_button(ui, "8px") {
                    params.block_px = 8;
                    changed = true;
                }
                if preset_button(ui, "12px") {
                    params.block_px = 12;
                    changed = true;
                }
                if preset_button(ui, "24px") {
                    params.block_px = 24;
                    changed = true;
                }
            });
            let mut block = params.block_px as i32;
            changed |= ui
                .add(egui::Slider::new(&mut block, 1..=96).text("タイル(px)"))
                .changed();
            params.block_px = block.max(1) as u32;
        }
        LocalEffect::Sharpen(params) => {
            ui.label(egui::RichText::new("プリセット").color(Color32::from_gray(190)));
            ui.horizontal_wrapped(|ui| {
                if preset_button(ui, "弱く") {
                    *params = SharpenParams {
                        amount: 0.35,
                        radius_px: 1.0,
                    };
                    changed = true;
                }
                if preset_button(ui, "くっきり") {
                    *params = SharpenParams {
                        amount: 0.7,
                        radius_px: 1.0,
                    };
                    changed = true;
                }
                if preset_button(ui, "線強調") {
                    *params = SharpenParams {
                        amount: 0.55,
                        radius_px: 2.0,
                    };
                    changed = true;
                }
            });
            changed |= ui
                .add(egui::Slider::new(&mut params.amount, 0.0..=2.0).text("量"))
                .changed();
            changed |= ui
                .add(egui::Slider::new(&mut params.radius_px, 0.0..=8.0).text("半径"))
                .changed();
        }
        LocalEffect::Hsl(params) => {
            ui.label(egui::RichText::new("プリセット").color(Color32::from_gray(190)));
            ui.horizontal_wrapped(|ui| {
                if preset_button(ui, "赤へ") {
                    *params = HslParams {
                        hue_degrees: -25.0,
                        saturation: 10.0,
                        lightness: 0.0,
                    };
                    changed = true;
                }
                if preset_button(ui, "青へ") {
                    *params = HslParams {
                        hue_degrees: 70.0,
                        saturation: 8.0,
                        lightness: 0.0,
                    };
                    changed = true;
                }
                if preset_button(ui, "緑へ") {
                    *params = HslParams {
                        hue_degrees: 120.0,
                        saturation: 8.0,
                        lightness: 0.0,
                    };
                    changed = true;
                }
                if preset_button(ui, "彩度+") {
                    *params = HslParams {
                        saturation: 25.0,
                        ..Default::default()
                    };
                    changed = true;
                }
                if preset_button(ui, "淡く") {
                    *params = HslParams {
                        saturation: -25.0,
                        lightness: 8.0,
                        ..Default::default()
                    };
                    changed = true;
                }
            });
            ui.label(
                egui::RichText::new("カラー範囲マスクと組み合わせると、髪や服だけ色替えできます。")
                    .size(10.0)
                    .color(Color32::from_gray(170)),
            );
            changed |= ui
                .add(egui::Slider::new(&mut params.hue_degrees, -180.0..=180.0).text("色相"))
                .changed();
            changed |= ui
                .add(egui::Slider::new(&mut params.saturation, -100.0..=100.0).text("彩度"))
                .changed();
            changed |= ui
                .add(egui::Slider::new(&mut params.lightness, -100.0..=100.0).text("明度"))
                .changed();
        }
        LocalEffect::Look(params) => {
            ui.label(egui::RichText::new("プリセット").color(Color32::from_gray(190)));
            ui.horizontal_wrapped(|ui| {
                if preset_button(ui, "夕焼け") {
                    *params = LookParams {
                        preset: LookPreset::Sunset,
                        strength: 0.7,
                    };
                    changed = true;
                }
                if preset_button(ui, "夜景") {
                    *params = LookParams {
                        preset: LookPreset::Night,
                        strength: 0.7,
                    };
                    changed = true;
                }
                if preset_button(ui, "明るい日光") {
                    *params = LookParams {
                        preset: LookPreset::BrightSun,
                        strength: 0.65,
                    };
                    changed = true;
                }
                if preset_button(ui, "淡色") {
                    *params = LookParams {
                        preset: LookPreset::Pale,
                        strength: 0.75,
                    };
                    changed = true;
                }
            });
            let before = params.preset;
            lab_combo_box(ui, "look_preset", look_preset_label(params.preset), |ui| {
                for preset in [
                    LookPreset::Sunset,
                    LookPreset::Night,
                    LookPreset::BrightSun,
                    LookPreset::Pale,
                    LookPreset::Cool,
                    LookPreset::Warm,
                    LookPreset::RetroFilm,
                    LookPreset::TealOrange,
                    LookPreset::CherryBlossom,
                    LookPreset::FreshGreen,
                    LookPreset::Moonlight,
                    LookPreset::HighKey,
                    LookPreset::LowKey,
                    LookPreset::Sepia,
                    LookPreset::Cyberpunk,
                ] {
                    ui.selectable_value(&mut params.preset, preset, look_preset_label(preset));
                }
            });
            changed |= params.preset != before;
            changed |= ui
                .add(egui::Slider::new(&mut params.strength, 0.0..=1.0).text("強度"))
                .changed();
        }
        LocalEffect::Bloom(params) => {
            ui.label(egui::RichText::new("プリセット").color(Color32::from_gray(190)));
            ui.horizontal_wrapped(|ui| {
                if preset_button(ui, "弱い光") {
                    *params = BloomParams {
                        threshold: 0.75,
                        radius_px: 18.0,
                        strength: 0.25,
                    };
                    changed = true;
                }
                if preset_button(ui, "瞳/光源") {
                    *params = BloomParams {
                        threshold: 0.82,
                        radius_px: 10.0,
                        strength: 0.55,
                    };
                    changed = true;
                }
                if preset_button(ui, "強いにじみ") {
                    *params = BloomParams {
                        threshold: 0.65,
                        radius_px: 32.0,
                        strength: 0.65,
                    };
                    changed = true;
                }
            });
            changed |= ui
                .add(egui::Slider::new(&mut params.threshold, 0.0..=0.98).text("明部しきい値"))
                .changed();
            changed |= ui
                .add(egui::Slider::new(&mut params.radius_px, 0.0..=120.0).text("半径"))
                .changed();
            changed |= ui
                .add(egui::Slider::new(&mut params.strength, 0.0..=2.0).text("強さ"))
                .changed();
        }
        LocalEffect::Vignette(params) => {
            ui.label(egui::RichText::new("プリセット").color(Color32::from_gray(190)));
            ui.horizontal_wrapped(|ui| {
                if preset_button(ui, "周辺を暗く") {
                    *params = VignetteParams {
                        strength: 0.35,
                        radius: 0.52,
                        feather: 0.36,
                    };
                    changed = true;
                }
                if preset_button(ui, "周辺を明るく") {
                    *params = VignetteParams {
                        strength: -0.25,
                        radius: 0.50,
                        feather: 0.38,
                    };
                    changed = true;
                }
            });
            changed |= ui
                .add(egui::Slider::new(&mut params.strength, -1.0..=1.0).text("強さ"))
                .changed();
            changed |= ui
                .add(egui::Slider::new(&mut params.radius, 0.0..=1.0).text("開始半径"))
                .changed();
            changed |= ui
                .add(egui::Slider::new(&mut params.feather, 0.01..=1.0).text("ぼかし幅"))
                .changed();
        }
        LocalEffect::FilmGrain(params) => {
            ui.label(egui::RichText::new("プリセット").color(Color32::from_gray(190)));
            ui.horizontal_wrapped(|ui| {
                if preset_button(ui, "微量") {
                    *params = FilmGrainParams {
                        amount: 0.08,
                        size_px: 1,
                        seed: params.seed,
                    };
                    changed = true;
                }
                if preset_button(ui, "フィルム") {
                    *params = FilmGrainParams {
                        amount: 0.18,
                        size_px: 2,
                        seed: params.seed,
                    };
                    changed = true;
                }
            });
            changed |= ui
                .add(egui::Slider::new(&mut params.amount, 0.0..=1.0).text("量"))
                .changed();
            let mut size = params.size_px as i32;
            changed |= ui
                .add(egui::Slider::new(&mut size, 1..=12).text("粒サイズ(px)"))
                .changed();
            params.size_px = size.max(1) as u32;
            let mut seed = params.seed as i32;
            changed |= ui
                .add(egui::Slider::new(&mut seed, 0..=9999).text("seed"))
                .changed();
            params.seed = seed.max(0) as u32;
        }
        LocalEffect::ChromaticAberration(params) => {
            ui.label(egui::RichText::new("プリセット").color(Color32::from_gray(190)));
            ui.horizontal_wrapped(|ui| {
                if preset_button(ui, "微量") {
                    params.offset_px = 1.2;
                    changed = true;
                }
                if preset_button(ui, "演出") {
                    params.offset_px = 3.0;
                    changed = true;
                }
            });
            changed |= ui
                .add(egui::Slider::new(&mut params.offset_px, 0.0..=24.0).text("ずれ(px)"))
                .changed();
        }
        LocalEffect::Halftone(params) => {
            ui.label(egui::RichText::new("プリセット").color(Color32::from_gray(190)));
            ui.horizontal_wrapped(|ui| {
                if preset_button(ui, "細かい") {
                    *params = HalftoneParams {
                        cell_px: 6,
                        strength: 0.35,
                    };
                    changed = true;
                }
                if preset_button(ui, "漫画風") {
                    *params = HalftoneParams {
                        cell_px: 10,
                        strength: 0.70,
                    };
                    changed = true;
                }
            });
            ui.label(
                egui::RichText::new(
                    "印刷網点風の演出です。背景や効果線、漫画調の質感付け向けです。",
                )
                .size(10.0)
                .color(Color32::from_gray(170)),
            );
            let mut cell = params.cell_px as i32;
            changed |= ui
                .add(egui::Slider::new(&mut cell, 2..=96).text("セル(px)"))
                .changed();
            params.cell_px = cell.max(2) as u32;
            changed |= ui
                .add(egui::Slider::new(&mut params.strength, 0.0..=1.0).text("強度"))
                .changed();
        }
        LocalEffect::StarGlow(params) => {
            ui.label(egui::RichText::new("プリセット").color(Color32::from_gray(190)));
            ui.horizontal_wrapped(|ui| {
                if preset_button(ui, "クロス弱") {
                    *params = StarGlowParams {
                        ray_count: 4,
                        rotation_degrees: 0.0,
                        threshold: 0.997,
                        length_px: 36.0,
                        strength: 0.45,
                    };
                    changed = true;
                }
                if preset_button(ui, "クロス強") {
                    *params = StarGlowParams {
                        ray_count: 4,
                        rotation_degrees: 0.0,
                        threshold: 0.993,
                        length_px: 72.0,
                        strength: 0.80,
                    };
                    changed = true;
                }
                if preset_button(ui, "X字") {
                    *params = StarGlowParams {
                        ray_count: 4,
                        rotation_degrees: 45.0,
                        threshold: 0.995,
                        length_px: 64.0,
                        strength: 0.75,
                    };
                    changed = true;
                }
                if preset_button(ui, "6本") {
                    *params = StarGlowParams {
                        ray_count: 6,
                        rotation_degrees: 0.0,
                        threshold: 0.996,
                        length_px: 56.0,
                        strength: 0.70,
                    };
                    changed = true;
                }
                if preset_button(ui, "8本") {
                    *params = StarGlowParams {
                        ray_count: 8,
                        rotation_degrees: 0.0,
                        threshold: 0.997,
                        length_px: 56.0,
                        strength: 0.65,
                    };
                    changed = true;
                }
            });
            ui.label(
                egui::RichText::new("明るい点を抽出し、レンズのクロス/スター光条風に伸ばします。")
                    .size(10.0)
                    .color(Color32::from_gray(170)),
            );
            let mut ray_count = params.ray_count as i32;
            changed |= ui
                .add(egui::Slider::new(&mut ray_count, 2..=12).text("光線本数"))
                .changed();
            if changed {
                let mut normalized = ray_count.clamp(2, 12) as u32;
                if normalized % 2 != 0 {
                    normalized += 1;
                }
                params.ray_count = normalized.clamp(2, 12);
            }
            changed |= ui
                .add(egui::Slider::new(&mut params.rotation_degrees, -180.0..=180.0).text("回転"))
                .changed();
            changed |= ui
                .add(
                    egui::Slider::new(&mut params.threshold, 0.90..=0.9999)
                        .text("明部しきい値")
                        .fixed_decimals(4)
                        .smart_aim(false),
                )
                .changed();
            changed |= ui
                .add(egui::Slider::new(&mut params.length_px, 1.0..=240.0).text("光線長"))
                .changed();
            changed |= ui
                .add(egui::Slider::new(&mut params.strength, 0.0..=3.0).text("強さ"))
                .changed();
        }
        LocalEffect::EdgeSmooth(params) => {
            ui.label(egui::RichText::new("プリセット").color(Color32::from_gray(190)));
            ui.horizontal_wrapped(|ui| {
                if preset_button(ui, "背景なじませ") {
                    *params = EdgeSmoothParams {
                        radius_px: 3.0,
                        strength: 0.35,
                        edge_threshold: 28.0,
                    };
                    changed = true;
                }
                if preset_button(ui, "強め") {
                    *params = EdgeSmoothParams {
                        radius_px: 5.0,
                        strength: 0.55,
                        edge_threshold: 45.0,
                    };
                    changed = true;
                }
            });
            changed |= ui
                .add(egui::Slider::new(&mut params.radius_px, 0.0..=8.0).text("半径"))
                .changed();
            changed |= ui
                .add(egui::Slider::new(&mut params.strength, 0.0..=1.0).text("強さ"))
                .changed();
            changed |= ui
                .add(
                    egui::Slider::new(&mut params.edge_threshold, 1.0..=120.0).text("境界しきい値"),
                )
                .changed();
        }
    }
    changed
}

fn layer_with_mask(
    name: impl Into<String>,
    mask_kind: MaskKind,
    width: usize,
    height: usize,
) -> LocalAdjustmentLayer {
    LocalAdjustmentLayer::new(
        name,
        default_mask(mask_kind, width, height),
        LocalEffect::None,
    )
}

fn default_mask(kind: MaskKind, width: usize, height: usize) -> LocalMask {
    match kind {
        MaskKind::Full => LocalMask::Full,
        MaskKind::Raster => LocalMask::RasterVector(RasterVectorMask::empty(width, height)),
        MaskKind::LinearGradient => LocalMask::LinearGradient(LinearGradientMask::default()),
        MaskKind::RadialGradient => LocalMask::RadialGradient(RadialGradientMask::default()),
        MaskKind::LumaRange => LocalMask::LumaRange(RangeMask::default()),
        MaskKind::ColorRange => LocalMask::ColorRange(ColorRangeMask::default()),
        MaskKind::Subject => LocalMask::Subject(RasterMask::empty(width, height)),
        MaskKind::Segmentation => LocalMask::Segmentation(RegionMask::empty(width, height)),
    }
}

fn copy_mask_fields_from_layer(target: &mut LocalAdjustmentLayer, source: &LocalAdjustmentLayer) {
    target.mask = source.mask.clone();
    target.manual_override = source.manual_override.clone();
    target.mask_inverted = source.mask_inverted;
    target.mask_expand_px = source.mask_expand_px;
    target.mask_feather_px = source.mask_feather_px;
}

fn make_shape(
    tool: MaskTool,
    start: [f32; 2],
    end: [f32; 2],
    line_width: f32,
    dims: Option<(usize, usize)>,
) -> Option<MaskShape> {
    let dx = end[0] - start[0];
    let dy = end[1] - start[1];
    match tool {
        MaskTool::Line => {
            if dx * dx + dy * dy <= 4.0 {
                return None;
            }
            Some(MaskShape::Line {
                op: ShapeOp::Add,
                kind: LineKind::Diagonal,
                p0: start,
                p1: end,
                thickness: line_width.max(1.0),
            })
        }
        MaskTool::VertLine => {
            let (_w, h) = dims?;
            let lx = start[0].min(end[0]);
            let rx = start[0].max(end[0]);
            let thickness = (rx - lx).max(1.0);
            let cx = (lx + rx) * 0.5;
            Some(MaskShape::Line {
                op: ShapeOp::Add,
                kind: LineKind::Vertical,
                p0: [cx, 0.0],
                p1: [cx, h as f32],
                thickness,
            })
        }
        MaskTool::HorizLine => {
            let (w, _h) = dims?;
            let ty = start[1].min(end[1]);
            let by = start[1].max(end[1]);
            let thickness = (by - ty).max(1.0);
            let cy = (ty + by) * 0.5;
            Some(MaskShape::Line {
                op: ShapeOp::Add,
                kind: LineKind::Horizontal,
                p0: [0.0, cy],
                p1: [w as f32, cy],
                thickness,
            })
        }
        MaskTool::Rect => {
            if dx.abs() <= 1.0 || dy.abs() <= 1.0 {
                return None;
            }
            Some(MaskShape::Rect {
                op: ShapeOp::Add,
                center: [(start[0] + end[0]) * 0.5, (start[1] + end[1]) * 0.5],
                half_w: dx.abs() * 0.5,
                half_h: dy.abs() * 0.5,
                rotation_rad: 0.0,
            })
        }
        MaskTool::Ellipse => {
            if dx.abs() <= 1.0 || dy.abs() <= 1.0 {
                return None;
            }
            Some(MaskShape::Ellipse {
                op: ShapeOp::Add,
                center: [(start[0] + end[0]) * 0.5, (start[1] + end[1]) * 0.5],
                rx: dx.abs() * 0.5,
                ry: dy.abs() * 0.5,
                rotation_rad: 0.0,
            })
        }
        _ => None,
    }
}

fn apply_shape_drag(drag: ShapeDrag, cur: [f32; 2], modifiers: egui::Modifiers) -> MaskShape {
    let dx = cur[0] - drag.origin[0];
    let dy = cur[1] - drag.origin[1];
    match (drag.base, drag.handle) {
        (shape, ShapeHandle::Body) => translate_shape(shape, dx, dy),
        (
            MaskShape::Line {
                op,
                kind,
                p0: _,
                p1,
                thickness,
            },
            ShapeHandle::LineStart,
        ) => MaskShape::Line {
            op,
            kind,
            p0: constrain_line_endpoint(cur, p1, modifiers),
            p1,
            thickness,
        },
        (
            MaskShape::Line {
                op,
                kind,
                p0,
                p1: _,
                thickness,
            },
            ShapeHandle::LineEnd,
        ) => MaskShape::Line {
            op,
            kind,
            p0,
            p1: constrain_line_endpoint(cur, p0, modifiers),
            thickness,
        },
        (
            MaskShape::Rect {
                op,
                center,
                half_w,
                half_h,
                rotation_rad,
            },
            ShapeHandle::Corner(corner),
        ) => resize_axis_rect(
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
            MaskShape::Ellipse {
                op,
                center,
                rx,
                ry,
                rotation_rad,
            },
            ShapeHandle::Corner(corner),
        ) => resize_axis_ellipse(op, center, rx, ry, rotation_rad, corner, cur, modifiers)
            .unwrap_or(drag.base),
        (
            MaskShape::Ellipse {
                op,
                center,
                rx: _,
                ry,
                rotation_rad,
            },
            ShapeHandle::Radius,
        ) => {
            let nrx = (cur[0] - center[0]).abs().max(1.0);
            let nry = if modifiers.shift { nrx } else { ry };
            MaskShape::Ellipse {
                op,
                center,
                rx: nrx,
                ry: nry,
                rotation_rad,
            }
        }
        _ => drag.base,
    }
}

fn translate_shape(shape: MaskShape, dx: f32, dy: f32) -> MaskShape {
    match shape {
        MaskShape::Line {
            op,
            kind,
            p0,
            p1,
            thickness,
        } => MaskShape::Line {
            op,
            kind,
            p0: [p0[0] + dx, p0[1] + dy],
            p1: [p1[0] + dx, p1[1] + dy],
            thickness,
        },
        MaskShape::Rect {
            op,
            center,
            half_w,
            half_h,
            rotation_rad,
        } => MaskShape::Rect {
            op,
            center: [center[0] + dx, center[1] + dy],
            half_w,
            half_h,
            rotation_rad,
        },
        MaskShape::Ellipse {
            op,
            center,
            rx,
            ry,
            rotation_rad,
        } => MaskShape::Ellipse {
            op,
            center: [center[0] + dx, center[1] + dy],
            rx,
            ry,
            rotation_rad,
        },
    }
}

fn rotate_shape(shape: MaskShape, delta_rad: f32, snap_15deg: bool) -> MaskShape {
    let snap = |angle: f32| {
        if snap_15deg {
            let step = 15.0_f32.to_radians();
            (angle / step).round() * step
        } else {
            angle
        }
    };
    match shape {
        MaskShape::Line {
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
            MaskShape::Line {
                op,
                kind,
                p0: [center[0] - c * half_len, center[1] - s * half_len],
                p1: [center[0] + c * half_len, center[1] + s * half_len],
                thickness,
            }
        }
        MaskShape::Rect {
            op,
            center,
            half_w,
            half_h,
            rotation_rad,
        } => MaskShape::Rect {
            op,
            center,
            half_w,
            half_h,
            rotation_rad: snap(rotation_rad + delta_rad),
        },
        MaskShape::Ellipse {
            op,
            center,
            rx,
            ry,
            rotation_rad,
        } => MaskShape::Ellipse {
            op,
            center,
            rx,
            ry,
            rotation_rad: snap(rotation_rad + delta_rad),
        },
    }
}

fn constrain_line_endpoint(
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

fn resize_axis_rect(
    op: ShapeOp,
    center: [f32; 2],
    half_w: f32,
    half_h: f32,
    rotation_rad: f32,
    corner: u8,
    cur: [f32; 2],
    modifiers: egui::Modifiers,
) -> Option<MaskShape> {
    if rotation_rad.abs() > 0.001 {
        return None;
    }
    let corners = axis_corners(center, half_w, half_h);
    let anchor = corners[((corner as usize) + 2) % 4];
    let cx = (anchor[0] + cur[0]) * 0.5;
    let cy = (anchor[1] + cur[1]) * 0.5;
    let mut hw = (cur[0] - anchor[0]).abs() * 0.5;
    let mut hh = (cur[1] - anchor[1]).abs() * 0.5;
    if modifiers.shift {
        let m = hw.max(hh);
        hw = m;
        hh = m;
    }
    Some(MaskShape::Rect {
        op,
        center: [cx, cy],
        half_w: hw.max(1.0),
        half_h: hh.max(1.0),
        rotation_rad,
    })
}

fn resize_axis_ellipse(
    op: ShapeOp,
    center: [f32; 2],
    rx: f32,
    ry: f32,
    rotation_rad: f32,
    corner: u8,
    cur: [f32; 2],
    modifiers: egui::Modifiers,
) -> Option<MaskShape> {
    if rotation_rad.abs() > 0.001 {
        return None;
    }
    let corners = axis_corners(center, rx, ry);
    let anchor = corners[((corner as usize) + 2) % 4];
    let cx = (anchor[0] + cur[0]) * 0.5;
    let cy = (anchor[1] + cur[1]) * 0.5;
    let mut nrx = (cur[0] - anchor[0]).abs() * 0.5;
    let mut nry = (cur[1] - anchor[1]).abs() * 0.5;
    if modifiers.shift {
        let m = nrx.max(nry);
        nrx = m;
        nry = m;
    }
    Some(MaskShape::Ellipse {
        op,
        center: [cx, cy],
        rx: nrx.max(1.0),
        ry: nry.max(1.0),
        rotation_rad,
    })
}

fn hit_shape_handles(shape: MaskShape, p: [f32; 2]) -> Option<ShapeHandle> {
    let r2 = 12.0_f32.powi(2);
    match shape {
        MaskShape::Line { p0, p1, .. } => {
            if dist2(p, p0) <= r2 {
                Some(ShapeHandle::LineStart)
            } else if dist2(p, p1) <= r2 {
                Some(ShapeHandle::LineEnd)
            } else {
                None
            }
        }
        MaskShape::Rect {
            center,
            half_w,
            half_h,
            rotation_rad,
            ..
        } => axis_corners(center, half_w, half_h)
            .iter()
            .enumerate()
            .find(|&(_, &c)| dist2(p, c) <= r2 && rotation_rad.abs() < 0.001)
            .map(|(i, _)| ShapeHandle::Corner(i as u8)),
        MaskShape::Ellipse {
            center,
            rx,
            ry,
            rotation_rad,
            ..
        } => {
            let handle = [center[0] + rx, center[1]];
            if dist2(p, handle) <= r2 && rotation_rad.abs() < 0.001 {
                Some(ShapeHandle::Radius)
            } else {
                axis_corners(center, rx, ry)
                    .iter()
                    .enumerate()
                    .find(|&(_, &c)| dist2(p, c) <= r2 && rotation_rad.abs() < 0.001)
                    .map(|(i, _)| ShapeHandle::Corner(i as u8))
            }
        }
    }
}

fn shape_contains(shape: MaskShape, p: [f32; 2]) -> bool {
    match shape {
        MaskShape::Line {
            p0, p1, thickness, ..
        } => distance_to_segment(p, p0, p1) <= thickness * 0.5 + 3.0,
        MaskShape::Rect {
            center,
            half_w,
            half_h,
            rotation_rad,
            ..
        } => {
            let local = inverse_rotate_point(p, center, rotation_rad);
            (local[0] - center[0]).abs() <= half_w && (local[1] - center[1]).abs() <= half_h
        }
        MaskShape::Ellipse {
            center,
            rx,
            ry,
            rotation_rad,
            ..
        } => {
            let local = inverse_rotate_point(p, center, rotation_rad);
            ((local[0] - center[0]) / rx).powi(2) + ((local[1] - center[1]) / ry).powi(2) <= 1.0
        }
    }
}

fn draw_shape_outline(
    painter: &egui::Painter,
    shape: MaskShape,
    to_screen: &impl Fn([f32; 2]) -> Pos2,
    color: Color32,
    selected: bool,
) {
    let stroke = egui::Stroke::new(if selected { 2.0 } else { 1.3 }, color);
    match shape {
        MaskShape::Line { p0, p1, .. } => {
            painter.line_segment([to_screen(p0), to_screen(p1)], stroke);
        }
        MaskShape::Rect {
            center,
            half_w,
            half_h,
            rotation_rad,
            ..
        } => {
            let pts: Vec<Pos2> = rotated_corners(center, half_w, half_h, rotation_rad)
                .into_iter()
                .map(to_screen)
                .collect();
            painter.add(egui::Shape::closed_line(pts, stroke));
        }
        MaskShape::Ellipse {
            center,
            rx,
            ry,
            rotation_rad,
            ..
        } => {
            let mut pts = Vec::with_capacity(65);
            for i in 0..64 {
                let t = i as f32 / 64.0 * std::f32::consts::TAU;
                let (s, c) = rotation_rad.sin_cos();
                let x = rx * t.cos();
                let y = ry * t.sin();
                pts.push(to_screen([
                    center[0] + x * c - y * s,
                    center[1] + x * s + y * c,
                ]));
            }
            painter.add(egui::Shape::closed_line(pts, stroke));
        }
    }
}

fn draw_shape_handles(
    painter: &egui::Painter,
    shape: MaskShape,
    to_screen: &impl Fn([f32; 2]) -> Pos2,
) {
    let fill = Color32::from_rgb(255, 250, 210);
    let stroke = egui::Stroke::new(2.0, Color32::from_rgb(35, 25, 10));
    let draw = |p: [f32; 2]| {
        let sp = to_screen(p);
        painter.circle_filled(sp, 5.5, fill);
        painter.circle_stroke(sp, 5.5, stroke);
    };
    match shape {
        MaskShape::Line { p0, p1, .. } => {
            draw(p0);
            draw(p1);
        }
        MaskShape::Rect {
            center,
            half_w,
            half_h,
            rotation_rad,
            ..
        } => {
            for p in rotated_corners(center, half_w, half_h, rotation_rad) {
                draw(p);
            }
        }
        MaskShape::Ellipse {
            center,
            rx,
            ry,
            rotation_rad,
            ..
        } => {
            for p in rotated_corners(center, rx, ry, rotation_rad) {
                draw(p);
            }
            draw([center[0] + rx, center[1]]);
        }
    }
}

fn edge_strength_at(image: &RgbaImageBuf, x: usize, y: usize) -> f32 {
    let xm = x.saturating_sub(1);
    let xp = (x + 1).min(image.width.saturating_sub(1));
    let ym = y.saturating_sub(1);
    let yp = (y + 1).min(image.height.saturating_sub(1));
    let left = luma_at(image, xm, y);
    let right = luma_at(image, xp, y);
    let top = luma_at(image, x, ym);
    let bottom = luma_at(image, x, yp);
    ((right - left).powi(2) + (bottom - top).powi(2)).sqrt()
}

fn line_interior_strength_at(image: &RgbaImageBuf, x: usize, y: usize) -> f32 {
    if image.width == 0 || image.height == 0 {
        return 0.0;
    }
    let center = luma_at(image, x, y);
    let radius = 3_isize;
    let mut best = 0.0_f32;
    for (dx, dy) in [(1, 0), (0, 1), (1, 1), (1, -1)] {
        let Some(a) = luma_offset(image, x, y, dx * radius, dy * radius) else {
            continue;
        };
        let Some(b) = luma_offset(image, x, y, -dx * radius, -dy * radius) else {
            continue;
        };
        let dark_line = (a - center).min(b - center);
        let bright_line = (center - a).min(center - b);
        best = best.max(dark_line.max(bright_line).max(0.0));
    }
    best
}

fn boundary_strength_at(image: &RgbaImageBuf, x: usize, y: usize) -> f32 {
    edge_strength_at(image, x, y).max(line_interior_strength_at(image, x, y))
}

fn raw_boundary_pixel_at(
    image: &RgbaImageBuf,
    x: usize,
    y: usize,
    edge_threshold: f32,
    ink_threshold: f32,
) -> bool {
    edge_strength_at(image, x, y) >= edge_threshold
        || line_interior_strength_at(image, x, y) >= ink_threshold
}

fn boundary_pixel_at(
    image: &RgbaImageBuf,
    x: usize,
    y: usize,
    edge_threshold: f32,
    ink_threshold: f32,
    gap_px: usize,
) -> bool {
    raw_boundary_pixel_at(image, x, y, edge_threshold, ink_threshold)
        || (gap_px > 0
            && boundary_gap_bridge_at(image, x, y, edge_threshold, ink_threshold, gap_px))
}

fn build_boundary_mask(
    image: &RgbaImageBuf,
    edge_threshold: u8,
    ink_threshold: u8,
    gap_px: usize,
) -> Vec<u8> {
    let edge_threshold = edge_threshold as f32;
    let ink_threshold = ink_threshold as f32;
    let len = image.width.saturating_mul(image.height);
    let mut raw = vec![0_u8; len];
    for y in 0..image.height {
        for x in 0..image.width {
            if raw_boundary_pixel_at(image, x, y, edge_threshold, ink_threshold) {
                raw[y * image.width + x] = 1;
            }
        }
    }
    if gap_px == 0 {
        return raw;
    }
    let mut bridged = raw.clone();
    for y in 0..image.height {
        for x in 0..image.width {
            let idx = y * image.width + x;
            if raw[idx] == 0
                && boundary_gap_bridge_in_mask(&raw, image.width, image.height, x, y, gap_px)
            {
                bridged[idx] = 1;
            }
        }
    }
    bridged
}

fn boundary_mask_at(mask: &[u8], width: usize, x: usize, y: usize) -> bool {
    mask.get(y.saturating_mul(width).saturating_add(x))
        .copied()
        .unwrap_or(0)
        != 0
}

fn boundary_gap_bridge_in_mask(
    raw: &[u8],
    width: usize,
    height: usize,
    x: usize,
    y: usize,
    max_gap: usize,
) -> bool {
    for ((dx0, dy0), (dx1, dy1)) in [
        ((-1, 0), (1, 0)),
        ((0, -1), (0, 1)),
        ((-1, -1), (1, 1)),
        ((-1, 1), (1, -1)),
    ] {
        let a = nearest_boundary_distance_in_mask(raw, width, height, x, y, dx0, dy0, max_gap);
        let b = nearest_boundary_distance_in_mask(raw, width, height, x, y, dx1, dy1, max_gap);
        if let (Some(a), Some(b)) = (a, b)
            && a + b <= max_gap + 1
        {
            return true;
        }
    }
    false
}

fn nearest_boundary_distance_in_mask(
    raw: &[u8],
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
        if boundary_mask_at(raw, width, nx as usize, ny as usize) {
            return Some(step);
        }
    }
    None
}

fn boundary_gap_bridge_at(
    image: &RgbaImageBuf,
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
        let a = nearest_boundary_distance(
            image,
            x,
            y,
            dx0,
            dy0,
            edge_threshold,
            ink_threshold,
            max_gap,
        );
        let b = nearest_boundary_distance(
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

fn nearest_boundary_distance(
    image: &RgbaImageBuf,
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
        if nx < 0 || ny < 0 || nx >= image.width as isize || ny >= image.height as isize {
            return None;
        }
        if raw_boundary_pixel_at(
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

fn include_adjacent_boundary_pixels(
    image: &RgbaImageBuf,
    targets: &mut Vec<usize>,
    target_map: &mut [bool],
    min_x: usize,
    max_x: usize,
    min_y: usize,
    max_y: usize,
    bw: usize,
    center: Pos2,
    radius_sq: f32,
    boundary_mask: &[u8],
) {
    if bw == 0 {
        return;
    }
    let initial_len = targets.len();
    let include_radius = EDGE_BRUSH_INCLUDE_BOUNDARY_RADIUS as isize;
    let include_radius_sq = include_radius * include_radius;
    for i in 0..initial_len {
        let src_idx = targets[i];
        let x = src_idx % image.width;
        let y = src_idx / image.width;
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
                let brush_dx = nx as f32 + 0.5 - center.x;
                let brush_dy = ny as f32 + 0.5 - center.y;
                if brush_dx * brush_dx + brush_dy * brush_dy > radius_sq {
                    continue;
                }
                if boundary_mask_at(boundary_mask, image.width, nx, ny) {
                    target_map[local_idx] = true;
                    targets.push(ny * image.width + nx);
                }
            }
        }
    }
}

fn edge_brush_pixel_allowed(
    image: &RgbaImageBuf,
    boundary_mask: &[u8],
    x: usize,
    y: usize,
    seed: [u8; 3],
    tolerance: i16,
) -> bool {
    let px = (y * image.width + x) * 4;
    let rgb = [image.pixels[px], image.pixels[px + 1], image.pixels[px + 2]];
    let max_delta = seed
        .iter()
        .zip(rgb)
        .map(|(&a, b)| (a as i16 - b as i16).abs())
        .max()
        .unwrap_or(0);
    max_delta <= tolerance && !boundary_mask_at(boundary_mask, image.width, x, y)
}

fn gap_between_masked_pixels(
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
    let left = nearest_mask_distance(alpha, width, height, x, y, -1, 0, max_gap);
    let right = nearest_mask_distance(alpha, width, height, x, y, 1, 0, max_gap);
    if let (Some(l), Some(r)) = (left, right)
        && l + r <= max_gap + 1
    {
        return true;
    }
    let up = nearest_mask_distance(alpha, width, height, x, y, 0, -1, max_gap);
    let down = nearest_mask_distance(alpha, width, height, x, y, 0, 1, max_gap);
    if let (Some(u), Some(d)) = (up, down)
        && u + d <= max_gap + 1
    {
        return true;
    }
    false
}

fn nearest_mask_distance(
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

fn edge_preview_size(width: usize, height: usize) -> [usize; 2] {
    const MAX_EDGE_PREVIEW_SIDE: usize = 640;
    let max_side = width.max(height).max(1);
    if max_side <= MAX_EDGE_PREVIEW_SIDE {
        [width.max(1), height.max(1)]
    } else {
        let scale = MAX_EDGE_PREVIEW_SIDE as f32 / max_side as f32;
        [
            ((width as f32 * scale).round() as usize).max(1),
            ((height as f32 * scale).round() as usize).max(1),
        ]
    }
}

fn build_edge_preview_image(
    image: &RgbaImageBuf,
    boundary_mask: &[u8],
    preview_size: [usize; 2],
    threshold: u8,
    ink_threshold: u8,
) -> ColorImage {
    let [pw, ph] = preview_size;
    let mut pixels = vec![Color32::TRANSPARENT; pw.saturating_mul(ph)];
    if image.width == 0 || image.height == 0 || pw == 0 || ph == 0 {
        return ColorImage::new([pw.max(1), ph.max(1)], pixels);
    }
    let threshold = threshold as f32;
    let ink_threshold = ink_threshold as f32;
    for py in 0..ph {
        let sy = ((py as f32 + 0.5) * image.height as f32 / ph as f32)
            .floor()
            .clamp(0.0, image.height.saturating_sub(1) as f32) as usize;
        for px in 0..pw {
            let sx = ((px as f32 + 0.5) * image.width as f32 / pw as f32)
                .floor()
                .clamp(0.0, image.width.saturating_sub(1) as f32) as usize;
            if boundary_mask_at(boundary_mask, image.width, sx, sy) {
                let strength = boundary_strength_at(image, sx, sy);
                let base_threshold = threshold.min(ink_threshold);
                let alpha = ((strength - base_threshold) / 96.0)
                    .clamp(0.18, 1.0)
                    .mul_add(180.0, 0.0)
                    .round() as u8;
                pixels[py * pw + px] = Color32::from_rgba_unmultiplied(255, 255, 255, alpha);
            }
        }
    }
    ColorImage::new([pw, ph], pixels)
}

fn luma_at(image: &RgbaImageBuf, x: usize, y: usize) -> f32 {
    let idx = (y * image.width + x) * 4;
    let r = image.pixels[idx] as f32;
    let g = image.pixels[idx + 1] as f32;
    let b = image.pixels[idx + 2] as f32;
    r * 0.299 + g * 0.587 + b * 0.114
}

fn luma_offset(image: &RgbaImageBuf, x: usize, y: usize, dx: isize, dy: isize) -> Option<f32> {
    let nx = x as isize + dx;
    let ny = y as isize + dy;
    if nx < 0 || ny < 0 || nx >= image.width as isize || ny >= image.height as isize {
        return None;
    }
    Some(luma_at(image, nx as usize, ny as usize))
}

fn dilate_alpha(src: &[f32], width: usize, height: usize) -> Vec<f32> {
    morph_alpha(src, width, height, true)
}

fn erode_alpha(src: &[f32], width: usize, height: usize) -> Vec<f32> {
    morph_alpha(src, width, height, false)
}

fn morph_alpha(src: &[f32], width: usize, height: usize, dilate: bool) -> Vec<f32> {
    if width == 0 || height == 0 {
        return src.to_vec();
    }
    let mut out = vec![0.0; src.len()];
    for y in 0..height {
        for x in 0..width {
            let mut v = if dilate { 0.0_f32 } else { 1.0_f32 };
            let y0 = y.saturating_sub(1);
            let y1 = (y + 1).min(height - 1);
            let x0 = x.saturating_sub(1);
            let x1 = (x + 1).min(width - 1);
            for yy in y0..=y1 {
                for xx in x0..=x1 {
                    let sample = src[yy * width + xx];
                    if dilate {
                        v = v.max(sample);
                    } else {
                        v = v.min(sample);
                    }
                }
            }
            out[y * width + x] = v;
        }
    }
    out
}

fn push_freehand_point(points: &mut Vec<[f32; 2]>, point: [f32; 2]) -> bool {
    if points
        .last()
        .map(|last| distance_sq(*last, point) > FREEHAND_MIN_DISTANCE_SQ)
        .unwrap_or(true)
    {
        points.push(point);
        true
    } else {
        false
    }
}

fn should_close_polygon(points: &[[f32; 2]], point: [f32; 2], image_to_screen_scale: f32) -> bool {
    if points.len() < 3 {
        return false;
    }
    let scale = image_to_screen_scale.max(0.001);
    distance_sq(points[0], point) * scale * scale <= POLYGON_CLOSE_RADIUS_PX.powi(2)
}

fn push_polygon_vertex(
    points: &mut Vec<[f32; 2]>,
    point: [f32; 2],
    image_to_screen_scale: f32,
) -> bool {
    let scale = image_to_screen_scale.max(0.001);
    let min_dist_sq = (POLYGON_VERTEX_MIN_DISTANCE_PX / scale).powi(2);
    if points
        .last()
        .map(|last| distance_sq(*last, point) > min_dist_sq)
        .unwrap_or(true)
    {
        points.push(point);
        true
    } else {
        false
    }
}

fn distance_sq(a: [f32; 2], b: [f32; 2]) -> f32 {
    let dx = a[0] - b[0];
    let dy = a[1] - b[1];
    dx * dx + dy * dy
}

fn fill_polygon_alpha(
    alpha: &mut [f32],
    width: usize,
    height: usize,
    points: &[[f32; 2]],
    add: bool,
    value: f32,
) {
    if points.len() < 3 || width == 0 || height == 0 {
        return;
    }
    let min_x = points
        .iter()
        .map(|p| p[0])
        .fold(f32::INFINITY, f32::min)
        .floor()
        .max(0.0) as usize;
    let max_x = points
        .iter()
        .map(|p| p[0])
        .fold(f32::NEG_INFINITY, f32::max)
        .ceil()
        .min(width as f32 - 1.0) as usize;
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
        .min(height as f32 - 1.0) as usize;
    for y in min_y..=max_y {
        for x in min_x..=max_x {
            if point_in_polygon([x as f32 + 0.5, y as f32 + 0.5], points) {
                alpha[y * width + x] = if add { value.clamp(0.0, 1.0) } else { 0.0 };
            }
        }
    }
}

fn point_in_polygon(p: [f32; 2], points: &[[f32; 2]]) -> bool {
    let mut inside = false;
    let mut j = points.len() - 1;
    for i in 0..points.len() {
        let pi = points[i];
        let pj = points[j];
        let dy = pj[1] - pi[1];
        if ((pi[1] > p[1]) != (pj[1] > p[1]))
            && dy.abs() > 1e-6
            && (p[0] < (pj[0] - pi[0]) * (p[1] - pi[1]) / dy + pi[0])
        {
            inside = !inside;
        }
        j = i;
    }
    inside
}

fn axis_corners(center: [f32; 2], half_w: f32, half_h: f32) -> [[f32; 2]; 4] {
    [
        [center[0] - half_w, center[1] - half_h],
        [center[0] + half_w, center[1] - half_h],
        [center[0] + half_w, center[1] + half_h],
        [center[0] - half_w, center[1] + half_h],
    ]
}

fn rotated_corners(center: [f32; 2], half_w: f32, half_h: f32, rotation_rad: f32) -> [[f32; 2]; 4] {
    let (s, c) = rotation_rad.sin_cos();
    axis_corners([0.0, 0.0], half_w, half_h).map(|p| {
        [
            center[0] + p[0] * c - p[1] * s,
            center[1] + p[0] * s + p[1] * c,
        ]
    })
}

fn inverse_rotate_point(p: [f32; 2], center: [f32; 2], rotation_rad: f32) -> [f32; 2] {
    let (s, c) = (-rotation_rad).sin_cos();
    let dx = p[0] - center[0];
    let dy = p[1] - center[1];
    [center[0] + dx * c - dy * s, center[1] + dx * s + dy * c]
}

fn dist2(a: [f32; 2], b: [f32; 2]) -> f32 {
    let dx = a[0] - b[0];
    let dy = a[1] - b[1];
    dx * dx + dy * dy
}

fn distance_to_segment(p: [f32; 2], a: [f32; 2], b: [f32; 2]) -> f32 {
    let ab = [b[0] - a[0], b[1] - a[1]];
    let ap = [p[0] - a[0], p[1] - a[1]];
    let denom = ab[0] * ab[0] + ab[1] * ab[1];
    let t = if denom <= f32::EPSILON {
        0.0
    } else {
        ((ap[0] * ab[0] + ap[1] * ab[1]) / denom).clamp(0.0, 1.0)
    };
    let closest = [a[0] + ab[0] * t, a[1] + ab[1] * t];
    dist2(p, closest).sqrt()
}

fn panel_toggle_button(
    ui: &mut egui::Ui,
    label: &str,
    active: bool,
    size: Option<egui::Vec2>,
    paint_mode_button: bool,
) -> egui::Response {
    let fill = if active {
        if paint_mode_button {
            Color32::from_rgb(130, 58, 58)
        } else {
            Color32::from_rgb(58, 96, 150)
        }
    } else {
        Color32::from_rgba_unmultiplied(70, 70, 70, 170)
    };
    let button = egui::Button::new(label).fill(fill);
    if let Some(size) = size {
        ui.add_sized(size, button)
    } else {
        ui.add(button)
    }
}

fn default_effect(kind: EffectKind) -> LocalEffect {
    match kind {
        EffectKind::None => LocalEffect::None,
        EffectKind::Tone => LocalEffect::Tone(ToneParams::default()),
        EffectKind::ToneCurve => LocalEffect::ToneCurve(ToneCurveParams::default()),
        EffectKind::Clarity => LocalEffect::Clarity(ClarityParams::default()),
        EffectKind::HighlightsShadows => {
            LocalEffect::HighlightsShadows(HighlightsShadowsParams::default())
        }
        EffectKind::Dehaze => LocalEffect::Dehaze(DehazeParams::default()),
        EffectKind::Blur => LocalEffect::Blur(BlurParams::default()),
        EffectKind::SoftFocus => LocalEffect::SoftFocus(SoftFocusParams::default()),
        EffectKind::Mosaic => LocalEffect::Mosaic(MosaicParams::default()),
        EffectKind::Sharpen => LocalEffect::Sharpen(SharpenParams::default()),
        EffectKind::Hsl => LocalEffect::Hsl(HslParams::default()),
        EffectKind::Look => LocalEffect::Look(LookParams::default()),
        EffectKind::Bloom => LocalEffect::Bloom(BloomParams::default()),
        EffectKind::Vignette => LocalEffect::Vignette(VignetteParams::default()),
        EffectKind::FilmGrain => LocalEffect::FilmGrain(FilmGrainParams::default()),
        EffectKind::ChromaticAberration => {
            LocalEffect::ChromaticAberration(ChromaticAberrationParams::default())
        }
        EffectKind::Halftone => LocalEffect::Halftone(HalftoneParams::default()),
        EffectKind::StarGlow => LocalEffect::StarGlow(StarGlowParams::default()),
        EffectKind::EdgeSmooth => LocalEffect::EdgeSmooth(EdgeSmoothParams::default()),
    }
}

fn run_u2netp_segmentation(source: &RgbaImageBuf, model_path: &Path) -> Result<RasterMask, String> {
    ensure_lab_ort_loaded()?;
    let input = build_u2netp_input(source)?;
    let input_tensor =
        ort::value::Tensor::from_array(input).map_err(|e| format!("Tensor creation: {e}"))?;

    let mut session = ort::session::Session::builder()
        .map_err(|e| format!("Session::builder: {e}"))?
        .with_optimization_level(ort::session::builder::GraphOptimizationLevel::Level3)
        .map_err(|e| format!("optimization_level: {e}"))?
        .with_intra_threads(4)
        .map_err(|e| format!("intra_threads: {e}"))?
        .commit_from_file(model_path)
        .map_err(|e| format!("model load {}: {e}", model_path.display()))?;

    let outputs = session
        .run(ort::inputs![input_tensor])
        .map_err(|e| format!("run: {e}"))?;
    let (shape, raw) = outputs[0]
        .try_extract_tensor::<f32>()
        .map_err(|e| format!("extract: {e}"))?;
    let dims: Vec<i64> = shape.iter().copied().collect();
    let (small_w, small_h) = output_mask_size(&dims, raw.len());
    let small_mask = normalized_u2netp_output(raw, small_w, small_h);
    let alpha = resize_mask_bilinear(&small_mask, small_w, small_h, source.width, source.height);
    Ok(RasterMask {
        width: source.width,
        height: source.height,
        alpha,
    })
}

fn build_region_segmentation(
    source: &RgbaImageBuf,
    subject: Option<&RasterMask>,
    color_tolerance: f32,
    min_area: usize,
    edge_threshold: u8,
    ink_threshold: u8,
    gap_px: usize,
) -> Result<RegionMask, String> {
    let len = source.width.saturating_mul(source.height);
    if source.pixels.len() != len.saturating_mul(4) {
        return Err("invalid source RGBA buffer".to_string());
    }
    if let Some(mask) = subject
        && (mask.width != source.width || mask.height != source.height || mask.alpha.len() != len)
    {
        return Err("subject mask size does not match image".to_string());
    }
    let boundary = build_boundary_mask(source, edge_threshold, ink_threshold, gap_px);
    let mut visited = vec![false; len];
    let mut labels = vec![0_u32; len];
    let mut label = 0_u32;
    let tol = color_tolerance.max(0.0);
    let min_area = min_area.max(1);
    let mut queue = VecDeque::new();
    let mut component = Vec::new();

    for start in 0..len {
        if visited[start] {
            continue;
        }
        if !region_seed_allowed(source, subject, &boundary, start) {
            visited[start] = true;
            continue;
        }
        let seed = source_rgb_at_index(source, start);
        visited[start] = true;
        queue.clear();
        component.clear();
        queue.push_back(start);
        while let Some(idx) = queue.pop_front() {
            component.push(idx);
            let x = idx % source.width;
            let y = idx / source.width;
            for (nx, ny) in region_neighbors(x, y, source.width, source.height) {
                let nidx = ny * source.width + nx;
                if visited[nidx] {
                    continue;
                }
                if !region_seed_allowed(source, subject, &boundary, nidx) {
                    visited[nidx] = true;
                    continue;
                }
                if region_color_close(seed, source_rgb_at_index(source, nidx), tol) {
                    visited[nidx] = true;
                    queue.push_back(nidx);
                }
            }
        }
        if component.len() >= min_area && (label as usize) < REGION_SEGMENT_MAX_LABELS {
            label += 1;
            for &idx in &component {
                labels[idx] = label;
            }
        }
    }

    let membership_allowed: Vec<bool> = (0..len)
        .map(|idx| region_membership_allowed(source, subject, idx))
        .collect();
    fill_unlabeled_region_pixels(
        &mut labels,
        source.width,
        source.height,
        &membership_allowed,
    );

    let selected = vec![false; label as usize + 1];
    Ok(RegionMask {
        width: source.width,
        height: source.height,
        labels,
        selected,
    })
}

fn region_seed_allowed(
    source: &RgbaImageBuf,
    subject: Option<&RasterMask>,
    boundary: &[u8],
    idx: usize,
) -> bool {
    if boundary.get(idx).copied().unwrap_or(0) != 0 {
        return false;
    }
    region_membership_allowed(source, subject, idx)
}

fn fill_unlabeled_region_pixels(labels: &mut [u32], width: usize, height: usize, allowed: &[bool]) {
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
        for (nx, ny) in region_neighbors(x, y, width, height) {
            let nidx = ny * width + nx;
            if nidx >= len || !allowed[nidx] || labels[nidx] != 0 {
                continue;
            }
            labels[nidx] = label;
            queue.push_back(nidx);
        }
    }
}

fn region_membership_allowed(
    source: &RgbaImageBuf,
    subject: Option<&RasterMask>,
    idx: usize,
) -> bool {
    if source.pixels.get(idx * 4 + 3).copied().unwrap_or(255) < 8 {
        return false;
    }
    subject
        .map(|mask| mask.alpha.get(idx).copied().unwrap_or(0.0) > 0.18)
        .unwrap_or(true)
}

fn source_rgb_at_index(source: &RgbaImageBuf, idx: usize) -> [u8; 3] {
    let i = idx * 4;
    [
        source.pixels.get(i).copied().unwrap_or(0),
        source.pixels.get(i + 1).copied().unwrap_or(0),
        source.pixels.get(i + 2).copied().unwrap_or(0),
    ]
}

fn region_color_close(a: [u8; 3], b: [u8; 3], tolerance: f32) -> bool {
    let max_delta = (a[0] as f32 - b[0] as f32)
        .abs()
        .max((a[1] as f32 - b[1] as f32).abs())
        .max((a[2] as f32 - b[2] as f32).abs());
    max_delta <= tolerance
}

fn region_neighbors(
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

fn ensure_lab_ort_loaded() -> Result<(), String> {
    static ORT_INIT: OnceLock<Result<(), String>> = OnceLock::new();
    match ORT_INIT.get_or_init(|| {
        let dll_path = lab_ort_dll_path();
        if !dll_path.is_file() {
            return Err(format!(
                "onnxruntime.dll が見つかりません: {}",
                dll_path.display()
            ));
        }
        ort::init_from(&dll_path)
            .map_err(|e| format!("ort::init_from: {e}"))?
            .commit();
        Ok(())
    }) {
        Ok(()) => Ok(()),
        Err(e) => Err(e.clone()),
    }
}

fn build_u2netp_input(source: &RgbaImageBuf) -> Result<ndarray::Array4<f32>, String> {
    let mut rgb = Vec::with_capacity(source.width.saturating_mul(source.height).saturating_mul(3));
    for px in source.pixels.chunks_exact(4) {
        rgb.extend_from_slice(&px[..3]);
    }
    let Some(rgb_image) = RgbImage::from_raw(source.width as u32, source.height as u32, rgb) else {
        return Err("invalid source RGB buffer".to_string());
    };
    let resized = image::imageops::resize(
        &rgb_image,
        U2NETP_INPUT_SIZE as u32,
        U2NETP_INPUT_SIZE as u32,
        FilterType::Triangle,
    );
    let mut input = ndarray::Array4::<f32>::zeros((1, 3, U2NETP_INPUT_SIZE, U2NETP_INPUT_SIZE));
    let mean = [0.485_f32, 0.456, 0.406];
    let std = [0.229_f32, 0.224, 0.225];
    for y in 0..U2NETP_INPUT_SIZE {
        for x in 0..U2NETP_INPUT_SIZE {
            let p = resized.get_pixel(x as u32, y as u32).0;
            for c in 0..3 {
                let v = p[c] as f32 / 255.0;
                input[[0, c, y, x]] = (v - mean[c]) / std[c];
            }
        }
    }
    Ok(input)
}

fn output_mask_size(shape: &[i64], raw_len: usize) -> (usize, usize) {
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
            U2NETP_INPUT_SIZE,
            raw_len.max(1).div_ceil(U2NETP_INPUT_SIZE),
        )
    }
}

fn normalized_u2netp_output(raw: &[f32], width: usize, height: usize) -> Vec<f32> {
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
        let v = values[idx];
        *slot = if range.is_finite() && range > 1.0e-6 {
            ((v - min_v) / range).clamp(0.0, 1.0)
        } else {
            v.clamp(0.0, 1.0)
        };
    }
    out
}

fn resize_mask_bilinear(
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

fn segmentation_model_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("models/u2netp.onnx")
}

fn lab_ort_dll_path() -> PathBuf {
    repo_root_from_lab_manifest()
        .join("vendor")
        .join("ort")
        .join("onnxruntime.dll")
}

fn repo_root_from_lab_manifest() -> PathBuf {
    let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    manifest
        .parent()
        .and_then(Path::parent)
        .map(Path::to_path_buf)
        .unwrap_or(manifest)
}

fn load_image(path: &Path) -> Result<LoadedImage, String> {
    let dyn_img = image::open(path).map_err(|e| e.to_string())?;
    let rgba = dyn_img.to_rgba8();
    let (w, h) = rgba.dimensions();
    let source =
        RgbaImageBuf::new(w as usize, h as usize, rgba.into_raw()).map_err(|e| e.to_string())?;
    Ok(LoadedImage {
        path: path.to_path_buf(),
        source,
    })
}

fn sidecar_path_for_image(path: &Path) -> PathBuf {
    let file_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("image");
    path.with_file_name(format!("{file_name}.miv"))
}

fn deflate_b64(bytes: &[u8]) -> Result<String, String> {
    let mut encoder = DeflateEncoder::new(Vec::new(), Compression::default());
    encoder.write_all(bytes).map_err(|e| e.to_string())?;
    let compressed = encoder.finish().map_err(|e| e.to_string())?;
    Ok(BASE64.encode(compressed))
}

fn inflate_b64(text: &str) -> Result<Vec<u8>, String> {
    let compressed = BASE64
        .decode(text.as_bytes())
        .map_err(|e| format!("base64 decode failed: {e}"))?;
    let mut decoder = DeflateDecoder::new(compressed.as_slice());
    let mut out = Vec::new();
    decoder.read_to_end(&mut out).map_err(|e| e.to_string())?;
    Ok(out)
}

fn pack_alpha_bits(alpha: &[f32]) -> Vec<u8> {
    let mut packed = vec![0u8; alpha.len().div_ceil(8)];
    for (idx, &value) in alpha.iter().enumerate() {
        if value >= 0.5 {
            packed[idx / 8] |= 1 << (7 - (idx % 8));
        }
    }
    packed
}

fn unpack_alpha_bits(bytes: &[u8], len: usize) -> Vec<f32> {
    let mut alpha = vec![0.0; len];
    for idx in 0..len {
        if bytes.get(idx / 8).copied().unwrap_or(0) & (1 << (7 - (idx % 8))) != 0 {
            alpha[idx] = 1.0;
        }
    }
    alpha
}

fn stored_raster_vector_from_mask(
    mask: &RasterVectorMask,
) -> Result<StoredRasterVectorMask, String> {
    let packed = pack_alpha_bits(&mask.alpha);
    Ok(StoredRasterVectorMask {
        width: mask.width,
        height: mask.height,
        bitmap_1bit_deflate_b64: deflate_b64(&packed)?,
        shapes: mask.shapes.clone(),
    })
}

fn raster_vector_from_stored(stored: &StoredRasterVectorMask) -> Result<RasterVectorMask, String> {
    let len = stored.width.saturating_mul(stored.height);
    let packed = inflate_b64(&stored.bitmap_1bit_deflate_b64)?;
    Ok(RasterVectorMask {
        width: stored.width,
        height: stored.height,
        alpha: unpack_alpha_bits(&packed, len),
        shapes: stored.shapes.clone(),
    })
}

fn stored_soft_mask_from_mask(mask: &RasterMask) -> Result<StoredSoftMask, String> {
    let alpha_u8: Vec<u8> = mask
        .alpha
        .iter()
        .map(|v| (v.clamp(0.0, 1.0) * 255.0).round() as u8)
        .collect();
    Ok(StoredSoftMask {
        width: mask.width,
        height: mask.height,
        alpha_u8_deflate_b64: deflate_b64(&alpha_u8)?,
    })
}

fn soft_mask_from_stored(stored: &StoredSoftMask) -> Result<RasterMask, String> {
    let len = stored.width.saturating_mul(stored.height);
    let bytes = inflate_b64(&stored.alpha_u8_deflate_b64)?;
    if bytes.len() < len {
        return Err("soft mask payload is shorter than expected".to_string());
    }
    Ok(RasterMask {
        width: stored.width,
        height: stored.height,
        alpha: bytes[..len].iter().map(|&v| v as f32 / 255.0).collect(),
    })
}

fn stored_region_mask_from_mask(mask: &RegionMask) -> Result<StoredRegionMask, String> {
    let mut bytes = Vec::with_capacity(mask.labels.len().saturating_mul(4));
    for &label in &mask.labels {
        bytes.extend_from_slice(&label.to_le_bytes());
    }
    Ok(StoredRegionMask {
        width: mask.width,
        height: mask.height,
        labels_u32le_deflate_b64: deflate_b64(&bytes)?,
        selected: mask.selected.clone(),
    })
}

fn region_mask_from_stored(stored: &StoredRegionMask) -> Result<RegionMask, String> {
    let len = stored.width.saturating_mul(stored.height);
    let bytes = inflate_b64(&stored.labels_u32le_deflate_b64)?;
    if bytes.len() < len.saturating_mul(4) {
        return Err("region label payload is shorter than expected".to_string());
    }
    let labels = bytes[..len * 4]
        .chunks_exact(4)
        .map(|chunk| u32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]))
        .collect();
    Ok(RegionMask {
        width: stored.width,
        height: stored.height,
        labels,
        selected: stored.selected.clone(),
    })
}

fn stored_manual_override_from_layer(
    manual_override: &ManualMaskOverride,
) -> Result<StoredManualOverride, String> {
    Ok(StoredManualOverride {
        add: manual_override
            .add
            .as_ref()
            .map(stored_raster_vector_from_mask)
            .transpose()?,
        subtract: manual_override
            .subtract
            .as_ref()
            .map(stored_raster_vector_from_mask)
            .transpose()?,
    })
}

fn manual_override_from_stored(
    stored: &StoredManualOverride,
) -> Result<ManualMaskOverride, String> {
    Ok(ManualMaskOverride {
        add: stored
            .add
            .as_ref()
            .map(raster_vector_from_stored)
            .transpose()?,
        subtract: stored
            .subtract
            .as_ref()
            .map(raster_vector_from_stored)
            .transpose()?,
    })
}

fn stored_mask_from_local(mask: &LocalMask) -> Result<StoredMask, String> {
    Ok(match mask {
        LocalMask::Full => StoredMask::Full,
        LocalMask::Raster(mask) => {
            StoredMask::Raster(stored_raster_vector_from_mask(&RasterVectorMask {
                width: mask.width,
                height: mask.height,
                alpha: mask.alpha.clone(),
                shapes: Vec::new(),
            })?)
        }
        LocalMask::RasterVector(mask) => StoredMask::Raster(stored_raster_vector_from_mask(mask)?),
        LocalMask::LinearGradient(mask) => StoredMask::LinearGradient(*mask),
        LocalMask::RadialGradient(mask) => StoredMask::RadialGradient(*mask),
        LocalMask::LumaRange(mask) => StoredMask::LumaRange(*mask),
        LocalMask::ColorRange(mask) => StoredMask::ColorRange(*mask),
        LocalMask::Subject(mask) => StoredMask::Subject(stored_soft_mask_from_mask(mask)?),
        LocalMask::Segmentation(mask) => {
            StoredMask::Segmentation(stored_region_mask_from_mask(mask)?)
        }
    })
}

fn local_mask_from_stored(stored: &StoredMask) -> Result<LocalMask, String> {
    Ok(match stored {
        StoredMask::Full => LocalMask::Full,
        StoredMask::Raster(mask) => LocalMask::RasterVector(raster_vector_from_stored(mask)?),
        StoredMask::LinearGradient(mask) => LocalMask::LinearGradient(*mask),
        StoredMask::RadialGradient(mask) => LocalMask::RadialGradient(*mask),
        StoredMask::LumaRange(mask) => LocalMask::LumaRange(*mask),
        StoredMask::ColorRange(mask) => LocalMask::ColorRange(*mask),
        StoredMask::Subject(mask) => LocalMask::Subject(soft_mask_from_stored(mask)?),
        StoredMask::Segmentation(mask) => LocalMask::Segmentation(region_mask_from_stored(mask)?),
    })
}

fn stored_layer_from_local(layer: &LocalAdjustmentLayer) -> Result<StoredLayer, String> {
    Ok(StoredLayer {
        name: layer.name.clone(),
        enabled: layer.enabled,
        opacity: layer.opacity,
        mask: stored_mask_from_local(&layer.mask)?,
        manual_override: stored_manual_override_from_layer(&layer.manual_override)?,
        mask_inverted: layer.mask_inverted,
        mask_expand_px: layer.mask_expand_px,
        mask_feather_px: layer.mask_feather_px,
        effect: layer.effect.clone(),
    })
}

fn local_layer_from_stored(stored: &StoredLayer) -> Result<LocalAdjustmentLayer, String> {
    Ok(LocalAdjustmentLayer {
        name: stored.name.clone(),
        enabled: stored.enabled,
        opacity: stored.opacity,
        mask: local_mask_from_stored(&stored.mask)?,
        manual_override: manual_override_from_stored(&stored.manual_override)?,
        mask_inverted: stored.mask_inverted,
        mask_expand_px: stored.mask_expand_px,
        mask_feather_px: stored.mask_feather_px,
        effect: stored.effect.clone(),
    })
}

fn color_image_from_rgba(image: &RgbaImageBuf) -> ColorImage {
    ColorImage::from_rgba_unmultiplied([image.width, image.height], &image.pixels)
}

fn build_mask_tile_image(
    mask: &[f32],
    layer: &LocalAdjustmentLayer,
    image_width: usize,
    tile_x: usize,
    tile_y: usize,
    tile_w: usize,
    tile_h: usize,
) -> ColorImage {
    let mut pixels = Vec::with_capacity(tile_w.saturating_mul(tile_h));
    for y in tile_y..tile_y + tile_h {
        let row = y * image_width;
        for x in tile_x..tile_x + tile_w {
            let idx = row + x;
            let add = layer
                .manual_override
                .add
                .as_ref()
                .map(|manual| raster_vector_alpha_at(manual, idx, x, y) >= 0.5)
                .unwrap_or(false);
            let subtract = layer
                .manual_override
                .subtract
                .as_ref()
                .map(|manual| raster_vector_alpha_at(manual, idx, x, y) >= 0.5)
                .unwrap_or(false);
            if subtract {
                pixels.push(Color32::from_rgba_unmultiplied(64, 190, 255, 218));
            } else if add {
                pixels.push(Color32::from_rgba_unmultiplied(90, 255, 120, 210));
            } else {
                let alpha =
                    (mask.get(idx).copied().unwrap_or(0.0).clamp(0.0, 1.0) * 155.0).round() as u8;
                pixels.push(Color32::from_rgba_unmultiplied(255, 48, 84, alpha));
            }
        }
    }
    ColorImage::new([tile_w, tile_h], pixels)
}

fn raster_vector_alpha_at(mask: &RasterVectorMask, idx: usize, x: usize, y: usize) -> f32 {
    let mut alpha = mask.alpha.get(idx).copied().unwrap_or(0.0).clamp(0.0, 1.0);
    if !mask.shapes.is_empty() {
        let point = [x as f32 + 0.5, y as f32 + 0.5];
        for &shape in &mask.shapes {
            if shape_contains_point(shape, point) {
                alpha = if shape.op().is_add() { 1.0 } else { 0.0 };
            }
        }
    }
    alpha
}

fn can_build_mask_tiles_from_layer(
    layer: &LocalAdjustmentLayer,
    image_width: usize,
    image_height: usize,
) -> bool {
    let mask_matches = match &layer.mask {
        LocalMask::RasterVector(mask) => {
            mask.width == image_width
                && mask.height == image_height
                && mask.alpha.len() == image_width.saturating_mul(image_height)
        }
        LocalMask::Segmentation(mask) => {
            mask.width == image_width
                && mask.height == image_height
                && mask.labels.len() == image_width.saturating_mul(image_height)
        }
        _ => false,
    };
    mask_matches
        && layer.manual_override.is_empty()
        && layer.mask_expand_px.abs() < 0.5
        && layer.mask_feather_px < 0.5
}

fn build_mask_tile_image_from_layer(
    layer: &LocalAdjustmentLayer,
    image_width: usize,
    tile_x: usize,
    tile_y: usize,
    tile_w: usize,
    tile_h: usize,
    time_sec: f32,
) -> ColorImage {
    match &layer.mask {
        LocalMask::RasterVector(mask) => {
            let opacity = layer.opacity.clamp(0.0, 1.0);
            let mut pixels = Vec::with_capacity(tile_w.saturating_mul(tile_h));
            for y in tile_y..tile_y + tile_h {
                let row = y * image_width;
                for x in tile_x..tile_x + tile_w {
                    let mut alpha = mask
                        .alpha
                        .get(row + x)
                        .copied()
                        .unwrap_or(0.0)
                        .clamp(0.0, 1.0);
                    if !mask.shapes.is_empty() {
                        let point = [x as f32 + 0.5, y as f32 + 0.5];
                        for &shape in &mask.shapes {
                            if shape_contains_point(shape, point) {
                                alpha = if shape.op().is_add() { 1.0 } else { 0.0 };
                            }
                        }
                    }
                    if layer.mask_inverted {
                        alpha = 1.0 - alpha;
                    }
                    alpha = (alpha * opacity).clamp(0.0, 1.0);
                    let alpha_u8 = (alpha * 155.0).round() as u8;
                    pixels.push(Color32::from_rgba_unmultiplied(255, 48, 84, alpha_u8));
                }
            }
            ColorImage::new([tile_w, tile_h], pixels)
        }
        LocalMask::Segmentation(mask) => {
            let mut pixels = Vec::with_capacity(tile_w.saturating_mul(tile_h));
            for y in tile_y..tile_y + tile_h {
                let row = y * image_width;
                for x in tile_x..tile_x + tile_w {
                    pixels.push(region_preview_pixel(
                        mask,
                        layer.mask_inverted,
                        x,
                        y,
                        row + x,
                        time_sec,
                    ));
                }
            }
            ColorImage::new([tile_w, tile_h], pixels)
        }
        _ => ColorImage::new(
            [tile_w, tile_h],
            vec![Color32::TRANSPARENT; tile_w.saturating_mul(tile_h)],
        ),
    }
}

fn region_preview_pixel(
    mask: &RegionMask,
    inverted: bool,
    x: usize,
    y: usize,
    idx: usize,
    time_sec: f32,
) -> Color32 {
    let label = mask.labels.get(idx).copied().unwrap_or(0);
    if label == 0 {
        return Color32::TRANSPARENT;
    }
    let active = region_label_active(mask, inverted, label);
    if active {
        if region_active_boundary(mask, inverted, label, x, y) {
            return Color32::from_rgba_unmultiplied(255, 245, 120, 235);
        }
        return Color32::from_rgba_unmultiplied(255, 42, 112, 188);
    }
    if region_label_boundary(mask, label, x, y) {
        let [r, g, b] = animated_region_boundary_color(label, time_sec);
        return Color32::from_rgba_unmultiplied(r, g, b, 190);
    }
    Color32::TRANSPARENT
}

fn region_label_active(mask: &RegionMask, inverted: bool, label: u32) -> bool {
    if label == 0 {
        return false;
    }
    let selected = mask.selected.get(label as usize).copied().unwrap_or(false);
    selected ^ inverted
}

fn region_active_boundary(
    mask: &RegionMask,
    inverted: bool,
    label: u32,
    x: usize,
    y: usize,
) -> bool {
    if x == 0 || y == 0 || x + 1 == mask.width || y + 1 == mask.height {
        return true;
    }
    for (nx, ny) in region_neighbors(x, y, mask.width, mask.height) {
        let n_label = mask.labels[ny * mask.width + nx];
        if n_label != label || !region_label_active(mask, inverted, n_label) {
            return true;
        }
    }
    false
}

fn region_label_boundary(mask: &RegionMask, label: u32, x: usize, y: usize) -> bool {
    if x == 0 || y == 0 || x + 1 == mask.width || y + 1 == mask.height {
        return true;
    }
    region_neighbors(x, y, mask.width, mask.height)
        .any(|(nx, ny)| mask.labels[ny * mask.width + nx] != label)
}

fn animated_region_boundary_color(label: u32, time_sec: f32) -> [u8; 3] {
    let hue = (time_sec * 130.0 + (label.wrapping_mul(47) % 360) as f32).rem_euclid(360.0);
    hsv_to_rgb(hue, 0.95, 1.0)
}

fn hsv_to_rgb(hue: f32, sat: f32, val: f32) -> [u8; 3] {
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

fn shape_contains_point(shape: MaskShape, point: [f32; 2]) -> bool {
    match shape {
        MaskShape::Line {
            p0, p1, thickness, ..
        } => distance_to_segment(point, p0, p1) <= thickness.max(1.0) * 0.5,
        MaskShape::Rect {
            center,
            half_w,
            half_h,
            rotation_rad,
            ..
        } => {
            let p = inverse_rotate_point(point, center, rotation_rad);
            (p[0] - center[0]).abs() <= half_w.max(0.5)
                && (p[1] - center[1]).abs() <= half_h.max(0.5)
        }
        MaskShape::Ellipse {
            center,
            rx,
            ry,
            rotation_rad,
            ..
        } => {
            let p = inverse_rotate_point(point, center, rotation_rad);
            let rx = rx.max(0.5);
            let ry = ry.max(0.5);
            ((p[0] - center[0]) / rx).powi(2) + ((p[1] - center[1]) / ry).powi(2) <= 1.0
        }
    }
}

impl CropRect {
    fn full(width: usize, height: usize) -> Self {
        Self {
            min_x: 0.0,
            min_y: 0.0,
            max_x: width.max(1) as f32,
            max_y: height.max(1) as f32,
        }
    }

    fn width(self) -> f32 {
        (self.max_x - self.min_x).max(1.0)
    }

    fn height(self) -> f32 {
        (self.max_y - self.min_y).max(1.0)
    }

    fn sanitized(self, width: usize, height: usize) -> Self {
        let max_w = width.max(1) as f32;
        let max_h = height.max(1) as f32;
        let mut min_x = self.min_x.min(self.max_x).clamp(0.0, max_w - 1.0);
        let mut min_y = self.min_y.min(self.max_y).clamp(0.0, max_h - 1.0);
        let mut max_x = self.max_x.max(self.min_x).clamp(1.0, max_w);
        let mut max_y = self.max_y.max(self.min_y).clamp(1.0, max_h);
        if max_x - min_x < 1.0 {
            if max_x >= max_w {
                min_x = (max_w - 1.0).max(0.0);
                max_x = max_w;
            } else {
                max_x = (min_x + 1.0).min(max_w);
            }
        }
        if max_y - min_y < 1.0 {
            if max_y >= max_h {
                min_y = (max_h - 1.0).max(0.0);
                max_y = max_h;
            } else {
                max_y = (min_y + 1.0).min(max_h);
            }
        }
        Self {
            min_x,
            min_y,
            max_x,
            max_y,
        }
    }

    fn to_screen_rect(self, image_rect: Rect, width: usize, height: usize) -> Rect {
        let w = width.max(1) as f32;
        let h = height.max(1) as f32;
        Rect::from_min_max(
            egui::pos2(
                image_rect.left() + image_rect.width() * self.min_x / w,
                image_rect.top() + image_rect.height() * self.min_y / h,
            ),
            egui::pos2(
                image_rect.left() + image_rect.width() * self.max_x / w,
                image_rect.top() + image_rect.height() * self.max_y / h,
            ),
        )
    }

    fn dragged(
        self,
        handle: CropHandle,
        delta_x: f32,
        delta_y: f32,
        width: usize,
        height: usize,
    ) -> Self {
        let mut next = self;
        match handle {
            CropHandle::Body => {
                let max_w = width.max(1) as f32;
                let max_h = height.max(1) as f32;
                let w = next.width().min(max_w);
                let h = next.height().min(max_h);
                next.min_x = (self.min_x + delta_x).clamp(0.0, max_w - w);
                next.min_y = (self.min_y + delta_y).clamp(0.0, max_h - h);
                next.max_x = next.min_x + w;
                next.max_y = next.min_y + h;
                return next.sanitized(width, height);
            }
            CropHandle::North => next.min_y += delta_y,
            CropHandle::South => next.max_y += delta_y,
            CropHandle::West => next.min_x += delta_x,
            CropHandle::East => next.max_x += delta_x,
            CropHandle::NorthWest => {
                next.min_x += delta_x;
                next.min_y += delta_y;
            }
            CropHandle::NorthEast => {
                next.max_x += delta_x;
                next.min_y += delta_y;
            }
            CropHandle::SouthWest => {
                next.min_x += delta_x;
                next.max_y += delta_y;
            }
            CropHandle::SouthEast => {
                next.max_x += delta_x;
                next.max_y += delta_y;
            }
        }
        next.sanitized(width, height)
    }
}

fn crop_handle_points(rect: Rect) -> [(CropHandle, Pos2); 9] {
    let center = rect.center();
    [
        (CropHandle::Body, center),
        (CropHandle::NorthWest, rect.left_top()),
        (CropHandle::North, egui::pos2(center.x, rect.top())),
        (CropHandle::NorthEast, rect.right_top()),
        (CropHandle::East, egui::pos2(rect.right(), center.y)),
        (CropHandle::SouthEast, rect.right_bottom()),
        (CropHandle::South, egui::pos2(center.x, rect.bottom())),
        (CropHandle::SouthWest, rect.left_bottom()),
        (CropHandle::West, egui::pos2(rect.left(), center.y)),
    ]
}

fn crop_handle_cursor(handle: CropHandle) -> egui::CursorIcon {
    match handle {
        CropHandle::Body => egui::CursorIcon::Grab,
        CropHandle::North | CropHandle::South => egui::CursorIcon::ResizeVertical,
        CropHandle::West | CropHandle::East => egui::CursorIcon::ResizeHorizontal,
        CropHandle::NorthWest | CropHandle::SouthEast => egui::CursorIcon::ResizeNwSe,
        CropHandle::NorthEast | CropHandle::SouthWest => egui::CursorIcon::ResizeNeSw,
    }
}

fn outside_rects(outer: Rect, inner: Rect) -> [Rect; 4] {
    let inner = inner.intersect(outer);
    [
        Rect::from_min_max(outer.min, egui::pos2(outer.right(), inner.top())),
        Rect::from_min_max(
            egui::pos2(outer.left(), inner.bottom()),
            outer.right_bottom(),
        ),
        Rect::from_min_max(
            egui::pos2(outer.left(), inner.top()),
            egui::pos2(inner.left(), inner.bottom()),
        ),
        Rect::from_min_max(
            egui::pos2(inner.right(), inner.top()),
            egui::pos2(outer.right(), inner.bottom()),
        ),
    ]
}

fn crop_rgba_image(src: &RgbaImageBuf, crop: CropRect) -> RgbaImageBuf {
    let crop = crop.sanitized(src.width, src.height);
    let x0 = crop
        .min_x
        .floor()
        .clamp(0.0, src.width.saturating_sub(1) as f32) as usize;
    let y0 = crop
        .min_y
        .floor()
        .clamp(0.0, src.height.saturating_sub(1) as f32) as usize;
    let x1 = crop
        .max_x
        .ceil()
        .clamp((x0 + 1) as f32, src.width.max(1) as f32) as usize;
    let y1 = crop
        .max_y
        .ceil()
        .clamp((y0 + 1) as f32, src.height.max(1) as f32) as usize;
    let out_w = (x1 - x0).max(1);
    let out_h = (y1 - y0).max(1);
    let mut pixels = Vec::with_capacity(out_w * out_h * 4);
    for y in y0..y1 {
        let start = (y * src.width + x0) * 4;
        let end = start + out_w * 4;
        pixels.extend_from_slice(&src.pixels[start..end]);
    }
    RgbaImageBuf::new(out_w, out_h, pixels).unwrap_or_else(|_| src.clone())
}

fn save_result_png(src_path: &Path, result: &RgbaImageBuf) -> Result<PathBuf, String> {
    let path = sibling_output_path(src_path, "local_adjust", "png");
    let Some(out) = RgbaImage::from_raw(
        result.width as u32,
        result.height as u32,
        result.pixels.clone(),
    ) else {
        return Err("invalid result RGBA buffer".to_string());
    };
    out.save(&path).map_err(|e| e.to_string())?;
    Ok(path)
}

fn sibling_output_path(src: &Path, suffix: &str, ext: &str) -> PathBuf {
    let parent = src.parent().unwrap_or_else(|| Path::new("."));
    let stem = src.file_stem().and_then(|s| s.to_str()).unwrap_or("image");
    parent.join(format!("{stem}_{suffix}.{ext}"))
}

fn fit_scale_for_canvas(canvas_rect: Rect, img_w: usize, img_h: usize) -> f32 {
    let w = img_w.max(1) as f32;
    let h = img_h.max(1) as f32;
    (canvas_rect.width() / w).min(canvas_rect.height() / h)
}

fn image_rect_for_canvas(
    canvas_rect: Rect,
    img_w: usize,
    img_h: usize,
    zoom: f32,
    pan: egui::Vec2,
) -> Rect {
    let scale = fit_scale_for_canvas(canvas_rect, img_w, img_h) * zoom.clamp(ZOOM_MIN, ZOOM_MAX);
    let size = egui::vec2(img_w.max(1) as f32 * scale, img_h.max(1) as f32 * scale);
    Rect::from_center_size(canvas_rect.center() + pan, size)
}

fn screen_to_image(rect: Rect, img_w: usize, img_h: usize, p: Pos2) -> Option<Pos2> {
    if !rect.contains(p) {
        return None;
    }
    let x = ((p.x - rect.left()) / rect.width()) * img_w as f32;
    let y = ((p.y - rect.top()) / rect.height()) * img_h as f32;
    Some(Pos2::new(
        x.clamp(0.0, img_w as f32 - 1.0),
        y.clamp(0.0, img_h as f32 - 1.0),
    ))
}

fn canvas_input_positions(
    ui: &egui::Ui,
    rect: Rect,
    img_w: usize,
    img_h: usize,
    fallback_screen: Pos2,
) -> Vec<Pos2> {
    let screen_positions = ui.input(|i| {
        let mut positions = Vec::new();
        for event in &i.events {
            match event {
                egui::Event::PointerMoved(pos) => positions.push(*pos),
                egui::Event::PointerButton {
                    pos,
                    button: egui::PointerButton::Primary,
                    ..
                } => positions.push(*pos),
                _ => {}
            }
        }
        positions
    });
    let mut out = Vec::new();
    for screen in screen_positions
        .into_iter()
        .chain(std::iter::once(fallback_screen))
    {
        let Some(image_pos) = screen_to_image(rect, img_w, img_h, screen) else {
            continue;
        };
        if out
            .last()
            .map(|last: &Pos2| last.distance(image_pos) > 0.25)
            .unwrap_or(true)
        {
            out.push(image_pos);
        }
    }
    out
}

fn brush_input_positions(input_positions: &[Pos2], fallback: Pos2) -> Vec<Pos2> {
    if input_positions.is_empty() {
        vec![fallback]
    } else {
        input_positions.to_vec()
    }
}

fn brush_dirty_rect_for_point(
    p: Pos2,
    radius: f32,
    width: usize,
    height: usize,
) -> Option<MaskDirtyRect> {
    if width == 0 || height == 0 {
        return None;
    }
    let pad = radius.ceil().max(1.0) as usize + 2;
    let cx = p.x.floor().clamp(0.0, width.saturating_sub(1) as f32) as usize;
    let cy = p.y.floor().clamp(0.0, height.saturating_sub(1) as f32) as usize;
    Some(MaskDirtyRect {
        min_x: cx.saturating_sub(pad),
        min_y: cy.saturating_sub(pad),
        max_x: (cx + pad).min(width.saturating_sub(1)),
        max_y: (cy + pad).min(height.saturating_sub(1)),
    })
}

fn insert_dirty_tiles_for_rect(
    tiles: &mut BTreeSet<(usize, usize)>,
    dirty_rect: MaskDirtyRect,
    width: usize,
    height: usize,
) {
    if width == 0 || height == 0 {
        return;
    }
    let min_col = dirty_rect.min_x.min(width.saturating_sub(1)) / MASK_PREVIEW_TILE_SIZE;
    let max_col = dirty_rect.max_x.min(width.saturating_sub(1)) / MASK_PREVIEW_TILE_SIZE;
    let min_row = dirty_rect.min_y.min(height.saturating_sub(1)) / MASK_PREVIEW_TILE_SIZE;
    let max_row = dirty_rect.max_y.min(height.saturating_sub(1)) / MASK_PREVIEW_TILE_SIZE;
    for row in min_row..=max_row {
        for col in min_col..=max_col {
            tiles.insert((col, row));
        }
    }
}

fn avg_ms(total_ms: f64, count: u64) -> f64 {
    if count == 0 {
        0.0
    } else {
        total_ms / count as f64
    }
}

fn avg_count(total: u64, count: u64) -> f64 {
    if count == 0 {
        0.0
    } else {
        total as f64 / count as f64
    }
}

fn perf_log_path() -> PathBuf {
    std::env::current_dir()
        .unwrap_or_else(|_| PathBuf::from("."))
        .join("target")
        .join("local_adjust_lab_perf.log")
}

fn interpolated_stroke_points(last: Option<Pos2>, current: Pos2, spacing: f32) -> Vec<Pos2> {
    let Some(last) = last else {
        return vec![current];
    };
    let distance = last.distance(current);
    if distance <= spacing.max(0.001) {
        return vec![current];
    }
    let steps = ((distance / spacing).ceil() as usize)
        .max(1)
        .min(BRUSH_STROKE_MAX_STAMPS_PER_FRAME);
    (1..=steps)
        .map(|i| last.lerp(current, i as f32 / steps as f32))
        .collect()
}

fn catmull_rom_stroke_points(p0: Pos2, p1: Pos2, p2: Pos2, p3: Pos2, spacing: f32) -> Vec<Pos2> {
    let estimated_len = p1.distance(p2);
    if estimated_len <= spacing.max(0.001) {
        return vec![p2];
    }
    let steps = ((estimated_len / spacing).ceil() as usize)
        .max(1)
        .min(BRUSH_STROKE_MAX_STAMPS_PER_FRAME);
    (1..=steps)
        .map(|i| {
            let t = i as f32 / steps as f32;
            let t2 = t * t;
            let t3 = t2 * t;
            let x = 0.5
                * ((2.0 * p1.x)
                    + (-p0.x + p2.x) * t
                    + (2.0 * p0.x - 5.0 * p1.x + 4.0 * p2.x - p3.x) * t2
                    + (-p0.x + 3.0 * p1.x - 3.0 * p2.x + p3.x) * t3);
            let y = 0.5
                * ((2.0 * p1.y)
                    + (-p0.y + p2.y) * t
                    + (2.0 * p0.y - 5.0 * p1.y + 4.0 * p2.y - p3.y) * t2
                    + (-p0.y + 3.0 * p1.y - 3.0 * p2.y + p3.y) * t3);
            Pos2::new(x, y)
        })
        .collect()
}

fn draw_ellipse_stroke(
    painter: &egui::Painter,
    center: Pos2,
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
        let a = std::f32::consts::TAU * i as f32 / steps as f32;
        points.push(Pos2::new(
            center.x + radius_x * a.cos(),
            center.y + radius_y * a.sin(),
        ));
    }
    painter.add(egui::Shape::line(points, stroke));
}

fn norm_to_screen(rect: Rect, n: [f32; 2]) -> Pos2 {
    Pos2::new(
        rect.left() + n[0].clamp(0.0, 1.0) * rect.width(),
        rect.top() + n[1].clamp(0.0, 1.0) * rect.height(),
    )
}

fn screen_to_norm(rect: Rect, p: Pos2) -> [f32; 2] {
    [
        ((p.x - rect.left()) / rect.width()).clamp(0.0, 1.0),
        ((p.y - rect.top()) / rect.height()).clamp(0.0, 1.0),
    ]
}

fn drag_norm_handle(
    ui: &mut egui::Ui,
    rect: Rect,
    id: egui::Id,
    center: Pos2,
    value: &mut [f32; 2],
    label: &str,
) -> (bool, bool) {
    let response = ui
        .interact(
            Rect::from_center_size(center, egui::vec2(24.0, 24.0)),
            id,
            Sense::drag(),
        )
        .on_hover_text(label);
    if response.dragged()
        && let Some(pos) = response.interact_pointer_pos()
    {
        *value = screen_to_norm(rect, pos);
        return (true, true);
    }
    (false, response.hovered() || response.dragged())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn vertical_line_image(
        bg: u8,
        line: u8,
        line_x: std::ops::RangeInclusive<usize>,
    ) -> RgbaImageBuf {
        let width = 17;
        let height = 17;
        let mut pixels = Vec::with_capacity(width * height * 4);
        for _y in 0..height {
            for x in 0..width {
                let v = if line_x.contains(&x) { line } else { bg };
                pixels.extend_from_slice(&[v, v, v, 255]);
            }
        }
        RgbaImageBuf::new(width, height, pixels).unwrap()
    }

    #[test]
    fn polygon_hit_test_handles_descending_sloped_edges() {
        let points = [[1.0, 1.0], [7.0, 1.0], [4.0, 7.0]];
        assert!(point_in_polygon([3.0, 4.0], &points));
        assert!(!point_in_polygon([1.5, 4.0], &points));
    }

    #[test]
    fn lab_dark_visuals_keep_popups_dark() {
        let visuals = lab_dark_visuals();
        assert_eq!(visuals.override_text_color, Some(Color32::WHITE));
        assert!(visuals.window_fill.r() < 64);
        assert!(visuals.panel_fill.r() < 64);
        assert!(visuals.extreme_bg_color.r() < 32);
        assert!(visuals.widgets.inactive.bg_fill.r() < 96);
    }

    #[test]
    fn copying_mask_fields_preserves_target_effect_and_opacity() {
        let mut target = LocalAdjustmentLayer::new(
            "target",
            LocalMask::Full,
            LocalEffect::Blur(BlurParams { radius_px: 8.0 }),
        );
        target.opacity = 0.4;
        let mut source = LocalAdjustmentLayer::new(
            "source",
            LocalMask::LinearGradient(LinearGradientMask {
                start: [0.1, 0.2],
                end: [0.8, 0.9],
                initialized: true,
            }),
            LocalEffect::None,
        );
        source.mask_inverted = true;
        source.mask_expand_px = 3.0;
        source.mask_feather_px = 5.0;
        source.manual_override.add = Some(RasterVectorMask {
            width: 1,
            height: 1,
            alpha: vec![1.0],
            shapes: Vec::new(),
        });

        copy_mask_fields_from_layer(&mut target, &source);

        assert!(matches!(target.mask, LocalMask::LinearGradient(_)));
        assert!(target.manual_override.add.is_some());
        assert!(target.mask_inverted);
        assert_eq!(target.mask_expand_px, 3.0);
        assert_eq!(target.mask_feather_px, 5.0);
        assert!(matches!(target.effect, LocalEffect::Blur(_)));
        assert_eq!(target.opacity, 0.4);
    }

    #[test]
    fn stored_raster_vector_mask_roundtrips_as_binary_alpha() {
        let mask = RasterVectorMask {
            width: 4,
            height: 1,
            alpha: vec![0.0, 0.49, 0.5, 1.0],
            shapes: vec![MaskShape::Rect {
                op: ShapeOp::Add,
                center: [1.0, 1.0],
                half_w: 0.5,
                half_h: 0.5,
                rotation_rad: 0.0,
            }],
        };

        let stored = stored_raster_vector_from_mask(&mask).unwrap();
        let restored = raster_vector_from_stored(&stored).unwrap();

        assert_eq!(restored.width, 4);
        assert_eq!(restored.height, 1);
        assert_eq!(restored.alpha, vec![0.0, 0.0, 1.0, 1.0]);
        assert_eq!(restored.shapes, mask.shapes);
    }

    #[test]
    fn stored_layer_roundtrips_manual_override_and_effect() {
        let mut layer = LocalAdjustmentLayer::new(
            "soft",
            LocalMask::RadialGradient(RadialGradientMask {
                initialized: true,
                center: [0.5, 0.5],
                inner_radius: 0.1,
                inner_radius_y: 0.2,
                outer_radius: 0.4,
                outer_radius_y: 0.6,
            }),
            LocalEffect::SoftFocus(SoftFocusParams {
                radius_px: 18.0,
                strength: 0.42,
            }),
        );
        layer.manual_override.subtract = Some(RasterVectorMask {
            width: 2,
            height: 1,
            alpha: vec![0.0, 1.0],
            shapes: Vec::new(),
        });
        layer.mask_inverted = true;
        layer.mask_feather_px = 3.0;

        let stored = stored_layer_from_local(&layer).unwrap();
        let restored = local_layer_from_stored(&stored).unwrap();

        assert_eq!(restored.name, "soft");
        assert!(matches!(restored.mask, LocalMask::RadialGradient(_)));
        assert!(restored.manual_override.subtract.is_some());
        assert!(matches!(restored.effect, LocalEffect::SoftFocus(_)));
        assert!(restored.mask_inverted);
        assert_eq!(restored.mask_feather_px, 3.0);
    }

    #[test]
    fn mask_preview_colors_manual_override() {
        let mut layer = LocalAdjustmentLayer::new(
            "mask",
            LocalMask::Full,
            LocalEffect::Tone(ToneParams::default()),
        );
        layer.manual_override.add = Some(RasterVectorMask {
            width: 3,
            height: 1,
            alpha: vec![1.0, 0.0, 0.0],
            shapes: Vec::new(),
        });
        layer.manual_override.subtract = Some(RasterVectorMask {
            width: 3,
            height: 1,
            alpha: vec![0.0, 1.0, 0.0],
            shapes: Vec::new(),
        });

        let image = build_mask_tile_image(&[0.2, 0.8, 0.5], &layer, 3, 0, 0, 3, 1);

        assert_eq!(
            image.pixels[0],
            Color32::from_rgba_unmultiplied(90, 255, 120, 210)
        );
        assert_eq!(
            image.pixels[1],
            Color32::from_rgba_unmultiplied(64, 190, 255, 218)
        );
        assert_eq!(
            image.pixels[2],
            Color32::from_rgba_unmultiplied(255, 48, 84, 78)
        );
    }

    #[test]
    fn crop_handle_cursor_matches_handle_direction() {
        assert_eq!(
            crop_handle_cursor(CropHandle::North),
            egui::CursorIcon::ResizeVertical
        );
        assert_eq!(
            crop_handle_cursor(CropHandle::East),
            egui::CursorIcon::ResizeHorizontal
        );
        assert_eq!(
            crop_handle_cursor(CropHandle::NorthWest),
            egui::CursorIcon::ResizeNwSe
        );
        assert_eq!(
            crop_handle_cursor(CropHandle::NorthEast),
            egui::CursorIcon::ResizeNeSw
        );
    }

    #[test]
    fn crop_rgba_image_returns_requested_region() {
        let src = RgbaImageBuf::new(
            3,
            2,
            vec![
                10, 0, 0, 255, 20, 0, 0, 255, 30, 0, 0, 255, 40, 0, 0, 255, 50, 0, 0, 255, 60, 0,
                0, 255,
            ],
        )
        .unwrap();
        let out = crop_rgba_image(
            &src,
            CropRect {
                min_x: 1.0,
                min_y: 0.0,
                max_x: 3.0,
                max_y: 2.0,
            },
        );
        assert_eq!(out.width, 2);
        assert_eq!(out.height, 2);
        assert_eq!(out.pixels[0], 20);
        assert_eq!(out.pixels[4], 30);
        assert_eq!(out.pixels[8], 50);
        assert_eq!(out.pixels[12], 60);
    }

    #[test]
    fn boundary_detector_keeps_dark_line_narrow() {
        let image = vertical_line_image(255, 0, 7..=9);
        let y = 8;
        let hits: Vec<usize> = (0..image.width)
            .filter(|&x| boundary_pixel_at(&image, x, y, 24.0, 28.0, 0))
            .collect();
        assert_eq!(hits, vec![6, 7, 8, 9, 10]);
    }

    #[test]
    fn boundary_detector_keeps_bright_line_narrow() {
        let image = vertical_line_image(0, 255, 7..=9);
        let y = 8;
        let hits: Vec<usize> = (0..image.width)
            .filter(|&x| boundary_pixel_at(&image, x, y, 24.0, 28.0, 0))
            .collect();
        assert_eq!(hits, vec![6, 7, 8, 9, 10]);
    }

    #[test]
    fn boundary_gap_fill_bridges_tiny_detection_holes() {
        let width = 17;
        let height = 17;
        let mut pixels = Vec::with_capacity(width * height * 4);
        for _y in 0..height {
            for x in 0..width {
                let v = if x == 7 || x == 9 { 0 } else { 255 };
                pixels.extend_from_slice(&[v, v, v, 255]);
            }
        }
        let image = RgbaImageBuf::new(width, height, pixels).unwrap();
        let y = 8;
        assert!(!boundary_pixel_at(&image, 8, y, 24.0, 28.0, 0));
        assert!(boundary_pixel_at(&image, 8, y, 24.0, 28.0, 2));
        assert!(!boundary_pixel_at(&image, 5, y, 24.0, 28.0, 2));
    }

    #[test]
    fn boundary_mask_cache_matches_detector() {
        let image = vertical_line_image(255, 0, 7..=9);
        let mask = build_boundary_mask(&image, 24, 28, 2);
        for y in 0..image.height {
            for x in 0..image.width {
                assert_eq!(
                    boundary_mask_at(&mask, image.width, x, y),
                    boundary_pixel_at(&image, x, y, 24.0, 28.0, 2)
                );
            }
        }
    }

    #[test]
    fn edge_brush_include_boundary_only_adds_adjacent_line_pixels() {
        let image = vertical_line_image(255, 0, 7..=9);
        let boundary_mask = build_boundary_mask(&image, 24, 28, 0);
        let y = 8;
        let bw = image.width;
        let mut targets = vec![y * image.width + 5];
        let mut target_map = vec![false; image.width * image.height];
        target_map[y * bw + 5] = true;

        include_adjacent_boundary_pixels(
            &image,
            &mut targets,
            &mut target_map,
            0,
            image.width - 1,
            0,
            image.height - 1,
            bw,
            Pos2::new(5.5, y as f32 + 0.5),
            25.0,
            &boundary_mask,
        );

        let hit_x: Vec<usize> = targets.into_iter().map(|idx| idx % image.width).collect();
        assert!(hit_x.contains(&6));
        assert!(hit_x.contains(&7));
        assert!(!hit_x.contains(&10));
    }

    #[test]
    fn stroke_interpolation_adds_points_between_fast_pointer_samples() {
        let points =
            interpolated_stroke_points(Some(Pos2::new(0.0, 0.0)), Pos2::new(30.0, 0.0), 10.0);
        assert_eq!(points.len(), 3);
        assert_eq!(points[0], Pos2::new(10.0, 0.0));
        assert_eq!(points[1], Pos2::new(20.0, 0.0));
        assert_eq!(points[2], Pos2::new(30.0, 0.0));
    }

    #[test]
    fn catmull_rom_stroke_interpolation_reaches_current_point() {
        let points = catmull_rom_stroke_points(
            Pos2::new(0.0, 0.0),
            Pos2::new(10.0, 0.0),
            Pos2::new(20.0, 10.0),
            Pos2::new(30.0, 10.0),
            5.0,
        );
        assert!(!points.is_empty());
        let last = *points.last().unwrap();
        assert!((last.x - 20.0).abs() < 0.001);
        assert!((last.y - 10.0).abs() < 0.001);
    }

    #[test]
    fn region_segmentation_splits_connected_color_regions() {
        let width = 6;
        let height = 4;
        let mut pixels = Vec::with_capacity(width * height * 4);
        for _y in 0..height {
            for x in 0..width {
                let rgb = if x < 3 { [230, 40, 40] } else { [40, 80, 230] };
                pixels.extend_from_slice(&[rgb[0], rgb[1], rgb[2], 255]);
            }
        }
        let source = RgbaImageBuf::new(width, height, pixels).unwrap();
        let mask = build_region_segmentation(&source, None, 8.0, 1, 255, 255, 0).unwrap();
        assert_eq!(mask.label_count(), 2);
        assert_ne!(mask.labels[0], mask.labels[width - 1]);
        assert!(mask.selected.iter().skip(1).all(|&selected| !selected));
    }

    #[test]
    fn region_segmentation_fills_unlabeled_internal_gaps() {
        let mut labels = vec![1, 0, 2];
        let allowed = vec![true, true, true];
        fill_unlabeled_region_pixels(&mut labels, 3, 1, &allowed);
        assert_ne!(labels[1], 0);
    }

    #[test]
    #[ignore = "loads the local ONNX model and ONNX Runtime DLL"]
    fn u2netp_segmentation_smoke_test() {
        if !segmentation_model_path().is_file() {
            return;
        }
        let width = 32;
        let height = 32;
        let mut pixels = Vec::with_capacity(width * height * 4);
        for y in 0..height {
            for x in 0..width {
                let inside = (8..24).contains(&x) && (6..26).contains(&y);
                let v = if inside { 230 } else { 20 };
                pixels.extend_from_slice(&[v, v, v, 255]);
            }
        }
        let source = RgbaImageBuf::new(width, height, pixels).unwrap();
        let mask = run_u2netp_segmentation(&source, &segmentation_model_path()).unwrap();
        assert_eq!(mask.width, width);
        assert_eq!(mask.height, height);
        assert_eq!(mask.alpha.len(), width * height);
        assert!(mask.alpha.iter().all(|v| v.is_finite()));
    }
}
