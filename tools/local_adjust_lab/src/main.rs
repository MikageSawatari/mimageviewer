use std::collections::{BTreeSet, VecDeque};
use std::fs::OpenOptions;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
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
    ArtisticMediaMode, ArtisticMediaParams, BloomParams, BlurParams, BrushStrokeMode,
    BrushStrokeParams, ChannelMixerParams, ChromaticAberrationParams, ClarityParams, CloudFogMode,
    CloudFogParams, ColorBalanceParams, ColorBalanceRange, ColorFillParams, ColorGradeWheel,
    ColorHalftoneParams, ColorMixerParams, ColorOverlayBlendMode, ColorOverlayParams,
    ColorOverlayShape, ColorRangeMask, CubeLutParams, CutoutParams, DehazeParams, DespeckleParams,
    DiffuseGlowParams, DuotoneParams, DuotonePreset, EdgeSmoothParams, EmbossParams,
    EqualizeParams, FilmGrainParams, GlassDisplacementMode, GlassDisplacementParams,
    GlowingEdgesParams, GodRaysParams, GradientMapParams, GradientMapPreset, HalftoneParams,
    HighPassParams, HighlightsShadowsParams, HslParams, InvertParams, LensBlurAperture,
    LensBlurParams, LensCorrectionParams, LensFlareParams, LineExtractMode, LineExtractParams,
    LineKind, LinearGradientMask, LocalAdjustmentLayer, LocalEffect, LocalMask, LookParams,
    LookPreset, ManualMaskOverride, MaskShape, MedianParams, MosaicBoundary, MosaicParams,
    MosaicTileMode, MotionBlurParams, NeonGlowParams, NoiseDistribution, NoiseParams,
    OilPaintParams, OutlineStrokeParams, OutlineStrokePlacement, PinchSpherizeParams,
    PixelStylizeMode, PixelStylizeParams, PolarCoordinatesMode, PolarCoordinatesParams,
    PosterizeParams, RadialBlurMode, RadialBlurParams, RadialGradientMask, RangeMask, RasterMask,
    RasterVectorMask, RegionMask, RgbToneCurveParams, RgbaImageBuf, RgbaImageRef, ScreenToneMode,
    ScreenToneParams, SelectiveColorParams, ShapeOp, SharpenParams, SmartSharpenParams,
    SoftFocusParams, SolarizeParams, SpeedLinesMode, SpeedLinesParams, SpotlightParams,
    StarGlowParams, SubjectMask, SubjectMaskRefinement, TextureParams, TextureizerMode,
    TextureizerParams, ThreeWayColorGradingParams, ThresholdParams, TiltShiftMode, TiltShiftParams,
    ToneCurveParams, ToneParams, TwirlParams, VignetteParams, WaveDistortionMode,
    WaveDistortionParams, WindDirection, WindParams, WindSource, apply_layers,
    apply_layers_with_progress, compute_mosaic_tile_size, default_mask_application_for_effect,
    evaluate_layer_mask, parse_cube_lut,
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
const MASK_PREVIEW_BASE_ALPHA: f32 = 155.0;
const MASK_PREVIEW_EDIT_ALPHA: u8 = 225;
const REGION_BOUNDARY_ANIM_INTERVAL_MS: u64 = 160;
const U2NETP_INPUT_SIZE: usize = 320;
const MAX_UNDO_SNAPSHOTS_NORMAL: usize = 24;
const MAX_UNDO_SNAPSHOTS_LARGE: usize = 8;
const LARGE_UNDO_PIXEL_COUNT: usize = 2_500_000;
const REGION_SEGMENT_MAX_LABELS: usize = 2048;
const FILE_HISTORY_LIMIT: usize = 20;
const LAB_TEXT_FAMILY_NAME: &str = "miv-lab-toolbar-text";
const LAB_TEXT_Y_OFFSET: f32 = 3.5;
const LAB_TOOLTIP_GAP: f32 = 8.0;
const LAB_CURSOR_FALLBACK_EXTENT: f32 = 34.0;

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
    let base_proportional = fonts
        .families
        .get(&egui::FontFamily::Proportional)
        .cloned()
        .unwrap_or_default();
    let mut lab_toolbar_fonts = Vec::new();
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
            Arc::new(egui::FontData::from_owned(data.clone())),
        );
        fonts.font_data.insert(
            "miv_lab_japanese_toolbar".to_owned(),
            Arc::new(egui::FontData::from_owned(data).tweak(egui::FontTweak {
                y_offset: LAB_TEXT_Y_OFFSET,
                ..Default::default()
            })),
        );
        lab_toolbar_fonts.push("miv_lab_japanese_toolbar".to_owned());
        for family in [egui::FontFamily::Proportional, egui::FontFamily::Monospace] {
            let family_fonts = fonts.families.entry(family).or_default();
            family_fonts.retain(|name| name != "miv_lab_japanese");
            family_fonts.insert(0, "miv_lab_japanese".to_owned());
        }
        break;
    }
    if lab_toolbar_fonts.is_empty() {
        lab_toolbar_fonts.extend(base_proportional.iter().cloned());
    } else {
        for name in base_proportional {
            if !lab_toolbar_fonts.contains(&name) {
                lab_toolbar_fonts.push(name);
            }
        }
    }
    fonts.families.insert(
        egui::FontFamily::Name(Arc::<str>::from(LAB_TEXT_FAMILY_NAME)),
        lab_toolbar_fonts,
    );
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
        apply_lab_text_style(style);
    });
}

fn apply_lab_dark_ui(ui: &mut egui::Ui) {
    ui.style_mut().visuals = lab_dark_visuals();
    apply_lab_text_style(ui.style_mut());
}

fn apply_lab_text_style(style: &mut egui::Style) {
    let family = egui::FontFamily::Name(Arc::<str>::from(LAB_TEXT_FAMILY_NAME));
    for text_style in [
        egui::TextStyle::Body,
        egui::TextStyle::Button,
        egui::TextStyle::Small,
    ] {
        if let Some(font_id) = style.text_styles.get_mut(&text_style) {
            font_id.family = family.clone();
        }
    }
}

fn lab_tooltip_frame() -> egui::Frame {
    static FRAME: OnceLock<egui::Frame> = OnceLock::new();
    FRAME
        .get_or_init(|| {
            let style = egui::Style {
                visuals: lab_dark_visuals(),
                ..egui::Style::default()
            };
            egui::Frame::popup(&style)
        })
        .clone()
}

fn lab_cursor_anchor_rect(ctx: &egui::Context) -> Option<egui::Rect> {
    let pos = ctx.pointer_hover_pos()?;
    Some(egui::Rect::from_min_max(
        egui::pos2(pos.x, pos.y),
        egui::pos2(pos.x, pos.y + LAB_CURSOR_FALLBACK_EXTENT),
    ))
}

fn show_lab_offset_tooltip(
    tip: egui::Tooltip<'_>,
    anchor: Option<egui::Rect>,
    text: impl Into<egui::WidgetText>,
) {
    let mut tip = tip.gap(LAB_TOOLTIP_GAP);
    match anchor {
        Some(rect) => tip.popup = tip.popup.anchor(rect),
        None => tip = tip.at_pointer(),
    }
    tip.popup = tip.popup.frame(lab_tooltip_frame());
    tip.show(|ui| {
        apply_lab_dark_ui(ui);
        ui.set_max_width(ui.spacing().tooltip_width);
        ui.add(egui::Label::new(text));
    });
}

trait LabHoverTipExt {
    fn lab_hover_tip(self, text: impl Into<egui::WidgetText>) -> Self;
}

impl LabHoverTipExt for egui::Response {
    fn lab_hover_tip(self, text: impl Into<egui::WidgetText>) -> Self {
        let anchor = self
            .contains_pointer()
            .then(|| lab_cursor_anchor_rect(&self.ctx))
            .flatten();
        show_lab_offset_tooltip(egui::Tooltip::for_enabled(&self), anchor, text);
        self
    }
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

fn raster_vector_edit_controls_visible(
    selected_mask_kind: Option<MaskKind>,
    override_edit_panel: Option<OverrideEditTarget>,
) -> bool {
    selected_mask_kind == Some(MaskKind::Raster) || override_edit_panel.is_some()
}

struct LoadedImage {
    path: PathBuf,
    source: RgbaImageBuf,
}

struct RenderPending {
    generation: u64,
    rx: mpsc::Receiver<RenderWorkerMessage>,
    cancel: Arc<AtomicBool>,
    started_at: Instant,
}

struct RenderProgress {
    generation: u64,
    layer_index: usize,
    layer_count: usize,
    effect_name: String,
    percent: f32,
}

enum RenderWorkerMessage {
    Progress(RenderProgress),
    Done(Result<RgbaImageBuf, String>),
}

struct SegmentationPending {
    layer_idx: usize,
    generation: u64,
    rx: mpsc::Receiver<Result<GeneratedMask, String>>,
    started_at: Instant,
}

struct LutLoadPending {
    layer_idx: usize,
    generation: u64,
    rx: mpsc::Receiver<Result<CubeLutParams, String>>,
    started_at: Instant,
    path: PathBuf,
}

enum GeneratedMask {
    Subject(RasterMask),
    Regions(RegionMask),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RegionSegmentationScope {
    Full,
    Subject,
    Background,
}

impl RegionSegmentationScope {
    fn requires_subject(self) -> bool {
        matches!(self, Self::Subject | Self::Background)
    }

    fn pending_label(self) -> &'static str {
        match self {
            Self::Full => "画像全体を領域分割中...",
            Self::Subject => "被写体内を領域分割中...",
            Self::Background => "背景を領域分割中...",
        }
    }
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
    crop_aspect_mode: CropAspectMode,
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
    #[serde(default)]
    mask_before_effect: bool,
    #[serde(default = "default_mask_after_effect")]
    mask_after_effect: bool,
    effect: LocalEffect,
}

fn default_mask_after_effect() -> bool {
    true
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct LabHistory {
    #[serde(default)]
    recent_files: Vec<PathBuf>,
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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    source_alpha_u8_deflate_b64: Option<String>,
    #[serde(default)]
    refinement: SubjectMaskRefinement,
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
enum CropAspectMode {
    Keep,
    #[default]
    Free,
    Landscape16x9,
    Landscape3x2,
    Landscape4x3,
    Square,
    Portrait3x4,
    Portrait2x3,
    Portrait9x16,
}

impl CropAspectMode {
    const ALL: [Self; 9] = [
        Self::Keep,
        Self::Free,
        Self::Landscape16x9,
        Self::Landscape3x2,
        Self::Landscape4x3,
        Self::Square,
        Self::Portrait3x4,
        Self::Portrait2x3,
        Self::Portrait9x16,
    ];

    fn label(self) -> &'static str {
        match self {
            Self::Keep => "維持",
            Self::Free => "自由",
            Self::Landscape16x9 => "16:9",
            Self::Landscape3x2 => "3:2",
            Self::Landscape4x3 => "4:3",
            Self::Square => "1:1",
            Self::Portrait3x4 => "3:4",
            Self::Portrait2x3 => "2:3",
            Self::Portrait9x16 => "9:16",
        }
    }

    fn aspect_ratio(self) -> Option<f32> {
        match self {
            Self::Keep | Self::Free => None,
            Self::Landscape16x9 => Some(16.0 / 9.0),
            Self::Landscape3x2 => Some(3.0 / 2.0),
            Self::Landscape4x3 => Some(4.0 / 3.0),
            Self::Square => Some(1.0),
            Self::Portrait3x4 => Some(3.0 / 4.0),
            Self::Portrait2x3 => Some(2.0 / 3.0),
            Self::Portrait9x16 => Some(9.0 / 16.0),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LabWorkflowPanel {
    Eraser,
    Adjust,
    Conceal,
    Crop,
    Save,
}

impl LabWorkflowPanel {
    const ALL: [Self; 5] = [
        Self::Eraser,
        Self::Adjust,
        Self::Conceal,
        Self::Crop,
        Self::Save,
    ];

    fn label(self) -> &'static str {
        match self {
            Self::Eraser => "消しゴム",
            Self::Adjust => "補正",
            Self::Conceal => "隠蔽",
            Self::Crop => "切り取り",
            Self::Save => "保存",
        }
    }
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
    aspect_ratio: Option<f32>,
}

#[derive(Debug, Clone, Copy)]
struct CropCreateDrag {
    start: [f32; 2],
}

/// What a fresh primary-button press inside the canvas should start, given where it
/// landed relative to the current crop. Resolved by [`crop_press_target`] so the branch
/// can be unit-tested without an egui input harness.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CropPressTarget {
    Resize(CropHandle),
    Move,
    Create,
}

/// The single in-progress crop gesture. `draw_crop_overlay` keeps this as the source of
/// truth so a gesture survives `crop_is_active()` flipping mid-drag, and so the whole
/// press/drag/release state machine can be exercised by [`crop_gesture_step`] tests
/// without an egui input harness.
#[derive(Debug, Clone, Copy)]
enum CropGesture {
    Idle,
    /// Dragging a resize handle, or moving the body (`CropHandle::Body`).
    Resize(CropDrag),
    /// Rubber-banding a brand new crop from `start` (image coordinates).
    Create(CropCreateDrag),
}

/// Per-frame inputs to [`crop_gesture_step`], all already resolved from egui's raw pointer
/// state by the caller so the reducer itself stays pure.
struct CropGestureInput {
    primary_pressed: bool,
    primary_down: bool,
    /// What the *press origin* hit (handle / body / empty image). Only consulted when a
    /// gesture begins, and computed from `press_origin`, not the live pointer.
    press_target: Option<CropPressTarget>,
    /// Press origin in image coordinates (create anchor).
    press_image: Option<Pos2>,
    /// Live pointer in image coordinates (create's moving corner).
    current_image: Option<Pos2>,
    /// Whether the pointer has travelled far enough from the press origin to commit a
    /// create. Guards against a plain click collapsing into a 1px crop.
    create_moved_enough: bool,
    /// Crop rect at the instant the gesture starts (resize/move base).
    base_at_press: CropRect,
    resize_aspect: Option<f32>,
    create_aspect: Option<f32>,
    /// Cumulative pointer travel since the press, already scaled to image pixels.
    total_delta_image: (f32, f32),
    img_w: usize,
    img_h: usize,
}

/// Advance the crop gesture state machine by one frame.
///
/// Returns the next gesture plus the new crop rect (if this frame changed it; `None`
/// leaves the existing rect untouched, e.g. a click that never became a drag, or a
/// released gesture). The reducer is deliberately free of `crop_is_active()`: once a
/// create drag is latched it stays a create drag, which is what fixes the original
/// "crop snaps back to full while dragging" bug.
fn crop_gesture_step(
    gesture: CropGesture,
    input: &CropGestureInput,
) -> (CropGesture, Option<CropRect>) {
    let mut gesture = gesture;

    // Begin a gesture on a fresh press (only when nothing is already latched).
    if input.primary_pressed && matches!(gesture, CropGesture::Idle) {
        gesture = match input.press_target {
            Some(CropPressTarget::Resize(handle)) => CropGesture::Resize(CropDrag {
                handle,
                base: input.base_at_press,
                aspect_ratio: input.resize_aspect,
            }),
            Some(CropPressTarget::Move) => CropGesture::Resize(CropDrag {
                handle: CropHandle::Body,
                base: input.base_at_press,
                aspect_ratio: None,
            }),
            Some(CropPressTarget::Create) => match input.press_image {
                Some(p) => CropGesture::Create(CropCreateDrag { start: [p.x, p.y] }),
                None => CropGesture::Idle,
            },
            None => CropGesture::Idle,
        };
    }

    // The button is up: end any gesture, leaving the last committed rect in place.
    if !input.primary_down {
        return (CropGesture::Idle, None);
    }

    match gesture {
        CropGesture::Resize(drag) => {
            let next = drag.base.dragged(
                drag.handle,
                input.total_delta_image.0,
                input.total_delta_image.1,
                input.img_w,
                input.img_h,
                drag.aspect_ratio,
            );
            (gesture, Some(next))
        }
        CropGesture::Create(create) => {
            if input.create_moved_enough
                && let Some(cur) = input.current_image
            {
                let next = crop_from_points(
                    create.start,
                    [cur.x, cur.y],
                    input.img_w,
                    input.img_h,
                    input.create_aspect,
                );
                (gesture, Some(next))
            } else {
                (gesture, None)
            }
        }
        CropGesture::Idle => (gesture, None),
    }
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
            Self::Add => "追加マスク",
            Self::Subtract => "削除マスク",
        }
    }
}

#[derive(Debug, Clone, Copy)]
struct MaskPreviewColors {
    base_rgb: [u8; 3],
    edit_rgb: [u8; 3],
    boundary_rgb: [u8; 3],
}

impl MaskPreviewColors {
    fn base(self, alpha: u8) -> Color32 {
        Color32::from_rgba_unmultiplied(self.base_rgb[0], self.base_rgb[1], self.base_rgb[2], alpha)
    }

    fn edit(self, alpha: u8) -> Color32 {
        Color32::from_rgba_unmultiplied(self.edit_rgb[0], self.edit_rgb[1], self.edit_rgb[2], alpha)
    }

    fn boundary(self, alpha: u8) -> Color32 {
        Color32::from_rgba_unmultiplied(
            self.boundary_rgb[0],
            self.boundary_rgb[1],
            self.boundary_rgb[2],
            alpha,
        )
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum MaskColorPreset {
    PinkCyan,
    CyanOrange,
    YellowViolet,
}

impl MaskColorPreset {
    const ALL: [Self; 3] = [Self::PinkCyan, Self::CyanOrange, Self::YellowViolet];

    fn label(self) -> &'static str {
        match self {
            Self::PinkCyan => "1",
            Self::CyanOrange => "2",
            Self::YellowViolet => "3",
        }
    }

    fn description(self) -> &'static str {
        match self {
            Self::PinkCyan => "ピンク / 水色",
            Self::CyanOrange => "シアン / オレンジ",
            Self::YellowViolet => "黄 / 紫",
        }
    }

    fn colors(self) -> MaskPreviewColors {
        match self {
            Self::PinkCyan => MaskPreviewColors {
                base_rgb: [255, 48, 84],
                edit_rgb: [64, 190, 255],
                boundary_rgb: [255, 245, 120],
            },
            Self::CyanOrange => MaskPreviewColors {
                base_rgb: [0, 205, 255],
                edit_rgb: [255, 150, 40],
                boundary_rgb: [255, 235, 80],
            },
            Self::YellowViolet => MaskPreviewColors {
                base_rgb: [255, 225, 40],
                edit_rgb: [185, 115, 255],
                boundary_rgb: [80, 230, 255],
            },
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
    RgbToneCurve,
    ColorBalance,
    ThreeWayColorGrading,
    SelectiveColor,
    ChannelMixer,
    Clarity,
    Texture,
    HighPass,
    HighlightsShadows,
    Dehaze,
    Blur,
    MotionBlur,
    Wind,
    SpeedLines,
    TiltShift,
    LensBlur,
    RadialBlur,
    WaveDistortion,
    PinchSpherize,
    Twirl,
    PolarCoordinates,
    GlassDisplacement,
    LensCorrection,
    LineExtract,
    ArtisticMedia,
    BrushStroke,
    Cutout,
    Emboss,
    PixelStylize,
    Solarize,
    GlowingEdges,
    OilPaint,
    SoftFocus,
    Mosaic,
    Sharpen,
    SmartSharpen,
    Hsl,
    ColorMixer,
    Look,
    CubeLut,
    Posterize,
    Threshold,
    Invert,
    Duotone,
    Equalize,
    GradientMap,
    ColorFill,
    OutlineStroke,
    ColorOverlay,
    NeonGlow,
    DiffuseGlow,
    Bloom,
    GodRays,
    LensFlare,
    CloudFog,
    Spotlight,
    Vignette,
    FilmGrain,
    Noise,
    ChromaticAberration,
    Halftone,
    ScreenTone,
    ColorHalftone,
    Textureizer,
    StarGlow,
    EdgeSmooth,
    Despeckle,
    Median,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RgbPickTarget {
    ColorFillStart,
    ColorFillMiddle,
    ColorFillEnd,
    ColorOverlayStart,
    ColorOverlayEnd,
    NeonGlowSource,
    NeonGlowTint,
    SpeedLinesColor,
    CloudFogColor,
    SpotlightTint,
    OutlineStrokeColor,
}

impl RgbPickTarget {
    fn label(self) -> &'static str {
        match self {
            Self::ColorFillStart => "塗りつぶしの開始色",
            Self::ColorFillMiddle => "塗りつぶしの中間色",
            Self::ColorFillEnd => "塗りつぶしの終了色",
            Self::ColorOverlayStart => "塗り/グラデーションの開始色",
            Self::ColorOverlayEnd => "塗り/グラデーションの終了色",
            Self::NeonGlowSource => "ネオングローの発光源色",
            Self::NeonGlowTint => "ネオングローの着色",
            Self::SpeedLinesColor => "集中線/スピード線の線色",
            Self::CloudFogColor => "雲/霧の色",
            Self::SpotlightTint => "スポットライトの光色",
            Self::OutlineStrokeColor => "縁取りの線色",
        }
    }
}

impl EffectKind {
    fn from_effect(effect: &LocalEffect) -> Self {
        match effect {
            LocalEffect::None => Self::None,
            LocalEffect::Tone(_) => Self::Tone,
            LocalEffect::ToneCurve(_) => Self::ToneCurve,
            LocalEffect::RgbToneCurve(_) => Self::RgbToneCurve,
            LocalEffect::ColorBalance(_) => Self::ColorBalance,
            LocalEffect::ThreeWayColorGrading(_) => Self::ThreeWayColorGrading,
            LocalEffect::SelectiveColor(_) => Self::SelectiveColor,
            LocalEffect::ChannelMixer(_) => Self::ChannelMixer,
            LocalEffect::Clarity(_) => Self::Clarity,
            LocalEffect::Texture(_) => Self::Texture,
            LocalEffect::HighPass(_) => Self::HighPass,
            LocalEffect::HighlightsShadows(_) => Self::HighlightsShadows,
            LocalEffect::Dehaze(_) => Self::Dehaze,
            LocalEffect::Blur(_) => Self::Blur,
            LocalEffect::MotionBlur(_) => Self::MotionBlur,
            LocalEffect::Wind(_) => Self::Wind,
            LocalEffect::SpeedLines(_) => Self::SpeedLines,
            LocalEffect::TiltShift(_) => Self::TiltShift,
            LocalEffect::LensBlur(_) => Self::LensBlur,
            LocalEffect::RadialBlur(_) => Self::RadialBlur,
            LocalEffect::WaveDistortion(_) => Self::WaveDistortion,
            LocalEffect::PinchSpherize(_) => Self::PinchSpherize,
            LocalEffect::Twirl(_) => Self::Twirl,
            LocalEffect::PolarCoordinates(_) => Self::PolarCoordinates,
            LocalEffect::GlassDisplacement(_) => Self::GlassDisplacement,
            LocalEffect::LensCorrection(_) => Self::LensCorrection,
            LocalEffect::LineExtract(_) => Self::LineExtract,
            LocalEffect::ArtisticMedia(_) => Self::ArtisticMedia,
            LocalEffect::BrushStroke(_) => Self::BrushStroke,
            LocalEffect::Cutout(_) => Self::Cutout,
            LocalEffect::Emboss(_) => Self::Emboss,
            LocalEffect::PixelStylize(_) => Self::PixelStylize,
            LocalEffect::Solarize(_) => Self::Solarize,
            LocalEffect::GlowingEdges(_) => Self::GlowingEdges,
            LocalEffect::OilPaint(_) => Self::OilPaint,
            LocalEffect::SoftFocus(_) => Self::SoftFocus,
            LocalEffect::Mosaic(_) => Self::Mosaic,
            LocalEffect::Sharpen(_) => Self::Sharpen,
            LocalEffect::SmartSharpen(_) => Self::SmartSharpen,
            LocalEffect::Hsl(_) => Self::Hsl,
            LocalEffect::ColorMixer(_) => Self::ColorMixer,
            LocalEffect::Look(_) => Self::Look,
            LocalEffect::CubeLut(_) => Self::CubeLut,
            LocalEffect::Posterize(_) => Self::Posterize,
            LocalEffect::Threshold(_) => Self::Threshold,
            LocalEffect::Invert(_) => Self::Invert,
            LocalEffect::Duotone(_) => Self::Duotone,
            LocalEffect::Equalize(_) => Self::Equalize,
            LocalEffect::GradientMap(_) => Self::GradientMap,
            LocalEffect::ColorFill(_) => Self::ColorFill,
            LocalEffect::OutlineStroke(_) => Self::OutlineStroke,
            LocalEffect::ColorOverlay(_) => Self::ColorOverlay,
            LocalEffect::NeonGlow(_) => Self::NeonGlow,
            LocalEffect::DiffuseGlow(_) => Self::DiffuseGlow,
            LocalEffect::Bloom(_) => Self::Bloom,
            LocalEffect::GodRays(_) => Self::GodRays,
            LocalEffect::LensFlare(_) => Self::LensFlare,
            LocalEffect::CloudFog(_) => Self::CloudFog,
            LocalEffect::Spotlight(_) => Self::Spotlight,
            LocalEffect::Vignette(_) => Self::Vignette,
            LocalEffect::FilmGrain(_) => Self::FilmGrain,
            LocalEffect::Noise(_) => Self::Noise,
            LocalEffect::ChromaticAberration(_) => Self::ChromaticAberration,
            LocalEffect::Halftone(_) => Self::Halftone,
            LocalEffect::ScreenTone(_) => Self::ScreenTone,
            LocalEffect::ColorHalftone(_) => Self::ColorHalftone,
            LocalEffect::Textureizer(_) => Self::Textureizer,
            LocalEffect::StarGlow(_) => Self::StarGlow,
            LocalEffect::EdgeSmooth(_) => Self::EdgeSmooth,
            LocalEffect::Despeckle(_) => Self::Despeckle,
            LocalEffect::Median(_) => Self::Median,
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::None => "効果なし",
            Self::Tone => "色調補正",
            Self::ToneCurve => "トーンカーブ",
            Self::RgbToneCurve => "RGBカーブ",
            Self::ColorBalance => "カラーバランス",
            Self::ThreeWayColorGrading => "3-wayグレーディング",
            Self::SelectiveColor => "セレクティブカラー",
            Self::ChannelMixer => "チャンネルミキサー",
            Self::Clarity => "明瞭度",
            Self::Texture => "テクスチャ",
            Self::HighPass => "ハイパス",
            Self::HighlightsShadows => "ハイライト/シャドウ",
            Self::Dehaze => "かすみ除去",
            Self::Blur => "ぼかし",
            Self::MotionBlur => "移動ぼかし",
            Self::Wind => "風/スピード",
            Self::SpeedLines => "集中線/スピード線",
            Self::TiltShift => "チルトシフト",
            Self::LensBlur => "レンズぼかし",
            Self::RadialBlur => "放射/回転ぼかし",
            Self::WaveDistortion => "波形ゆがみ",
            Self::PinchSpherize => "つまむ/魚眼",
            Self::Twirl => "渦巻き",
            Self::PolarCoordinates => "極座標",
            Self::GlassDisplacement => "ガラス/変位",
            Self::LensCorrection => "レンズ補正",
            Self::LineExtract => "線画抽出",
            Self::ArtisticMedia => "水彩/鉛筆",
            Self::BrushStroke => "ドライブラシ/塗料",
            Self::Cutout => "切り絵",
            Self::Emboss => "エンボス",
            Self::PixelStylize => "粒状スタイル",
            Self::Solarize => "ソラリゼーション",
            Self::GlowingEdges => "エッジ光彩",
            Self::OilPaint => "油彩",
            Self::SoftFocus => "ソフトフォーカス",
            Self::Mosaic => "モザイク",
            Self::Sharpen => "シャープ",
            Self::SmartSharpen => "スマートシャープ",
            Self::Hsl => "色相/HSL",
            Self::ColorMixer => "カラーミキサー",
            Self::Look => "ルック",
            Self::CubeLut => "3D LUT",
            Self::Posterize => "ポスタリゼーション",
            Self::Threshold => "2値化",
            Self::Invert => "階調反転/ネガ",
            Self::Duotone => "ダブルトーン",
            Self::Equalize => "ヒストグラム平坦化",
            Self::GradientMap => "グラデーションマップ",
            Self::ColorFill => "塗りつぶし",
            Self::OutlineStroke => "縁取り",
            Self::ColorOverlay => "塗り/グラデーション",
            Self::NeonGlow => "ネオングロー",
            Self::DiffuseGlow => "拡散光彩",
            Self::Bloom => "ブルーム",
            Self::GodRays => "光芒",
            Self::LensFlare => "レンズフレア",
            Self::CloudFog => "雲/霧",
            Self::Spotlight => "スポットライト",
            Self::Vignette => "ビネット",
            Self::FilmGrain => "フィルム粒子",
            Self::Noise => "ノイズ付加",
            Self::ChromaticAberration => "色収差",
            Self::Halftone => "ハーフトーン",
            Self::ScreenTone => "スクリーントーン",
            Self::ColorHalftone => "カラーハーフトーン",
            Self::Textureizer => "テクスチャライザ",
            Self::StarGlow => "クロス光",
            Self::EdgeSmooth => "エッジ保持ぼかし",
            Self::Despeckle => "ディスペックル",
            Self::Median => "メディアン",
        }
    }

    fn description(self) -> &'static str {
        match self {
            Self::None => "このレイヤーで加工を行わず、マスクだけを準備します。",
            Self::Tone => "明るさ、コントラスト、彩度、色温度などをまとめて調整します。",
            Self::ToneCurve => "暗部から明部までの明るさをカーブで細かく調整します。",
            Self::RgbToneCurve => "赤、緑、青のチャンネル別カーブで色味と明暗を細かく調整します。",
            Self::ColorBalance => "シャドウ、中間、ハイライトごとに色の偏りを調整します。",
            Self::ThreeWayColorGrading => {
                "シャドウ、中間、ハイライトに別々の色味と明るさを足して空気感を作ります。"
            }
            Self::SelectiveColor => "指定した色相の近くにある色だけを狙って調整します。",
            Self::ChannelMixer => "RGBチャンネルの寄与率を変え、色変換や本格的な白黒化を行います。",
            Self::Clarity => "局所コントラストを上げ、輪郭や質感をくっきり見せます。",
            Self::Texture => {
                "中くらいの細かさの質感だけを強めたり弱めたりします。肌や塗り面のざらつき調整に使います。"
            }
            Self::HighPass => {
                "中間グレーのハイパス抽出を使い、輪郭や細部をオーバーレイ合成で引き締めます。"
            }
            Self::HighlightsShadows => "明るい部分と暗い部分を個別に持ち上げたり抑えたりします。",
            Self::Dehaze => "白っぽさを減らし、遠景や薄いコントラストを締めます。",
            Self::Blur => "選択範囲を均一にぼかします。背景ぼかしや軽い隠しに使います。",
            Self::MotionBlur => "指定した方向へ流れるようにぼかし、動きや速度感を加えます。",
            Self::Wind => {
                "明部・暗部・輪郭を片方向へ引きずり、風やスピード感のある流線を作ります。"
            }
            Self::SpeedLines => {
                "中心へ向かう集中線や、指定方向へ流れる平行スピード線を自動生成します。"
            }
            Self::TiltShift => {
                "焦点帯を残して周囲をぼかし、浅い被写界深度やジオラマ風の見た目を作ります。"
            }
            Self::LensBlur => "絞り形状でぼかし、明るい点を玉ボケのように膨らませます。",
            Self::RadialBlur => {
                "中心から外へ伸びるズームぼかし、または中心周りの回転ぼかしを作ります。"
            }
            Self::WaveDistortion => {
                "波やさざ波のように画像を揺らし、水面・反射・熱気の表現に使います。"
            }
            Self::PinchSpherize => {
                "中心を基準にふくらませたり、つまんだりして、魚眼レンズや誇張表現を作ります。"
            }
            Self::Twirl => {
                "中心を基準に画像を渦巻き状に回転させ、渦や魔法陣のような演出を作ります。"
            }
            Self::PolarCoordinates => {
                "矩形画像を円形構図へ巻き、または円形構図を横長の極座標画像へ展開します。"
            }
            Self::GlassDisplacement => {
                "手続き型の変位マップで画像をずらし、すりガラスや波打つガラス越しの歪みを作ります。"
            }
            Self::LensCorrection => {
                "樽型・糸巻き型のレンズ歪みを補正し、必要なら周辺減光も持ち上げます。"
            }
            Self::LineExtract => {
                "明るさのエッジを検出して線画を作り、白地・黒地・元画像への重ねに使えます。"
            }
            Self::ArtisticMedia => "水彩、色鉛筆、鉛筆画の質感で、写真や3D素材を絵画調に寄せます。",
            Self::BrushStroke => {
                "方向のある筆跡で、ドライブラシ、厚塗り、パレットナイフ風の質感を作ります。"
            }
            Self::Cutout => {
                "色面をなじませて階調を減らし、切り絵やフラットなベクター調の見た目にします。"
            }
            Self::Emboss => "明るさの傾きから陰影を作り、紙や金属の浮き彫りのような質感にします。",
            Self::PixelStylize => "結晶化、点描、Facet、メゾチントの粒状スタイライズを作ります。",
            Self::Solarize => "明るい部分を反転させ、写真暗室風のトーン反転や特殊色を作ります。",
            Self::GlowingEdges => {
                "輪郭を抽出してネオン色で光らせ、黒背景や元画像上の発光線を作ります。"
            }
            Self::OilPaint => {
                "Kuwahara フィルタで色面を選択的になじませ、油彩のような塗り感を作ります。"
            }
            Self::SoftFocus => "ぼかした画像を重ね、柔らかく発光したような印象にします。",
            Self::Mosaic => "選択範囲をモザイク化します。隠蔽加工と同じ境界処理を選べます。",
            Self::Sharpen => "輪郭を強調して、少し眠い画像を引き締めます。",
            Self::SmartSharpen => {
                "輪郭を検出してシャープをかけ、白フチや黒フチを抑えながら細部を引き締めます。"
            }
            Self::Hsl => "色相、彩度、明度を調整し、髪色変更などの色替えに使います。",
            Self::ColorMixer => "赤、黄、緑、青などの色帯ごとに色相、彩度、明度を調整します。",
            Self::Look => "夕焼け、夜景、フィルム風などのまとまった色味を適用します。",
            Self::CubeLut => {
                ".cube 形式の外部3D LUTを読み込み、配布LUTや映画風の色味を適用します。"
            }
            Self::Posterize => "色の階調数を減らし、フラットでグラフィックな見た目にします。",
            Self::Threshold => "明るさをしきい値で黒と白に分け、線画やモノクロ風にします。",
            Self::Invert => "RGBの明暗を反転し、ネガフィルムのような見た目にします。",
            Self::Duotone => "明暗を2色または3色へ置き換え、印刷やポスターのような色味にします。",
            Self::Equalize => {
                "明暗の分布を広げ、自動補正のように眠い画像のコントラストを整えます。"
            }
            Self::GradientMap => {
                "明るさを指定したグラデーションの色へ置き換え、色設計や色トレス風に使います。"
            }
            Self::ColorFill => {
                "マスク範囲を単色、線形グラデーション、円形グラデーションで塗りつぶします。"
            }
            Self::OutlineStroke => {
                "マスク境界から外側・内側・中央の色枠を作り、被写体のステッカー風分離に使えます。"
            }
            Self::ColorOverlay => {
                "単色やグラデーションの色面を、乗算・スクリーン・ソフトライトなどで重ねます。"
            }
            Self::NeonGlow => {
                "明るい部分や高彩度の色を拾い、色付きの内側グローと外側ハローを作ります。"
            }
            Self::DiffuseGlow => {
                "明るい部分を白く柔らかく拡散し、粒状感のある夢幻的な光彩を作ります。"
            }
            Self::Bloom => "明るい部分を周囲へにじませ、発光感を足します。",
            Self::GodRays => "明るい部分から光源方向に沿った放射状の光芒を作ります。",
            Self::LensFlare => "光源のにじみ、ハロー、レンズ内反射のゴーストを重ねます。",
            Self::CloudFog => "手続き型のノイズで霧や雲を重ね、大気感と遠近感を加えます。",
            Self::Spotlight => "指定した中心を照らし、周辺を落として局所的な光を作ります。",
            Self::Vignette => "周辺を暗く、または明るくして視線を中央へ誘導します。",
            Self::FilmGrain => "粒状感を加え、フィルムや紙っぽい質感を作ります。",
            Self::Noise => {
                "均一またはガウス分布のノイズを加え、単色/カラーのざらつきやデジタルノイズを作ります。"
            }
            Self::ChromaticAberration => "RGBを少しずらし、レンズやデジタル風の色ズレを作ります。",
            Self::Halftone => "明るさをドットパターンに変換し、漫画や印刷風にします。",
            Self::ScreenTone => {
                "網点、平行線、カケアミを重ね、濃度と元画像の明暗追従で漫画用のトーンを作ります。"
            }
            Self::ColorHalftone => {
                "CMYK 4版の角度違いドットで、ポップアートやアメコミ風の印刷網点を作ります。"
            }
            Self::Textureizer => {
                "紙目、キャンバス、リネンの手続き型テクスチャをソフトライトで重ね、手描き感や紙質を足します。"
            }
            Self::StarGlow => "明るい部分から十字や多方向の光線を描写します。",
            Self::EdgeSmooth => "輪郭をなるべく残しながら面をなめらかにします。",
            Self::Despeckle => {
                "周囲から大きく外れた孤立点だけを中央値へ寄せ、スキャンの白点・黒点を目立ちにくくします。"
            }
            Self::Median => "孤立した点ノイズや細かいゴミを、周囲の中央値で目立ちにくくします。",
        }
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
        title: "範囲選択",
        kinds: &[MaskKind::LumaRange, MaskKind::ColorRange],
    },
    MaskGroup {
        title: "自動・領域",
        kinds: &[MaskKind::Subject, MaskKind::Segmentation],
    },
];

struct EffectGroup {
    title: &'static str,
    kinds: &'static [EffectKind],
}

const EFFECT_GROUPS: &[EffectGroup] = &[
    EffectGroup {
        title: "基本",
        kinds: &[
            EffectKind::None,
            EffectKind::ColorFill,
            EffectKind::OutlineStroke,
        ],
    },
    EffectGroup {
        title: "色調補正",
        kinds: &[
            EffectKind::Tone,
            EffectKind::ToneCurve,
            EffectKind::RgbToneCurve,
            EffectKind::ColorBalance,
            EffectKind::ThreeWayColorGrading,
            EffectKind::SelectiveColor,
            EffectKind::ChannelMixer,
            EffectKind::Hsl,
            EffectKind::ColorMixer,
            EffectKind::HighlightsShadows,
            EffectKind::Dehaze,
            EffectKind::Equalize,
        ],
    },
    EffectGroup {
        title: "色変換・ルック",
        kinds: &[
            EffectKind::Look,
            EffectKind::CubeLut,
            EffectKind::GradientMap,
            EffectKind::Posterize,
            EffectKind::Threshold,
            EffectKind::Invert,
            EffectKind::Duotone,
        ],
    },
    EffectGroup {
        title: "ぼかし・フォーカス",
        kinds: &[
            EffectKind::Blur,
            EffectKind::MotionBlur,
            EffectKind::TiltShift,
            EffectKind::LensBlur,
            EffectKind::RadialBlur,
            EffectKind::SoftFocus,
            EffectKind::EdgeSmooth,
            EffectKind::Despeckle,
            EffectKind::Median,
        ],
    },
    EffectGroup {
        title: "シャープ・ディテール",
        kinds: &[
            EffectKind::Clarity,
            EffectKind::Texture,
            EffectKind::HighPass,
            EffectKind::Sharpen,
            EffectKind::SmartSharpen,
        ],
    },
    EffectGroup {
        title: "変形・歪み",
        kinds: &[
            EffectKind::WaveDistortion,
            EffectKind::PinchSpherize,
            EffectKind::Twirl,
            EffectKind::PolarCoordinates,
            EffectKind::GlassDisplacement,
            EffectKind::LensCorrection,
        ],
    },
    EffectGroup {
        title: "表現・絵画調",
        kinds: &[
            EffectKind::Wind,
            EffectKind::SpeedLines,
            EffectKind::LineExtract,
            EffectKind::ArtisticMedia,
            EffectKind::BrushStroke,
            EffectKind::Cutout,
            EffectKind::Emboss,
            EffectKind::PixelStylize,
            EffectKind::Solarize,
            EffectKind::GlowingEdges,
            EffectKind::OilPaint,
            EffectKind::Halftone,
            EffectKind::ScreenTone,
            EffectKind::ColorHalftone,
            EffectKind::Textureizer,
        ],
    },
    EffectGroup {
        title: "隠蔽・加工",
        kinds: &[EffectKind::Mosaic],
    },
    EffectGroup {
        title: "光・雰囲気",
        kinds: &[
            EffectKind::ColorOverlay,
            EffectKind::NeonGlow,
            EffectKind::DiffuseGlow,
            EffectKind::Bloom,
            EffectKind::GodRays,
            EffectKind::LensFlare,
            EffectKind::CloudFog,
            EffectKind::Spotlight,
            EffectKind::StarGlow,
            EffectKind::Vignette,
            EffectKind::FilmGrain,
            EffectKind::Noise,
            EffectKind::ChromaticAberration,
        ],
    },
];

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
    lut_load_pending: Option<LutLoadPending>,
    generation: u64,
    result_dirty: bool,
    mask_dirty: bool,
    last_edit: Instant,
    last_mask_preview_update: Instant,
    workflow_panel: LabWorkflowPanel,
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
    effect_gradient_drag_active: bool,
    tilt_shift_drag_active: bool,
    effect_position_handles_visible: bool,
    boundary_edge_threshold: f32,
    boundary_ink_threshold: f32,
    boundary_gap_px: f32,
    edge_snap_radius: f32,
    region_color_tolerance: f32,
    region_min_area: usize,
    subject_cutout_edit_active: bool,
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
    mask_color_preset: MaskColorPreset,
    preview_to_selected_layer: bool,
    crop_enabled: bool,
    crop_overlay: bool,
    crop_edit_mode: bool,
    crop_aspect_mode: CropAspectMode,
    crop_rect: Option<CropRect>,
    crop_drag: Option<CropDrag>,
    crop_create_drag: Option<CropCreateDrag>,
    add_layer_dialog_open: bool,
    add_layer_mask_kind: MaskKind,
    effect_picker_dialog_open: bool,
    selective_color_pick_active: bool,
    rgb_pick_active: Option<RgbPickTarget>,
    recent_files: Vec<PathBuf>,
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
        let recent_files = load_recent_files();
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
            lut_load_pending: None,
            generation: 0,
            result_dirty: false,
            mask_dirty: false,
            last_edit: Instant::now(),
            last_mask_preview_update: Instant::now(),
            workflow_panel: LabWorkflowPanel::Adjust,
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
            effect_gradient_drag_active: false,
            tilt_shift_drag_active: false,
            effect_position_handles_visible: true,
            boundary_edge_threshold: 24.0,
            boundary_ink_threshold: 28.0,
            boundary_gap_px: 2.0,
            edge_snap_radius: 16.0,
            region_color_tolerance: 42.0,
            region_min_area: 64,
            subject_cutout_edit_active: false,
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
            mask_color_preset: MaskColorPreset::PinkCyan,
            preview_to_selected_layer: false,
            crop_enabled: false,
            crop_overlay: true,
            crop_edit_mode: false,
            crop_aspect_mode: CropAspectMode::Free,
            crop_rect: None,
            crop_drag: None,
            crop_create_drag: None,
            add_layer_dialog_open: false,
            add_layer_mask_kind: MaskKind::Full,
            effect_picker_dialog_open: false,
            selective_color_pick_active: false,
            rgb_pick_active: None,
            recent_files,
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
                let history_save = self.remember_recent_file(path);
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
                self.cancel_pending_render();
                self.segmentation_pending = None;
                self.lut_load_pending = None;
                self.image = Some(loaded);
                self.subject_cutout_edit_active = false;
                self.undo_stack.clear();
                self.redo_stack.clear();
                self.view_zoom = 1.0;
                self.view_pan = egui::Vec2::ZERO;
                self.pan_drag_start = None;
                self.crop_enabled = false;
                self.crop_overlay = true;
                self.crop_edit_mode = false;
                self.crop_aspect_mode = CropAspectMode::Free;
                self.crop_rect = None;
                self.crop_drag = None;
                self.crop_create_drag = None;
                self.prev_paint_pos = None;
                self.last_paint_pos = None;
                self.workflow_panel = LabWorkflowPanel::Adjust;
                self.override_edit_panel = None;
                self.radial_gradient_drag_active = false;
                self.effect_gradient_drag_active = false;
                self.tilt_shift_drag_active = false;
                self.edge_brush_seed = None;
                self.selective_color_pick_active = false;
                self.rgb_pick_active = None;
                let load_status = format!("読み込み: {}", path.display());
                self.status = load_status.clone();
                let sidecar_path = sidecar_path_for_image(path);
                match self.load_settings_sidecar_from_path(&sidecar_path) {
                    Ok(true) => {}
                    Ok(false) => self.status = load_status,
                    Err(e) => self.status = format!("{load_status} / 設定読込失敗: {e}"),
                }
                if let Err(e) = history_save {
                    self.status = format!("{} / 履歴保存失敗: {e}", self.status);
                }
                self.mark_dirty();
            }
            Err(e) => {
                self.status = format!("読み込み失敗: {e}");
            }
        }
    }

    fn remember_recent_file(&mut self, path: &Path) -> Result<(), String> {
        push_recent_file(&mut self.recent_files, path);
        save_recent_files(&self.recent_files)
    }

    fn draw_menu_bar(&mut self, ctx: &egui::Context) {
        let recent_files = self.recent_files.clone();
        let current_path = self.image.as_ref().map(|image| image.path.clone());
        let mut path_to_load = None;

        egui::TopBottomPanel::top("local_adjust_lab_menubar").show(ctx, |ui| {
            apply_lab_dark_ui(ui);
            egui::MenuBar::new().ui(ui, |ui| {
                ui.menu_button("読み込み", |ui| {
                    if ui.button("画像を開く...").clicked() {
                        let mut dialog = rfd::FileDialog::new()
                            .add_filter("画像", &["jpg", "jpeg", "png"])
                            .set_title("画像を選択");
                        if let Some(parent) = current_path.as_ref().and_then(|path| path.parent()) {
                            dialog = dialog.set_directory(parent);
                        }
                        path_to_load = dialog.pick_file();
                        ui.close();
                    }
                    ui.separator();
                    if recent_files.is_empty() {
                        ui.add_enabled(false, egui::Button::new("履歴はありません"));
                    } else {
                        for path in recent_files {
                            let is_current = current_path
                                .as_ref()
                                .map(|current| recent_file_key(current) == recent_file_key(&path))
                                .unwrap_or(false);
                            let button =
                                egui::Button::new(history_menu_label(&path)).fill(if is_current {
                                    Color32::from_rgb(36, 112, 150)
                                } else {
                                    Color32::from_rgba_unmultiplied(70, 70, 70, 190)
                                });
                            if ui
                                .add_sized(egui::vec2(460.0, 24.0), button)
                                .lab_hover_tip(path.display().to_string())
                                .clicked()
                            {
                                path_to_load = Some(path);
                                ui.close();
                            }
                        }
                    }
                });
            });
        });

        if let Some(path) = path_to_load {
            self.load_path(ctx, &path);
        }
    }

    fn mark_dirty(&mut self) {
        self.cancel_pending_render();
        self.generation = self.generation.wrapping_add(1);
        self.result_dirty = true;
        self.mask_dirty = true;
        self.mask_dirty_tiles = None;
        self.last_edit = Instant::now();
    }

    fn reveal_mask_preview(&mut self) {
        self.show_mask = true;
    }

    fn hide_mask_preview(&mut self) {
        self.show_mask = false;
    }

    fn mark_mask_changed(&mut self) {
        self.reveal_mask_preview();
        self.mark_dirty();
    }

    fn mark_mask_tiles_changed(&mut self, new_tiles: BTreeSet<(usize, usize)>) {
        self.reveal_mask_preview();
        self.mark_dirty_tiles(new_tiles);
    }

    fn mark_dirty_tiles(&mut self, new_tiles: BTreeSet<(usize, usize)>) {
        self.cancel_pending_render();
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

    fn cancel_pending_render(&mut self) {
        if let Some(pending) = self.pending.take() {
            pending.cancel.store(true, Ordering::Relaxed);
        }
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
            self.subject_cutout_edit_active = false;
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
            self.subject_cutout_edit_active = false;
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
            self.mark_mask_changed();
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

    fn crop_rect_or_full(&self) -> Option<CropRect> {
        let (w, h) = self.image_dims()?;
        Some(
            self.crop_rect
                .unwrap_or_else(|| CropRect::full(w, h))
                .sanitized(w, h),
        )
    }

    fn crop_is_active(&self) -> bool {
        let Some((w, h)) = self.image_dims() else {
            return false;
        };
        let crop = self
            .crop_rect
            .unwrap_or_else(|| CropRect::full(w, h))
            .sanitized(w, h);
        !crop.is_full(w, h)
    }

    fn effective_crop_rect(&self) -> Option<CropRect> {
        if !self.crop_is_active() {
            return None;
        }
        self.crop_rect_or_full()
    }

    fn current_crop_aspect_ratio(&self) -> Option<f32> {
        let crop = self.crop_rect_or_full()?;
        Some(crop.width() / crop.height().max(1.0))
    }

    fn crop_resize_aspect_ratio(&self) -> Option<f32> {
        self.crop_aspect_mode
            .aspect_ratio()
            .or_else(|| match self.crop_aspect_mode {
                CropAspectMode::Keep => self.current_crop_aspect_ratio(),
                _ => None,
            })
    }

    /// Read the current crop gesture out of the two legacy `crop_drag` / `crop_create_drag`
    /// fields as a single value for [`crop_gesture_step`].
    fn crop_gesture(&self) -> CropGesture {
        if let Some(drag) = self.crop_drag {
            CropGesture::Resize(drag)
        } else if let Some(create) = self.crop_create_drag {
            CropGesture::Create(create)
        } else {
            CropGesture::Idle
        }
    }

    fn set_crop_gesture(&mut self, gesture: CropGesture) {
        match gesture {
            CropGesture::Idle => {
                self.crop_drag = None;
                self.crop_create_drag = None;
            }
            CropGesture::Resize(drag) => {
                self.crop_drag = Some(drag);
                self.crop_create_drag = None;
            }
            CropGesture::Create(create) => {
                self.crop_drag = None;
                self.crop_create_drag = Some(create);
            }
        }
    }

    fn reset_crop(&mut self) {
        let Some((w, h)) = self.image_dims() else {
            return;
        };
        self.crop_rect = Some(CropRect::full(w, h));
        self.crop_enabled = false;
        self.crop_drag = None;
        self.crop_create_drag = None;
    }

    fn apply_crop_aspect_mode_to_rect(&mut self) {
        let Some((w, h)) = self.image_dims() else {
            return;
        };
        let Some(ratio) = self.crop_aspect_mode.aspect_ratio() else {
            return;
        };
        let crop = self
            .crop_rect_or_full()
            .unwrap_or_else(|| CropRect::full(w, h))
            .fit_to_aspect_around_center(ratio, w, h);
        self.crop_enabled = !crop.is_full(w, h);
        self.crop_rect = Some(crop);
        self.crop_drag = None;
        self.crop_create_drag = None;
    }

    fn set_workflow_panel(&mut self, panel: LabWorkflowPanel) {
        if self.workflow_panel == panel {
            return;
        }
        self.workflow_panel = panel;
        self.crop_edit_mode = panel == LabWorkflowPanel::Crop;
        self.crop_drag = None;
        self.crop_create_drag = None;
        self.pan_drag_start = None;
        self.prev_paint_pos = None;
        self.last_paint_pos = None;
        if panel != LabWorkflowPanel::Adjust {
            self.shape_drag = None;
            self.radial_gradient_drag_active = false;
            self.tilt_shift_drag_active = false;
        }
        if panel == LabWorkflowPanel::Crop {
            self.ensure_crop_rect();
        }
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
        let layer = layer_with_mask(format!("補正 {}", self.layers.len() + 1), mask_kind, w, h);
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
        self.status = format!("補正レイヤーを追加しました: {}", mask_kind.label());
        self.mark_mask_changed();
    }

    fn add_layer_with_mask_and_auto_generate(&mut self, mask_kind: MaskKind, ctx: &egui::Context) {
        if mask_kind == MaskKind::Subject && !subject_model_available() {
            self.status =
                "被写体選択には U²-Netp モデルが必要です。保存済み被写体マスクは利用できます。"
                    .to_string();
            return;
        }
        self.add_layer_with_mask(mask_kind);
        match mask_kind {
            MaskKind::Subject => self.start_subject_segmentation(ctx),
            MaskKind::Segmentation => {
                self.start_region_segmentation(ctx, RegionSegmentationScope::Full);
            }
            _ => {}
        }
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
        self.mark_mask_changed();
    }

    fn clear_selected_manual_override_target(&mut self, target: OverrideEditTarget) {
        let Some(layer) = self.selected_layer_ref() else {
            return;
        };
        let has_target = match target {
            OverrideEditTarget::Add => layer.manual_override.add.is_some(),
            OverrideEditTarget::Subtract => layer.manual_override.subtract.is_some(),
        };
        if !has_target {
            return;
        }
        self.push_undo_snapshot();
        if let Some(layer) = self.selected_layer_mut() {
            match target {
                OverrideEditTarget::Add => layer.manual_override.add = None,
                OverrideEditTarget::Subtract => layer.manual_override.subtract = None,
            }
        }
        self.selected_shape = None;
        self.status = format!("{}を全消去しました。", target.label());
        self.mark_mask_changed();
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
        let (messages, disconnected) = {
            let Some(pending) = self.pending.as_ref() else {
                return;
            };
            let mut messages = Vec::new();
            let mut disconnected = false;
            loop {
                match pending.rx.try_recv() {
                    Ok(message) => messages.push(message),
                    Err(mpsc::TryRecvError::Empty) => break,
                    Err(mpsc::TryRecvError::Disconnected) => {
                        disconnected = true;
                        break;
                    }
                }
            }
            (messages, disconnected)
        };

        for message in messages {
            match message {
                RenderWorkerMessage::Progress(progress) => {
                    if progress.generation == self.generation {
                        self.status = format!(
                            "再合成中: {} {:.0}% ({}/{})",
                            progress.effect_name,
                            (progress.percent.clamp(0.0, 1.0) * 100.0).round(),
                            progress.layer_index + 1,
                            progress.layer_count.max(1)
                        );
                        ctx.request_repaint();
                    }
                }
                RenderWorkerMessage::Done(result) => {
                    let Some(pending) = self.pending.take() else {
                        continue;
                    };
                    let generation = pending.generation;
                    let render_ms = pending.started_at.elapsed().as_secs_f64() * 1000.0;
                    self.perf_stats.render_jobs += 1;
                    self.perf_stats.render_ms_total += render_ms;
                    self.perf_stats.render_ms_max = self.perf_stats.render_ms_max.max(render_ms);
                    match result {
                        Ok(result) => {
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
                        Err(e) if e == "cancelled" => {
                            self.status = "再合成をキャンセルしました。".to_string();
                            if self.result_dirty {
                                ctx.request_repaint();
                            }
                        }
                        Err(e) => {
                            self.status = format!("再合成失敗: {e}");
                        }
                    }
                }
            }
        }
        if disconnected && self.pending.is_some() {
            self.pending = None;
            self.status = "再合成 worker が停止しました。".to_string();
        }
        if self.pending.is_some() {
            ctx.request_repaint();
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
                            layer.mask = LocalMask::Subject(SubjectMask::from_raster(mask));
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
                self.mark_mask_changed();
                ctx.request_repaint();
            }
            Ok(Err(e)) => {
                self.segmentation_pending = None;
                self.status = format!("マスク生成失敗: {e}");
            }
            Err(mpsc::TryRecvError::Empty) => {}
            Err(mpsc::TryRecvError::Disconnected) => {
                self.segmentation_pending = None;
                self.status = "セグメンテーション worker が停止しました。".to_string();
            }
        }
    }

    fn choose_cube_lut_for_selected_layer(&mut self) {
        let layer_idx = self.selected_layer;
        if !matches!(
            self.layers.get(layer_idx).map(|layer| &layer.effect),
            Some(LocalEffect::CubeLut(_))
        ) {
            self.status = "3D LUT レイヤーを選択してください。".to_string();
            return;
        }
        let Some(path) = rfd::FileDialog::new()
            .add_filter("3D LUT (.cube)", &["cube"])
            .set_title("3D LUTを選択")
            .pick_file()
        else {
            return;
        };
        self.start_cube_lut_load(layer_idx, path);
    }

    fn start_cube_lut_load(&mut self, layer_idx: usize, path: PathBuf) {
        if self.lut_load_pending.is_some() {
            self.status = "LUT読み込み中です。".to_string();
            return;
        }
        let fallback_name = path
            .file_stem()
            .and_then(|name| name.to_str())
            .unwrap_or("3D LUT")
            .to_string();
        let worker_path = path.clone();
        let (tx, rx) = mpsc::channel();
        let spawn_result = std::thread::Builder::new()
            .name("local-adjust-lab-lut-load".to_string())
            .spawn(move || {
                let result = std::fs::read_to_string(&worker_path)
                    .map_err(|e| format!("LUTファイルを読めません: {e}"))
                    .and_then(|text| parse_cube_lut(&text, &fallback_name));
                let _ = tx.send(result);
            });
        match spawn_result {
            Ok(_) => {
                self.lut_load_pending = Some(LutLoadPending {
                    layer_idx,
                    generation: self.generation,
                    rx,
                    started_at: Instant::now(),
                    path: path.clone(),
                });
                self.status = format!("LUT読み込み中: {}", path.display());
            }
            Err(e) => {
                self.status = format!("LUT読み込み worker 起動失敗: {e}");
            }
        }
    }

    fn poll_lut_load(&mut self, ctx: &egui::Context) {
        let recv_result = {
            let Some(pending) = self.lut_load_pending.as_ref() else {
                return;
            };
            pending.rx.try_recv()
        };
        match recv_result {
            Ok(Ok(params)) => {
                let Some(pending) = self.lut_load_pending.take() else {
                    return;
                };
                if self.generation != pending.generation {
                    self.status =
                        "LUT読み込み結果を破棄しました。画像またはレイヤーが変更されています。"
                            .to_string();
                    return;
                }
                if !matches!(
                    self.layers
                        .get(pending.layer_idx)
                        .map(|layer| &layer.effect),
                    Some(LocalEffect::CubeLut(_))
                ) {
                    self.status = "LUT読み込み結果を破棄しました。対象レイヤーが変更されています。"
                        .to_string();
                    return;
                }
                let elapsed_ms = pending.started_at.elapsed().as_secs_f64() * 1000.0;
                let name = params.name.clone();
                let size = params.size;
                self.push_undo_snapshot();
                if let Some(layer) = self.layers.get_mut(pending.layer_idx) {
                    layer.effect = LocalEffect::CubeLut(params);
                }
                self.selected_layer = pending.layer_idx;
                self.status = format!(
                    "LUT読み込み完了: {name} ({size}^3, {elapsed_ms:.0}ms) / {}",
                    pending.path.display()
                );
                self.mark_dirty();
                ctx.request_repaint();
            }
            Ok(Err(e)) => {
                self.lut_load_pending = None;
                self.status = format!("LUT読み込み失敗: {e}");
            }
            Err(mpsc::TryRecvError::Empty) => {}
            Err(mpsc::TryRecvError::Disconnected) => {
                self.lut_load_pending = None;
                self.status = "LUT読み込み worker が停止しました。".to_string();
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

    fn start_region_segmentation(&mut self, ctx: &egui::Context, scope: RegionSegmentationScope) {
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
        let subject = if scope.requires_subject() {
            self.subject_mask_candidate()
        } else {
            None
        };
        if scope.requires_subject() && subject.is_none() {
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
                    scope,
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
                self.status = scope.pending_label().to_string();
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
                Some(mask.current_raster_mask())
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
        let cancel = Arc::new(AtomicBool::new(false));
        let worker_cancel = Arc::clone(&cancel);
        let (tx, rx) = mpsc::channel();
        let spawn_result = std::thread::Builder::new()
            .name("local-adjust-lab-render".to_string())
            .spawn(move || {
                let progress_tx = tx.clone();
                let mut last_layer = usize::MAX;
                let mut last_percent = -1.0_f32;
                let result = apply_layers_with_progress(
                    source.as_ref(),
                    &layers,
                    Some(worker_cancel.as_ref()),
                    |progress| {
                        if worker_cancel.load(Ordering::Relaxed) {
                            return;
                        }
                        let rounded_percent = (progress.percent.clamp(0.0, 1.0) * 100.0).round();
                        let should_send = progress.layer_index != last_layer
                            || rounded_percent - last_percent >= 5.0
                            || rounded_percent >= 100.0;
                        if should_send {
                            last_layer = progress.layer_index;
                            last_percent = rounded_percent;
                            let _ =
                                progress_tx.send(RenderWorkerMessage::Progress(RenderProgress {
                                    generation,
                                    layer_index: progress.layer_index,
                                    layer_count: progress.layer_count,
                                    effect_name: progress.effect_name.to_string(),
                                    percent: progress.percent,
                                }));
                        }
                    },
                )
                .map_err(|e| e.to_string());
                let _ = tx.send(RenderWorkerMessage::Done(result));
            });
        match spawn_result {
            Ok(_) => {
                self.pending = Some(RenderPending {
                    generation,
                    rx,
                    cancel,
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
        let preview_edit_target = self.override_edit_panel;
        let use_fast_tile_eval = preview_edit_target.is_none()
            && can_build_mask_tiles_from_layer(&self.layers[self.selected_layer], width, height);
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
        let mask_colors = self.mask_color_preset.colors();
        let mask = if use_fast_tile_eval {
            None
        } else {
            let preview_layer =
                layer_for_mask_preview(&self.layers[self.selected_layer], preview_edit_target);
            match evaluate_layer_mask(image.source.as_ref(), &preview_layer) {
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
                        preview_edit_target,
                        mask_colors,
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
                        mask_colors,
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
                    format!("切り取りして保存しました: {}", path.display())
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
            crop_enabled: self.crop_is_active(),
            crop_overlay: true,
            crop_aspect_mode: self.crop_aspect_mode,
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
        self.crop_overlay = true;
        self.crop_aspect_mode = sidecar.crop_aspect_mode;
        self.crop_rect = sidecar
            .crop_rect
            .or_else(|| sidecar.crop_enabled.then(|| CropRect::full(w, h)))
            .map(|crop| crop.sanitized(w, h));
        self.crop_drag = None;
        self.crop_create_drag = None;
        self.crop_edit_mode = self.workflow_panel == LabWorkflowPanel::Crop;
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
            self.mark_mask_changed();
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
        self.mark_mask_changed();
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
        self.mark_mask_changed();
    }

    fn pick_color(&mut self, p: Pos2) {
        let Some((rgb, _, _)) = self.source_rgb_at_image_pos(p) else {
            return;
        };
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
        self.mark_mask_changed();
    }

    fn pick_selective_color_target(&mut self, p: Pos2) {
        let Some((rgb, x, y)) = self.source_rgb_at_image_pos(p) else {
            return;
        };
        let hue = hue_degrees_from_rgb(rgb);
        let Some(layer) = self.selected_layer_mut() else {
            self.selective_color_pick_active = false;
            return;
        };
        let LocalEffect::SelectiveColor(params) = &mut layer.effect else {
            self.selective_color_pick_active = false;
            return;
        };
        params.target_hue_degrees = hue;
        self.selective_color_pick_active = false;
        self.status = format!(
            "セレクティブカラー対象色: #{:02X}{:02X}{:02X} ({hue:.0}°, x:{x}, y:{y})",
            rgb[0], rgb[1], rgb[2]
        );
        self.mark_dirty();
    }

    fn pick_effect_rgb_target(&mut self, p: Pos2, target: RgbPickTarget) {
        let Some((rgb, x, y)) = self.source_rgb_at_image_pos(p) else {
            return;
        };
        let label = target.label();
        let Some(layer) = self.selected_layer_mut() else {
            self.rgb_pick_active = None;
            return;
        };
        let picked = set_rgb_pick_target(&mut layer.effect, target, rgb);
        self.rgb_pick_active = None;
        if picked {
            self.status = format!(
                "{label}: #{:02X}{:02X}{:02X} (x:{x}, y:{y})",
                rgb[0], rgb[1], rgb[2]
            );
            self.mark_dirty();
        } else {
            self.status = "スポイト対象の効果が切り替わったため解除しました。".to_string();
        }
    }

    fn source_rgb_at_image_pos(&self, p: Pos2) -> Option<([u8; 3], usize, usize)> {
        let image = self.image.as_ref()?;
        let x = p.x.round().clamp(0.0, image.source.width as f32 - 1.0) as usize;
        let y = p.y.round().clamp(0.0, image.source.height as f32 - 1.0) as usize;
        let i = (y * image.source.width + x) * 4;
        Some((
            [
                image.source.pixels[i],
                image.source.pixels[i + 1],
                image.source.pixels[i + 2],
            ],
            x,
            y,
        ))
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
                self.mark_mask_changed();
            }
        }
    }

    fn draw_controls(&mut self, ui: &mut egui::Ui) {
        apply_lab_dark_ui(ui);

        let btn_w = ((PANEL_W - 20.0 - 4.0) / 2.0).max(96.0);
        let btn_size = egui::vec2(btn_w, 24.0);
        self.draw_workflow_selector(ui);

        ui.separator();
        match self.workflow_panel {
            LabWorkflowPanel::Eraser => self.draw_placeholder_stage_panel(
                ui,
                "消しゴム",
                "本体統合時に既存の消しゴム処理を接続します。",
            ),
            LabWorkflowPanel::Adjust => self.draw_adjust_layer_controls(ui, btn_size),
            LabWorkflowPanel::Conceal => self.draw_placeholder_stage_panel(
                ui,
                "隠蔽加工",
                "本体統合時に既存の隠蔽加工処理を接続します。",
            ),
            LabWorkflowPanel::Crop => self.draw_crop_controls(ui),
            LabWorkflowPanel::Save => self.draw_save_controls(ui, btn_size),
        }
    }

    fn draw_display_controls(&mut self, ui: &mut egui::Ui, btn_size: egui::Vec2) {
        ui.horizontal(|ui| {
            ui.label(egui::RichText::new("表示:").color(Color32::from_gray(200)));
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                for preset in MaskColorPreset::ALL.into_iter().rev() {
                    let selected = self.mask_color_preset == preset;
                    let colors = preset.colors();
                    let button = egui::Button::new(
                        egui::RichText::new(preset.label())
                            .strong()
                            .size(10.0)
                            .color(Color32::WHITE),
                    )
                    .fill(colors.base(if selected { 145 } else { 80 }))
                    .stroke(if selected {
                        egui::Stroke::new(1.5, colors.edit(255))
                    } else {
                        egui::Stroke::new(1.0, Color32::from_rgba_unmultiplied(255, 255, 255, 35))
                    });
                    if ui
                        .add_sized(egui::vec2(24.0, 18.0), button)
                        .lab_hover_tip(format!("マスクカラー: {}", preset.description()))
                        .clicked()
                    {
                        self.mask_color_preset = preset;
                        self.reveal_mask_preview();
                        self.mask_dirty = true;
                        self.mask_dirty_tiles = None;
                    }
                }
            });
        });
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
    }

    fn draw_workflow_selector(&mut self, ui: &mut egui::Ui) {
        ui.label(egui::RichText::new("処理順:").color(Color32::from_gray(200)));
        let button_w = ((PANEL_W - 28.0) / 5.0).max(52.0);
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 4.0;
            for panel in LabWorkflowPanel::ALL {
                if panel_toggle_button(
                    ui,
                    panel.label(),
                    self.workflow_panel == panel,
                    Some(egui::vec2(button_w, 24.0)),
                    false,
                )
                .clicked()
                {
                    self.set_workflow_panel(panel);
                }
            }
        });
    }

    fn draw_placeholder_stage_panel(&mut self, ui: &mut egui::Ui, title: &str, body: &str) {
        ui.label(
            egui::RichText::new(title)
                .size(14.0)
                .strong()
                .color(Color32::WHITE),
        );
        ui.label(
            egui::RichText::new(body)
                .size(11.0)
                .color(Color32::from_gray(170)),
        );
    }

    fn draw_adjust_layer_controls(&mut self, ui: &mut egui::Ui, btn_size: egui::Vec2) {
        self.draw_display_controls(ui, btn_size);
        ui.separator();
        self.draw_layer_list(ui, PANEL_W);
        if self.layers.is_empty() {
            return;
        }

        ui.separator();
        self.draw_effect_selector(ui);

        ui.separator();
        self.draw_manual_tool_selector(ui, btn_size);
    }

    fn draw_effect_selector(&mut self, ui: &mut egui::Ui) {
        ui.label(egui::RichText::new("加工内容:").color(Color32::from_gray(200)));
        let label = self
            .layers
            .get(self.selected_layer)
            .map(|layer| effect_summary(&layer.effect))
            .unwrap_or_else(|| "効果なし".to_string());
        ui.horizontal(|ui| {
            ui.add_sized(
                egui::vec2((PANEL_W - 82.0).max(160.0), 24.0),
                egui::Label::new(
                    egui::RichText::new(label)
                        .size(12.0)
                        .color(Color32::from_gray(230)),
                ),
            );
            if ui
                .add_sized(egui::vec2(74.0, 24.0), egui::Button::new("効果選択"))
                .lab_hover_tip("効果をグループ別の一覧から選びます。")
                .clicked()
            {
                self.effect_picker_dialog_open = true;
            }
        });
    }

    fn draw_save_controls(&mut self, ui: &mut egui::Ui, btn_size: egui::Vec2) {
        let btn_w = btn_size.x;
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
                .lab_hover_tip("画像ファイル名に .miv を付けたサイドカーファイルへ保存します。")
                .clicked()
            {
                self.save_settings_sidecar();
            }
            if ui
                .add_sized(btn_size, egui::Button::new("設定読込"))
                .lab_hover_tip("画像横の .miv サイドカーファイルからレイヤー設定を読み込みます。")
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
                     Ctrl:元画像を一時表示  Alt:マスク表示を一時反転  Shift:ルーペ\n\
                     境界筆[A]:境界で止めながら近い色を塗る  Ctrl中は境界表示+通常筆\n\
                     隙間補完[G]:細い未塗り部分を補完\n\
                     切り取り:黄色枠/ハンドルをドラッグ、保存時に最後段で切り出し\n\
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

    fn draw_crop_controls(&mut self, ui: &mut egui::Ui) {
        let Some((w, h)) = self.image_dims() else {
            return;
        };
        let btn_w = ((PANEL_W - 20.0 - 4.0) / 2.0).max(96.0);
        ui.label(
            egui::RichText::new("切り取り")
                .size(14.0)
                .strong()
                .color(Color32::WHITE),
        );
        ui.horizontal(|ui| {
            let active = self.crop_is_active();
            ui.label(
                egui::RichText::new(if active {
                    "切り取りあり"
                } else {
                    "切り取りなし"
                })
                .color(if active {
                    Color32::from_rgb(120, 220, 150)
                } else {
                    Color32::from_gray(170)
                }),
            );
        });

        if ui
            .add_sized(
                egui::vec2(btn_w * 2.0 + 4.0, 24.0),
                egui::Button::new("リセット"),
            )
            .clicked()
        {
            self.reset_crop();
        }

        let selected_text = self.crop_aspect_mode.label();
        let mut next_mode = self.crop_aspect_mode;
        ui.horizontal(|ui| {
            ui.label(egui::RichText::new("比率:").color(Color32::from_gray(200)));
            lab_combo_box(ui, "crop_aspect_mode", selected_text, |ui| {
                for mode in CropAspectMode::ALL {
                    ui.selectable_value(&mut next_mode, mode, mode.label());
                }
            });
        });
        if next_mode != self.crop_aspect_mode {
            self.crop_aspect_mode = next_mode;
            self.apply_crop_aspect_mode_to_rect();
        }

        let mut crop = self
            .ensure_crop_rect()
            .unwrap_or_else(|| CropRect::full(w, h));
        let mut x = crop.min_x.round() as i32;
        let mut y = crop.min_y.round() as i32;
        let mut cw = crop.width().round() as i32;
        let mut ch = crop.height().round() as i32;
        let mut x_changed = false;
        let mut y_changed = false;
        let mut w_changed = false;
        let mut h_changed = false;
        ui.horizontal(|ui| {
            x_changed |= ui
                .add(egui::DragValue::new(&mut x).range(0..=w.saturating_sub(1) as i32))
                .changed();
            ui.label("X");
            y_changed |= ui
                .add(egui::DragValue::new(&mut y).range(0..=h.saturating_sub(1) as i32))
                .changed();
            ui.label("Y");
        });
        ui.horizontal(|ui| {
            w_changed |= ui
                .add(egui::DragValue::new(&mut cw).range(1..=w.max(1) as i32))
                .changed();
            ui.label("W");
            h_changed |= ui
                .add(egui::DragValue::new(&mut ch).range(1..=h.max(1) as i32))
                .changed();
            ui.label("H");
        });
        if x_changed || y_changed || w_changed || h_changed {
            crop = crop_from_xywh_inputs(
                x,
                y,
                cw,
                ch,
                w,
                h,
                self.crop_resize_aspect_ratio(),
                h_changed && !w_changed,
            );
            self.crop_rect = Some(crop);
            self.crop_enabled = !crop.is_full(w, h);
            self.crop_drag = None;
        }
        ui.label(
            egui::RichText::new("保存時に最終結果を切り出します。上流のマスク座標は変わりません。")
                .size(10.0)
                .color(Color32::from_gray(170)),
        );
    }

    fn draw_manual_tool_selector(&mut self, ui: &mut egui::Ui, btn_size: egui::Vec2) {
        let mask_kind = self
            .selected_layer_ref()
            .map(|layer| MaskKind::from_mask(&layer.mask))
            .unwrap_or(MaskKind::Raster);
        if mask_kind == MaskKind::Full && self.override_edit_panel == Some(OverrideEditTarget::Add)
        {
            self.reset_override_edit_state_for_selected_layer();
        }
        let editing_base_manual = mask_kind == MaskKind::Raster;
        let editing_full_mask = mask_kind == MaskKind::Full;
        ui.label(
            egui::RichText::new(if editing_base_manual {
                "手動マスク:"
            } else if editing_full_mask {
                "削除マスク:"
            } else {
                "追加/削除マスク:"
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
            if !editing_full_mask {
                let add_label = if has_add {
                    "追加マスクあり"
                } else {
                    "追加マスク"
                };
                if panel_toggle_button(
                    ui,
                    add_label,
                    self.override_edit_panel == Some(OverrideEditTarget::Add),
                    Some(btn_size),
                    true,
                )
                .lab_hover_tip("ベースマスクに手動で足す2値マスクを編集します。")
                .clicked()
                {
                    self.toggle_override_edit_panel(OverrideEditTarget::Add);
                }
            }
            let subtract_label = if has_subtract {
                "削除マスクあり"
            } else {
                "削除マスク"
            };
            if panel_toggle_button(
                ui,
                subtract_label,
                self.override_edit_panel == Some(OverrideEditTarget::Subtract),
                Some(btn_size),
                false,
            )
            .lab_hover_tip("ベースマスクから手動で除外する2値マスクを編集します。")
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
                    ui.separator();
                    let has_target = self
                        .selected_layer_ref()
                        .map(|layer| match target {
                            OverrideEditTarget::Add => layer.manual_override.add.is_some(),
                            OverrideEditTarget::Subtract => {
                                layer.manual_override.subtract.is_some()
                            }
                        })
                        .unwrap_or(false);
                    let clear_label = format!("{}を全消去", target.label());
                    if ui
                        .add_enabled(
                            has_target,
                            egui::Button::new(clear_label).fill(Color32::from_rgb(95, 45, 45)),
                        )
                        .lab_hover_tip("現在開いている追加/削除マスクだけを空にします。ベースマスクは残ります。")
                        .clicked()
                    {
                        self.clear_selected_manual_override_target(target);
                    }
                });
        } else {
            ui.horizontal(|ui| {
                ui.add_space(2.0);
                let help = if editing_full_mask {
                    "全体マスクでは削除マスクだけを開いて除外範囲を描きます。"
                } else {
                    "必要なときだけ追加マスク/削除マスクを開いて手描きします。"
                };
                ui.label(
                    egui::RichText::new(help)
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
        let adjust_panel_active = self.workflow_panel == LabWorkflowPanel::Adjust;
        let crop_panel_active = self.workflow_panel == LabWorkflowPanel::Crop;
        let (ctrl_down, alt_down, shift_down) =
            ui.input(|i| (i.modifiers.ctrl, i.modifiers.alt, i.modifiers.shift));
        let source_preview_active = adjust_panel_active && (self.show_source || ctrl_down);
        let mask_preview_active =
            mask_preview_active(adjust_panel_active, self.show_mask, alt_down);
        let active_texture_id = if source_preview_active {
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
        let dialog_open = self.add_layer_dialog_open || self.effect_picker_dialog_open;
        let view_input_used = if dialog_open {
            self.pan_drag_start = None;
            false
        } else {
            self.handle_view_navigation(ui, canvas_rect, rect, img_w, img_h, panel_blocks_pointer)
        };

        let uv = Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0));
        ui.painter()
            .image(active_texture_id, rect, uv, Color32::WHITE);
        if mask_preview_active {
            self.draw_mask_tile_preview(ui, rect);
        }
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

        if adjust_panel_active && !dialog_open {
            self.draw_shape_overlay(ui, rect, pointer_screen, !pan_mode && !panel_blocks_pointer);
        }
        if adjust_panel_active
            && !pan_mode
            && !panel_blocks_pointer
            && !dialog_open
            && !self.selective_color_pick_active
            && self.rgb_pick_active.is_none()
        {
            self.draw_brush_cursor(ui, rect, pointer_screen);
        }
        if (self.selective_color_pick_active || self.rgb_pick_active.is_some())
            && !panel_blocks_pointer
            && !dialog_open
            && pointer_screen.map(|p| rect.contains(p)).unwrap_or(false)
        {
            ui.ctx().set_cursor_icon(egui::CursorIcon::Crosshair);
        }
        let effect_gradient_active = self.selected_effect_gradient_shape().is_some();
        let effect_gradient_handle_used =
            if pan_mode || !adjust_panel_active || dialog_open || !effect_gradient_active {
                false
            } else {
                self.draw_effect_gradient_handles(ui, rect)
            };
        let effect_position_handle_used = if pan_mode
            || !adjust_panel_active
            || dialog_open
            || !self.effect_position_handles_visible
            || self.selective_color_pick_active
            || self.rgb_pick_active.is_some()
        {
            false
        } else {
            self.draw_effect_position_handles(ui, rect)
        };
        let gradient_handle_used =
            if pan_mode || !adjust_panel_active || dialog_open || effect_gradient_active {
                false
            } else {
                self.draw_gradient_handles(ui, rect)
            };
        let tilt_shift_handle_used = if pan_mode || !adjust_panel_active || dialog_open {
            false
        } else {
            self.draw_tilt_shift_handles(ui, rect)
        };
        let crop_used = self.draw_crop_overlay(
            ui,
            rect,
            img_w,
            img_h,
            pointer_screen,
            crop_panel_active && !panel_blocks_pointer && !pan_mode && !dialog_open,
        );
        if shift_down && !panel_blocks_pointer && !dialog_open {
            self.draw_loupe(ui, canvas_rect, rect, active_texture_id, pointer_screen);
        }
        let secondary_pressed = ui.input(|i| i.pointer.secondary_pressed());

        if adjust_panel_active
            && !dialog_open
            && !view_input_used
            && !panel_blocks_pointer
            && !crop_used
            && (response.hovered() || response.dragged() || response.clicked() || secondary_pressed)
        {
            let pointer = ui.input(|i| i.pointer.interact_pos());
            if let Some(pointer_screen) = pointer {
                let pos = screen_to_image(rect, img_w, img_h, pointer_screen);
                if !effect_gradient_handle_used
                    && !effect_position_handle_used
                    && !gradient_handle_used
                    && !tilt_shift_handle_used
                {
                    if let Some(pos) = pos {
                        if self.selective_color_pick_active {
                            if response.clicked() || ui.input(|i| i.pointer.primary_pressed()) {
                                self.pick_selective_color_target(pos);
                            }
                            return;
                        }
                        if let Some(target) = self.rgb_pick_active {
                            if response.clicked() || ui.input(|i| i.pointer.primary_pressed()) {
                                self.pick_effect_rgb_target(pos, target);
                            }
                            return;
                        }
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

    fn draw_loupe(
        &self,
        ui: &mut egui::Ui,
        canvas_rect: Rect,
        image_rect: Rect,
        texture_id: egui::TextureId,
        pointer_screen: Option<Pos2>,
    ) {
        let Some(pointer) = pointer_screen else {
            return;
        };
        if !image_rect.contains(pointer) || image_rect.width() <= 1.0 || image_rect.height() <= 1.0
        {
            return;
        }

        let margin = 14.0;
        let size = 180.0_f32
            .min((canvas_rect.width() - margin * 2.0).max(72.0))
            .min((canvas_rect.height() - margin * 2.0).max(72.0));
        let zoom = 3.0;
        let offset = 18.0;
        let mut min = pointer + egui::vec2(offset, offset);
        if min.x + size + margin > canvas_rect.right() {
            min.x = pointer.x - size - offset;
        }
        if min.y + size + margin > canvas_rect.bottom() {
            min.y = pointer.y - size - offset;
        }
        let min_x = canvas_rect.left() + margin;
        let max_x = (canvas_rect.right() - size - margin).max(min_x);
        let min_y = canvas_rect.top() + margin;
        let max_y = (canvas_rect.bottom() - size - margin).max(min_y);
        min.x = min.x.clamp(min_x, max_x);
        min.y = min.y.clamp(min_y, max_y);
        let dst = Rect::from_min_size(min, egui::vec2(size, size));

        let center_u = ((pointer.x - image_rect.left()) / image_rect.width()).clamp(0.0, 1.0);
        let center_v = ((pointer.y - image_rect.top()) / image_rect.height()).clamp(0.0, 1.0);
        let half_u = ((size / zoom) / image_rect.width() * 0.5).clamp(0.001, 0.5);
        let half_v = ((size / zoom) / image_rect.height() * 0.5).clamp(0.001, 0.5);
        let uv = Rect::from_min_max(
            Pos2::new(
                (center_u - half_u).clamp(0.0, 1.0),
                (center_v - half_v).clamp(0.0, 1.0),
            ),
            Pos2::new(
                (center_u + half_u).clamp(0.0, 1.0),
                (center_v + half_v).clamp(0.0, 1.0),
            ),
        );

        let painter = ui.painter();
        painter.rect_filled(
            dst.expand(6.0),
            8.0,
            Color32::from_rgba_unmultiplied(8, 8, 10, 225),
        );
        painter.image(texture_id, dst, uv, Color32::WHITE);
        painter.rect_stroke(
            dst,
            6.0,
            egui::Stroke::new(1.0, Color32::from_rgba_unmultiplied(255, 255, 255, 150)),
            egui::StrokeKind::Inside,
        );
        let center = dst.center();
        painter.line_segment(
            [
                Pos2::new(center.x - 10.0, center.y),
                Pos2::new(center.x + 10.0, center.y),
            ],
            egui::Stroke::new(1.0, Color32::from_rgba_unmultiplied(255, 255, 255, 170)),
        );
        painter.line_segment(
            [
                Pos2::new(center.x, center.y - 10.0),
                Pos2::new(center.x, center.y + 10.0),
            ],
            egui::Stroke::new(1.0, Color32::from_rgba_unmultiplied(255, 255, 255, 170)),
        );
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
                                egui::RichText::new("補正レイヤー")
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
        if self.image.is_none() || self.workflow_panel != LabWorkflowPanel::Adjust {
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
        let action_row_w = btn_w * 2.0 + 4.0;
        ui.label(
            egui::RichText::new("レイヤー")
                .size(14.0)
                .strong()
                .color(Color32::WHITE),
        );
        if ui
            .add_sized(
                egui::vec2(btn_w * 2.0 + 4.0, 24.0),
                egui::Button::new("+ 補正レイヤー"),
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
        let mut layer_control_changed = false;
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
                    ui.set_min_height(56.0);
                    let layer = &mut self.layers[idx];
                    let mut row_clicked = false;
                    ui.horizontal(|ui| {
                        ui.spacing_mut().item_spacing.x = 8.0;
                        ui.vertical(|ui| {
                            ui.spacing_mut().item_spacing.y = 2.0;
                            ui.set_min_width(42.0);
                            if ui.checkbox(&mut layer.enabled, "").changed() {
                                layer_control_changed = true;
                            }
                            ui.horizontal(|ui| {
                                ui.spacing_mut().item_spacing.x = 2.0;
                                let before_response = draw_mask_application_button(
                                    ui,
                                    "前",
                                    layer.mask_before_effect,
                                );
                                let before_clicked = before_response.clicked();
                                before_response.lab_hover_tip(
                                    "ONで、マスク範囲だけを効果の入力素材にします。",
                                );
                                if before_clicked {
                                    layer.mask_before_effect = !layer.mask_before_effect;
                                    layer_control_changed = true;
                                }

                                let after_response =
                                    draw_mask_application_button(ui, "後", layer.mask_after_effect);
                                let after_clicked = after_response.clicked();
                                after_response.lab_hover_tip(
                                    "ONで、効果後の結果をマスク範囲で切り取ります。",
                                );
                                if after_clicked {
                                    layer.mask_after_effect = !layer.mask_after_effect;
                                    layer_control_changed = true;
                                }
                            });
                        });
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
                                ui.allocate_exact_size(egui::vec2(spacer_w, 56.0), Sense::click());
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
        if layer_control_changed {
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
            let gap = 4.0;
            ui.spacing_mut().item_spacing.x = gap;
            let unit_w = ((action_row_w - gap * 3.0) / 6.0).max(24.0);
            let small_btn = egui::vec2(unit_w, 22.0);
            let wide_btn = egui::vec2(unit_w * 2.0, 22.0);
            if ui.add_sized(small_btn, egui::Button::new("↑")).clicked() {
                self.move_selected_layer(-1);
            }
            if ui.add_sized(small_btn, egui::Button::new("↓")).clicked() {
                self.move_selected_layer(1);
            }
            if ui.add_sized(wide_btn, egui::Button::new("複製")).clicked() {
                self.duplicate_layer();
            }
            if ui
                .add_sized(
                    wide_btn,
                    egui::Button::new("削除").fill(Color32::from_rgb(120, 50, 50)),
                )
                .clicked()
            {
                self.remove_selected_layer();
            }
        });
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
            raster_vector_edit_controls_visible(selected_mask_kind, self.override_edit_panel);
        if manual_edit_controls_visible {
            self.draw_tool_controls(ui);
        } else {
            ui.label(
                egui::RichText::new(
                    "追加マスク/削除マスクを開くと、手描きツール設定を表示します。",
                )
                .size(11.0)
                .color(Color32::from_gray(180)),
            );
        }
        if selected_mask_kind != Some(MaskKind::Raster) && manual_edit_controls_visible {
            let help = if self.override_edit_panel.is_some() {
                "追加/削除マスクパネルが開いている間は、筆/図形ツールで追加マスクまたは削除マスクを編集します。ベースマスクを調整する場合はパネルを閉じます。"
            } else {
                match selected_mask_kind {
                    Some(MaskKind::LinearGradient) => {
                        "選択ツールでは画像上のドラッグで生成/調整します。筆などに切り替えると追加/削除マスクを描けます。"
                    }
                    Some(MaskKind::RadialGradient) => {
                        "選択ツールでは画像上のドラッグで生成/調整します。筆などに切り替えると追加/削除マスクを描けます。"
                    }
                    Some(MaskKind::ColorRange) => {
                        "選択ツールでは画像上クリックでスポイト指定します。筆などに切り替えると追加/削除マスクを描けます。"
                    }
                    Some(MaskKind::LumaRange) => {
                        "輝度範囲はスライダーで調整します。筆などで追加/削除マスクを描けます。"
                    }
                    Some(MaskKind::Full) => "全体マスクに対して削除マスクなどを描けます。",
                    Some(MaskKind::Subject) => {
                        "被写体/背景マットを保ったまま、筆などで追加/削除マスクを描けます。"
                    }
                    Some(MaskKind::Segmentation) => {
                        "選択ツールでは領域候補をクリック/ドラッグでON/OFFします。筆などでは追加/削除マスクを描けます。"
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
                .add(egui::Slider::new(&mut layer.opacity, 0.0..=1.0).text("不透明度"))
                .changed();
            ui.horizontal(|ui| {
                ui.label("マスク適用");
                let before_response =
                    draw_mask_application_button(ui, "前", layer.mask_before_effect);
                let before_clicked = before_response.clicked();
                before_response.lab_hover_tip("ONで、マスク範囲だけを効果の入力素材にします。");
                if before_clicked {
                    layer.mask_before_effect = !layer.mask_before_effect;
                    changed = true;
                }
                let after_response =
                    draw_mask_application_button(ui, "後", layer.mask_after_effect);
                let after_clicked = after_response.clicked();
                after_response.lab_hover_tip("ONで、効果後の結果をマスク範囲で切り取ります。");
                if after_clicked {
                    layer.mask_after_effect = !layer.mask_after_effect;
                    changed = true;
                }
            });
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
            self.mark_mask_changed();
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
        let dims = self.image_dims().unwrap_or((1, 1));
        let mut request_cube_lut_load = false;
        let mut request_selective_color_pick = false;
        let mut request_selective_color_pick_cancel = false;
        let mut request_rgb_pick = None;
        let mut request_rgb_pick_cancel = false;
        let mut request_effect_position_handles_visible = None;
        let selective_color_pick_active = self.selective_color_pick_active;
        let rgb_pick_active = self.rgb_pick_active;
        let effect_position_handles_visible = self.effect_position_handles_visible;
        if let Some(layer) = self.selected_layer_mut() {
            let response = draw_effect_params(
                ui,
                layer,
                dims,
                selective_color_pick_active,
                rgb_pick_active,
                effect_position_handles_visible,
            );
            request_cube_lut_load = response.load_cube_lut;
            request_selective_color_pick = response.start_selective_color_pick;
            request_selective_color_pick_cancel = response.cancel_selective_color_pick;
            request_rgb_pick = response.start_rgb_pick;
            request_rgb_pick_cancel = response.cancel_rgb_pick;
            request_effect_position_handles_visible = response.set_effect_position_handles_visible;
            if response.changed {
                self.hide_mask_preview();
                self.mark_dirty();
            }
        }
        if request_cube_lut_load {
            self.choose_cube_lut_for_selected_layer();
        }
        if request_selective_color_pick {
            self.selective_color_pick_active = true;
            self.rgb_pick_active = None;
            self.tool = MaskTool::Select;
            self.status =
                "画像上をクリックして、セレクティブカラーの対象色を取得します。".to_string();
        }
        if request_selective_color_pick_cancel {
            self.selective_color_pick_active = false;
            self.status = "セレクティブカラーのスポイトを解除しました。".to_string();
        }
        if let Some(target) = request_rgb_pick {
            self.rgb_pick_active = Some(target);
            self.selective_color_pick_active = false;
            self.tool = MaskTool::Select;
            self.status = format!("画像上をクリックして、{}を取得します。", target.label());
        }
        if request_rgb_pick_cancel {
            self.rgb_pick_active = None;
            self.status = "RGBスポイトを解除しました。".to_string();
        }
        if let Some(visible) = request_effect_position_handles_visible {
            self.effect_position_handles_visible = visible;
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
        let model_available = subject_model_available();
        let generated_mask_available = self
            .selected_layer_ref()
            .and_then(|layer| match &layer.mask {
                LocalMask::Subject(mask) => Some(subject_mask_has_content(mask)),
                _ => None,
            })
            .unwrap_or(false);
        if model_available {
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
        let generate_label = if pending {
            "被写体マスク生成中..."
        } else if generated_mask_available {
            "元画像から再生成"
        } else {
            "被写体マスク生成"
        };
        let generate_response = ui.add_enabled(
            !pending && model_available,
            egui::Button::new(generate_label),
        );
        let generate_tip = if model_available {
            if generated_mask_available {
                "保存済みまたは現在の被写体マスクを破棄し、元画像からモデルで再生成します。"
            } else {
                "元画像から被写体マスクを生成します。"
            }
        } else {
            "U²-Netp モデルがないため生成/再生成はできません。保存済みマスクの編集と適用は可能です。"
        };
        if generate_response.lab_hover_tip(generate_tip).clicked() {
            self.start_subject_segmentation(ui.ctx());
        }
        ui.horizontal(|ui| {
            if ui.button("被写体を選択").clicked()
                && let Some(layer) = self.selected_layer_mut()
            {
                layer.mask_inverted = false;
                self.mark_mask_changed();
            }
            if ui.button("背景を選択").clicked()
                && let Some(layer) = self.selected_layer_mut()
            {
                layer.mask_inverted = true;
                self.mark_mask_changed();
            }
        });
        if let Some((stats, mut refinement, refinement_enabled)) =
            self.selected_subject_cutout_state()
        {
            ui.separator();
            ui.label(
                egui::RichText::new("切り抜き向け整形")
                    .size(12.0)
                    .strong()
                    .color(Color32::from_gray(210)),
            );
            let mut restore_source = false;
            let mut apply_refinement = false;
            let mut push_undo = false;
            let mut refinement_controls_enabled = refinement_enabled;
            let enable_response = ui.checkbox(&mut refinement_controls_enabled, "マスクを整形");
            let enable_changed = enable_response.changed();
            enable_response.lab_hover_tip(
                "ONにすると、生成直後の元マットから切り抜き向けのマスクを再生成します。",
            );
            if enable_changed {
                push_undo = true;
                if refinement_controls_enabled {
                    refinement.enabled = true;
                    apply_refinement = true;
                } else {
                    restore_source = true;
                }
            }
            ui.add_enabled_ui(refinement_controls_enabled, |ui| {
                ui.horizontal_wrapped(|ui| {
                    if preset_button(ui, "標準") {
                        refinement = SubjectMaskRefinement {
                            enabled: true,
                            threshold: 0.52,
                            expand_px: 0,
                            feather_px: 1,
                        };
                        apply_refinement = true;
                        push_undo = true;
                    }
                    if preset_button(ui, "硬め") {
                        refinement = SubjectMaskRefinement {
                            enabled: true,
                            threshold: 0.58,
                            expand_px: -1,
                            feather_px: 0,
                        };
                        apply_refinement = true;
                        push_undo = true;
                    }
                    if preset_button(ui, "柔らかめ") {
                        refinement = SubjectMaskRefinement {
                            enabled: true,
                            threshold: 0.45,
                            expand_px: 0,
                            feather_px: 2,
                        };
                        apply_refinement = true;
                        push_undo = true;
                    }
                });
            });
            let mut threshold_started = false;
            let mut threshold_changed = false;
            let mut threshold_stopped = false;
            let mut expand_started = false;
            let mut expand_changed = false;
            let mut expand_stopped = false;
            let mut feather_started = false;
            let mut feather_changed = false;
            let mut feather_stopped = false;
            ui.add_enabled_ui(refinement_controls_enabled, |ui| {
                let threshold = ui.add(
                    egui::Slider::new(&mut refinement.threshold, 0.05..=0.95).text("しきい値"),
                );
                threshold_started = threshold.drag_started();
                threshold_changed = threshold.changed();
                threshold_stopped = threshold.drag_stopped();
                threshold.lab_hover_tip(
                    "この値以上を被写体として残します。上げるほど背景側の半透明が減ります。",
                );
                let expand =
                    ui.add(egui::Slider::new(&mut refinement.expand_px, -4..=4).text("収縮/拡張"));
                expand_started = expand.drag_started();
                expand_changed = expand.changed();
                expand_stopped = expand.drag_stopped();
                expand.lab_hover_tip(
                    "マイナスで少し内側へ縮め、プラスで外側へ広げます。背景のにじみがある時は -1 が効きます。",
                );
                let feather = ui.add(
                    egui::Slider::new(&mut refinement.feather_px, 0..=8).text("境界なめらか"),
                );
                feather_started = feather.drag_started();
                feather_changed = feather.changed();
                feather_stopped = feather.drag_stopped();
                feather.lab_hover_tip(
                    "2値化後の境界だけをなじませます。0は完全な2値、1〜2は切り抜き向けの軽い境界です。",
                );
            });
            let slider_started = threshold_started || expand_started || feather_started;
            let slider_changed = threshold_changed || expand_changed || feather_changed;
            let slider_stopped = threshold_stopped || expand_stopped || feather_stopped;
            if slider_started && !self.subject_cutout_edit_active {
                self.subject_cutout_edit_active = true;
                push_undo = true;
            }
            if slider_changed {
                refinement.enabled = true;
                apply_refinement = true;
                if !self.subject_cutout_edit_active {
                    self.subject_cutout_edit_active = true;
                    push_undo = true;
                }
            }
            if restore_source {
                self.restore_subject_source_mask(push_undo);
            } else if apply_refinement {
                self.apply_subject_cutout_refinement(refinement, push_undo);
            }
            if slider_stopped {
                self.subject_cutout_edit_active = false;
            }
            let (display_stats, display_enabled) = self
                .selected_subject_cutout_state()
                .map(|(stats, _, enabled)| (stats, enabled))
                .unwrap_or((stats, refinement_controls_enabled));
            let mode_label = if display_enabled {
                "整形済み"
            } else {
                "元マット"
            };
            ui.label(
                egui::RichText::new(format!(
                    "{mode_label} / 前景 {:.1}% / 半透明 {:.1}%",
                    display_stats.foreground_percent, display_stats.soft_percent
                ))
                .size(10.0)
                .color(Color32::from_gray(170)),
            );
        }
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
            .lab_hover_tip("大きいほど近い色が同じ領域にまとまり、小さいほど細かく分かれます。")
            .changed();
        let mut min_area = self.region_min_area as i32;
        if ui
            .add(egui::Slider::new(&mut min_area, 1..=2048).text("最小領域"))
            .lab_hover_tip(
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
        ui.label(
            egui::RichText::new("色ベース")
                .size(12.0)
                .strong()
                .color(Color32::from_gray(220)),
        );
        if ui
            .add_enabled(!pending, egui::Button::new("画像全体を領域分割"))
            .clicked()
        {
            self.start_region_segmentation(ui.ctx(), RegionSegmentationScope::Full);
        }
        if ui
            .add_enabled(
                !pending && subject_available,
                egui::Button::new("被写体内を領域分割"),
            )
            .clicked()
        {
            self.start_region_segmentation(ui.ctx(), RegionSegmentationScope::Subject);
        }
        if ui
            .add_enabled(
                !pending && subject_available,
                egui::Button::new("背景を領域分割"),
            )
            .clicked()
        {
            self.start_region_segmentation(ui.ctx(), RegionSegmentationScope::Background);
        }
        if !subject_available {
            ui.label(
                egui::RichText::new(
                    "被写体選択レイヤーを生成すると、被写体内や背景だけを分割できます。",
                )
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
            self.mark_mask_changed();
        }
        ui.label(
            egui::RichText::new(
                "画像上の色分け領域をクリックまたはドラッグして、追加/解除します。選択中の領域はマスクカラーと明るい境界で表示します。",
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
        let current_kind = self.add_layer_mask_kind;
        let mut add_requested = None;
        let subject_model_available = subject_model_available();
        let dialog_frame = egui::Frame::window(ctx.style().as_ref())
            .fill(Color32::from_rgba_unmultiplied(24, 24, 26, 245))
            .stroke(egui::Stroke::new(
                1.0,
                Color32::from_rgba_unmultiplied(255, 255, 255, 70),
            ));
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
                apply_lab_dark_ui(ui);
                ui.label(
                    egui::RichText::new(
                        "使いたいマスク種類を選んでください。クリックするとレイヤーを追加します。",
                    )
                    .size(11.0)
                    .color(Color32::from_gray(180)),
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
                                    .color(Color32::WHITE),
                            );
                            ui.horizontal_wrapped(|ui| {
                                ui.spacing_mut().item_spacing = egui::vec2(6.0, 6.0);
                                for &kind in group.kinds {
                                    let enabled =
                                        kind != MaskKind::Subject || subject_model_available;
                                    let fill = if !enabled {
                                        Color32::from_rgba_unmultiplied(54, 54, 54, 150)
                                    } else if kind == current_kind {
                                        Color32::from_rgb(36, 112, 150)
                                    } else {
                                        Color32::from_rgba_unmultiplied(70, 70, 70, 190)
                                    };
                                    let tip = if enabled {
                                        kind.description().to_string()
                                    } else {
                                        "被写体選択の新規生成には U²-Netp モデルが必要です。保存済みの被写体マスクは読み込んで利用できます。".to_string()
                                    };
                                    let response = ui
                                        .add_enabled(
                                            enabled,
                                            egui::Button::new(kind.label())
                                                .fill(fill)
                                                .min_size(egui::vec2(156.0, 30.0)),
                                        )
                                        .lab_hover_tip(tip);
                                    if response.clicked() {
                                        add_requested = Some(kind);
                                    }
                                }
                            });
                            ui.add_space(8.0);
                        }
                    });
            });

        self.add_layer_dialog_open = open;
        if let Some(kind) = add_requested {
            self.add_layer_with_mask_and_auto_generate(kind, ctx);
        }
    }

    fn draw_effect_picker_dialog(&mut self, ctx: &egui::Context) {
        if !self.effect_picker_dialog_open {
            return;
        }
        if self.layers.is_empty() {
            self.effect_picker_dialog_open = false;
            return;
        }

        let current_kind = self
            .layers
            .get(self.selected_layer)
            .map(|layer| EffectKind::from_effect(&layer.effect))
            .unwrap_or(EffectKind::None);
        let mut open = self.effect_picker_dialog_open;
        let mut selected_kind = None;
        let dialog_frame = egui::Frame::window(ctx.style().as_ref())
            .fill(Color32::from_rgba_unmultiplied(24, 24, 26, 248))
            .stroke(egui::Stroke::new(
                1.0,
                Color32::from_rgba_unmultiplied(255, 255, 255, 70),
            ));
        egui::Window::new("効果選択")
            .order(egui::Order::Foreground)
            .frame(dialog_frame)
            .anchor(egui::Align2::CENTER_CENTER, egui::Vec2::ZERO)
            .collapsible(false)
            .resizable(true)
            .default_width(720.0)
            .default_height(520.0)
            .open(&mut open)
            .show(ctx, |ui| {
                apply_lab_dark_ui(ui);
                ui.label(
                    egui::RichText::new(
                        "使いたい効果を選んでください。各ボタンにカーソルを置くと説明が出ます。",
                    )
                    .size(11.0)
                    .color(Color32::from_gray(180)),
                );
                ui.separator();
                egui::ScrollArea::vertical()
                    .max_height(440.0)
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        for group in EFFECT_GROUPS {
                            ui.label(
                                egui::RichText::new(group.title)
                                    .size(13.0)
                                    .strong()
                                    .color(Color32::WHITE),
                            );
                            ui.horizontal_wrapped(|ui| {
                                ui.spacing_mut().item_spacing = egui::vec2(6.0, 6.0);
                                for &kind in group.kinds {
                                    let selected = kind == current_kind;
                                    let fill = if selected {
                                        Color32::from_rgb(36, 112, 150)
                                    } else {
                                        Color32::from_rgba_unmultiplied(70, 70, 70, 190)
                                    };
                                    let response = ui
                                        .add_sized(
                                            egui::vec2(130.0, 28.0),
                                            egui::Button::new(kind.label()).fill(fill),
                                        )
                                        .lab_hover_tip(kind.description());
                                    if response.clicked() {
                                        selected_kind = Some(kind);
                                    }
                                }
                            });
                            ui.add_space(8.0);
                        }
                    });
            });

        self.effect_picker_dialog_open = open;
        if let Some(kind) = selected_kind
            && let Some(layer) = self.selected_layer_mut()
        {
            let effect = default_effect(kind);
            let mask_application = default_mask_application_for_effect(&effect);
            layer.effect = effect;
            layer.mask_before_effect = mask_application.before_effect;
            layer.mask_after_effect = mask_application.after_effect;
            self.effect_picker_dialog_open = false;
            if kind != EffectKind::SelectiveColor {
                self.selective_color_pick_active = false;
            }
            self.rgb_pick_active = None;
            self.effect_gradient_drag_active = false;
            self.status = format!("加工内容を「{}」に変更しました。", kind.label());
            self.mark_dirty();
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

    fn selected_effect_gradient_shape(&self) -> Option<ColorOverlayShape> {
        self.selected_layer_ref()
            .and_then(|layer| match &layer.effect {
                LocalEffect::ColorFill(params) => Some(params.shape),
                LocalEffect::ColorOverlay(params) => Some(params.shape),
                _ => None,
            })
            .filter(|shape| matches!(shape, ColorOverlayShape::Linear | ColorOverlayShape::Radial))
    }

    fn reset_selected_effect_gradient_geometry(&mut self) -> bool {
        let Some(layer) = self.selected_layer_mut() else {
            return false;
        };
        match &mut layer.effect {
            LocalEffect::ColorFill(params) => {
                let mut geometry = color_fill_gradient_geometry(params);
                if reset_color_gradient_geometry(&mut geometry) {
                    apply_color_fill_gradient_geometry(params, geometry);
                    true
                } else {
                    false
                }
            }
            LocalEffect::ColorOverlay(params) => {
                let mut geometry = color_overlay_gradient_geometry(params);
                if reset_color_gradient_geometry(&mut geometry) {
                    apply_color_overlay_gradient_geometry(params, geometry);
                    true
                } else {
                    false
                }
            }
            _ => false,
        }
    }

    fn selected_subject_cutout_state(
        &self,
    ) -> Option<(SubjectMaskStats, SubjectMaskRefinement, bool)> {
        let layer = self.selected_layer_ref()?;
        let LocalMask::Subject(mask) = &layer.mask else {
            return None;
        };
        Some((
            subject_mask_stats(mask),
            mask.refinement,
            mask.refinement.enabled,
        ))
    }

    fn restore_subject_source_mask(&mut self, push_undo: bool) {
        let layer_idx = self.selected_layer;
        if !matches!(
            self.layers.get(layer_idx).map(|layer| &layer.mask),
            Some(LocalMask::Subject(_))
        ) {
            self.status = "被写体選択レイヤーを選択してください。".to_string();
            return;
        }
        if push_undo {
            self.push_undo_snapshot();
        }
        let Some(layer) = self.layers.get_mut(layer_idx) else {
            return;
        };
        let LocalMask::Subject(mask) = &mut layer.mask else {
            return;
        };
        let source = mask.source_alpha.as_ref().unwrap_or(&mask.alpha).clone();
        mask.alpha = source;
        mask.refinement.enabled = false;
        let stats = subject_mask_stats(mask);
        self.status = format!(
            "被写体マスクを元マットに戻しました。前景 {:.1}% / 半透明 {:.1}%",
            stats.foreground_percent, stats.soft_percent
        );
        self.mark_mask_changed();
    }

    fn apply_subject_cutout_refinement(
        &mut self,
        refinement: SubjectMaskRefinement,
        push_undo: bool,
    ) {
        let layer_idx = self.selected_layer;
        if !matches!(
            self.layers.get(layer_idx).map(|layer| &layer.mask),
            Some(LocalMask::Subject(_))
        ) {
            self.status = "被写体選択レイヤーを選択してください。".to_string();
            return;
        }
        if push_undo {
            self.push_undo_snapshot();
        }
        let Some(layer) = self.layers.get_mut(layer_idx) else {
            return;
        };
        let LocalMask::Subject(mask) = &mut layer.mask else {
            return;
        };
        if mask.source_alpha.is_none() {
            mask.set_source_from_current();
        }
        let source = mask.source_raster_mask();
        let alpha = subject_cutout_refined_alpha(
            &source,
            refinement.threshold,
            refinement.expand_px,
            refinement.feather_px.max(0) as usize,
        );
        mask.alpha = alpha;
        mask.refinement = SubjectMaskRefinement {
            enabled: true,
            threshold: refinement.threshold,
            expand_px: refinement.expand_px,
            feather_px: refinement.feather_px.max(0),
        };
        let stats = subject_mask_stats(mask);
        self.status = format!(
            "被写体マスクを元マットから再整形しました。前景 {:.1}% / 半透明 {:.1}%",
            stats.foreground_percent, stats.soft_percent
        );
        self.mark_mask_changed();
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
            if let Some(effect_gradient_shape) = self.selected_effect_gradient_shape() {
                if secondary_pressed {
                    self.push_undo_snapshot();
                    if self.reset_selected_effect_gradient_geometry() {
                        self.effect_gradient_drag_active = false;
                        self.status = format!(
                            "{}を初期位置へ戻しました。",
                            color_overlay_shape_label(effect_gradient_shape)
                        );
                        self.mark_dirty();
                    }
                    return;
                }
                if primary_down {
                    let started = !self.effect_gradient_drag_active;
                    if started {
                        self.push_undo_snapshot();
                        self.effect_gradient_drag_active = true;
                    }
                    self.drag_effect_gradient_line(pos, started);
                    return;
                }
            }
            let tilt_shift_range_initialized = self.selected_layer_ref().and_then(|layer| {
                if let LocalEffect::TiltShift(params) = &layer.effect {
                    Some(params.range_initialized)
                } else {
                    None
                }
            });
            if let Some(initialized) = tilt_shift_range_initialized {
                if secondary_pressed && initialized {
                    self.push_undo_snapshot();
                    if let Some(layer) = self.selected_layer_mut()
                        && let LocalEffect::TiltShift(params) = &mut layer.effect
                    {
                        params.range_initialized = false;
                        params.mode_selected = true;
                    }
                    self.tilt_shift_drag_active = false;
                    self.status = "チルトぼかし範囲をクリアしました。".to_string();
                    self.mark_dirty();
                    return;
                }
                if primary_down && (!initialized || self.tilt_shift_drag_active) {
                    let started = !self.tilt_shift_drag_active;
                    if started {
                        self.push_undo_snapshot();
                        self.tilt_shift_drag_active = true;
                    }
                    self.drag_tilt_shift_range(pos, started);
                    return;
                }
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
                        self.mark_mask_changed();
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
                            self.mark_mask_tiles_changed(dirty_tiles);
                        } else {
                            self.mark_mask_changed();
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
                            self.mark_mask_tiles_changed(dirty_tiles);
                        } else {
                            self.mark_mask_changed();
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
                            self.mark_mask_tiles_changed(dirty_tiles);
                        } else {
                            self.mark_mask_changed();
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
                self.mark_mask_changed();
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
        let selected_mask_kind = self
            .selected_layer_ref()
            .map(|layer| MaskKind::from_mask(&layer.mask));
        if !raster_vector_edit_controls_visible(selected_mask_kind, self.override_edit_panel) {
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
        let selected_mask_kind = self
            .selected_layer_ref()
            .map(|layer| MaskKind::from_mask(&layer.mask));
        if !raster_vector_edit_controls_visible(selected_mask_kind, self.override_edit_panel) {
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
        Self::draw_brush_cursor_ring(ui.painter(), pos, radius, color);
        if matches!(self.tool, MaskTool::EdgeBrush) {
            ui.ctx()
                .request_repaint_after(Duration::from_millis(EDGE_OVERLAY_REPAINT_MS));
        }
        ui.ctx().set_cursor_icon(egui::CursorIcon::None);
    }

    fn draw_brush_cursor_ring(painter: &egui::Painter, pos: Pos2, radius: f32, color: Color32) {
        let radius = radius.max(1.0);
        painter.circle_stroke(
            pos,
            radius,
            egui::Stroke::new(3.25, Color32::from_rgba_unmultiplied(0, 0, 0, 220)),
        );
        painter.circle_stroke(
            pos,
            radius,
            egui::Stroke::new(2.0, Color32::from_rgba_unmultiplied(255, 255, 255, 220)),
        );
        painter.circle_stroke(pos, radius, egui::Stroke::new(1.2, color));

        if radius >= 7.0 {
            let arm = (radius * 0.35).clamp(4.0, 9.0);
            let gap = 2.0;
            let shadow = egui::Stroke::new(2.0, Color32::from_rgba_unmultiplied(0, 0, 0, 110));
            let mark = egui::Stroke::new(1.0, Color32::from_rgba_unmultiplied(255, 255, 255, 150));
            for stroke in [shadow, mark] {
                painter.line_segment(
                    [pos - egui::vec2(arm, 0.0), pos - egui::vec2(gap, 0.0)],
                    stroke,
                );
                painter.line_segment(
                    [pos + egui::vec2(gap, 0.0), pos + egui::vec2(arm, 0.0)],
                    stroke,
                );
                painter.line_segment(
                    [pos - egui::vec2(0.0, arm), pos - egui::vec2(0.0, gap)],
                    stroke,
                );
                painter.line_segment(
                    [pos + egui::vec2(0.0, gap), pos + egui::vec2(0.0, arm)],
                    stroke,
                );
            }
        }
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
        let visuals = mask_gradient_visuals();
        let stroke = visuals.stroke;
        let handle_fill = visuals.center_fill;
        let handle_stroke = visuals.handle_stroke;

        match &mut self.layers[layer_idx].mask {
            LocalMask::LinearGradient(mask) => {
                if !mask.initialized {
                    return false;
                }
                let (linear_changed, linear_used) = draw_linear_gradient_handles(
                    ui,
                    rect,
                    ui.id().with(("mask_linear_gradient", layer_idx)),
                    &mut mask.start,
                    &mut mask.end,
                    visuals,
                );
                changed |= linear_changed;
                used = linear_used;
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
                    .lab_hover_tip("内側 横");
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
                    .lab_hover_tip("内側 縦");
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
                    .lab_hover_tip("外側 横");
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
                    .lab_hover_tip("外側 縦");
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
            self.mark_mask_changed();
        }
        used
    }

    fn draw_effect_gradient_handles(&mut self, ui: &mut egui::Ui, rect: Rect) -> bool {
        let Some(layer_idx) = self
            .layers
            .get(self.selected_layer)
            .map(|_| self.selected_layer)
        else {
            return false;
        };
        let mut changed = false;
        let mut used = false;
        let visuals = effect_gradient_visuals();

        match &mut self.layers[layer_idx].effect {
            LocalEffect::ColorFill(params) => {
                let mut geometry = color_fill_gradient_geometry(params);
                let (geometry_changed, geometry_used) = draw_color_gradient_geometry_handles(
                    ui,
                    rect,
                    layer_idx,
                    "fill",
                    &mut geometry,
                    visuals,
                );
                if geometry_changed {
                    apply_color_fill_gradient_geometry(params, geometry);
                }
                changed |= geometry_changed;
                used |= geometry_used;
            }
            LocalEffect::ColorOverlay(params) => {
                let mut geometry = color_overlay_gradient_geometry(params);
                let (geometry_changed, geometry_used) = draw_color_gradient_geometry_handles(
                    ui,
                    rect,
                    layer_idx,
                    "overlay",
                    &mut geometry,
                    visuals,
                );
                if geometry_changed {
                    apply_color_overlay_gradient_geometry(params, geometry);
                }
                changed |= geometry_changed;
                used |= geometry_used;
            }
            _ => {}
        }

        if changed {
            self.mark_dirty();
        }
        used
    }

    fn draw_effect_position_handles(&mut self, ui: &mut egui::Ui, rect: Rect) -> bool {
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
        let image_dims = self.image_dims().unwrap_or((1, 1));
        let source_px_scale = screen_px_per_source_px(rect, image_dims);
        match &mut self.layers[layer_idx].effect {
            LocalEffect::RadialBlur(params) => {
                let (center_changed, center_used, center) = draw_effect_center_handle(
                    ui,
                    rect,
                    ui.id().with(("radial_blur_center", layer_idx)),
                    &mut params.center,
                    "放射ぼかし中心",
                    Color32::from_rgb(185, 235, 255),
                );
                changed |= center_changed;
                used |= center_used;

                let guide_stroke =
                    egui::Stroke::new(1.0, Color32::from_rgba_unmultiplied(120, 220, 255, 130));
                painter.circle_stroke(center, 18.0, guide_stroke);
            }
            LocalEffect::WaveDistortion(params) if params.mode == WaveDistortionMode::Ripple => {
                let (center_changed, center_used, center) = draw_effect_center_handle(
                    ui,
                    rect,
                    ui.id().with(("wave_distortion_center", layer_idx)),
                    &mut params.center,
                    "波形中心",
                    Color32::from_rgb(170, 235, 255),
                );
                changed |= center_changed;
                used |= center_used;

                let stroke =
                    egui::Stroke::new(1.0, Color32::from_rgba_unmultiplied(120, 220, 255, 115));
                for radius in [24.0, 48.0, 72.0] {
                    painter.circle_stroke(center, radius, stroke);
                }
            }
            LocalEffect::PinchSpherize(params) => {
                let (center_changed, center_used, center) = draw_effect_center_handle(
                    ui,
                    rect,
                    ui.id().with(("pinch_spherize_center", layer_idx)),
                    &mut params.center,
                    "つまむ/魚眼中心",
                    Color32::from_rgb(185, 235, 255),
                );
                changed |= center_changed;
                used |= center_used;
                draw_effect_source_radius(
                    &painter,
                    rect,
                    center,
                    params.radius_px,
                    source_px_scale,
                    Color32::from_rgba_unmultiplied(120, 220, 255, 150),
                );
            }
            LocalEffect::Twirl(params) => {
                let (center_changed, center_used, center) = draw_effect_center_handle(
                    ui,
                    rect,
                    ui.id().with(("twirl_center", layer_idx)),
                    &mut params.center,
                    "渦巻き中心",
                    Color32::from_rgb(190, 225, 255),
                );
                changed |= center_changed;
                used |= center_used;
                draw_effect_source_radius(
                    &painter,
                    rect,
                    center,
                    params.radius_px,
                    source_px_scale,
                    Color32::from_rgba_unmultiplied(130, 205, 255, 150),
                );
            }
            LocalEffect::PolarCoordinates(params) => {
                let (center_changed, center_used, center) = draw_effect_center_handle(
                    ui,
                    rect,
                    ui.id().with(("polar_coordinates_center", layer_idx)),
                    &mut params.center,
                    "極座標中心",
                    Color32::from_rgb(190, 225, 255),
                );
                changed |= center_changed;
                used |= center_used;
                draw_effect_source_radius(
                    &painter,
                    rect,
                    center,
                    params.radius_px,
                    source_px_scale,
                    Color32::from_rgba_unmultiplied(130, 205, 255, 150),
                );
            }
            LocalEffect::LensCorrection(params) => {
                let (center_changed, center_used, center) = draw_effect_center_handle(
                    ui,
                    rect,
                    ui.id().with(("lens_correction_center", layer_idx)),
                    &mut params.center,
                    "レンズ補正中心",
                    Color32::from_rgb(190, 225, 255),
                );
                changed |= center_changed;
                used |= center_used;

                let stroke =
                    egui::Stroke::new(1.0, Color32::from_rgba_unmultiplied(150, 215, 255, 125));
                let radius = rect.width().min(rect.height()) * 0.18;
                painter.circle_stroke(center, radius.max(2.0), stroke);
            }
            LocalEffect::GodRays(params) => {
                let center = norm_to_screen(rect, params.center);
                let (center_changed, center_used) = drag_norm_handle(
                    ui,
                    rect,
                    ui.id().with(("god_rays_center", layer_idx)),
                    center,
                    &mut params.center,
                    "光源位置",
                );
                changed |= center_changed;
                used |= center_used;

                let center = norm_to_screen(rect, params.center);
                let guide_stroke =
                    egui::Stroke::new(1.0, Color32::from_rgba_unmultiplied(255, 245, 170, 170));
                let handle_stroke = egui::Stroke::new(2.0, Color32::from_rgb(45, 35, 10));
                painter.circle_filled(center, 7.0, Color32::from_rgb(255, 238, 145));
                painter.circle_stroke(center, 7.0, handle_stroke);
                painter.line_segment(
                    [
                        Pos2::new(center.x - 14.0, center.y),
                        Pos2::new(center.x + 14.0, center.y),
                    ],
                    guide_stroke,
                );
                painter.line_segment(
                    [
                        Pos2::new(center.x, center.y - 14.0),
                        Pos2::new(center.x, center.y + 14.0),
                    ],
                    guide_stroke,
                );
            }
            LocalEffect::LensFlare(params) => {
                let (center_changed, center_used, center) = draw_effect_center_handle(
                    ui,
                    rect,
                    ui.id().with(("lens_flare_center", layer_idx)),
                    &mut params.center,
                    "フレア光源位置",
                    Color32::from_rgb(255, 232, 135),
                );
                changed |= center_changed;
                used |= center_used;

                draw_effect_source_radius(
                    &painter,
                    rect,
                    center,
                    params.radius_px,
                    source_px_scale,
                    Color32::from_rgba_unmultiplied(255, 226, 130, 145),
                );
                let guide_stroke =
                    egui::Stroke::new(1.0, Color32::from_rgba_unmultiplied(255, 245, 170, 135));
                painter.line_segment(
                    [
                        Pos2::new(center.x - 28.0, center.y),
                        Pos2::new(center.x + 28.0, center.y),
                    ],
                    guide_stroke,
                );
            }
            LocalEffect::SpeedLines(params) => {
                let (center_changed, center_used, center) = draw_effect_center_handle(
                    ui,
                    rect,
                    ui.id().with(("speed_lines_center", layer_idx)),
                    &mut params.center,
                    "集中線/スピード線中心",
                    Color32::from_rgb(215, 250, 255),
                );
                changed |= center_changed;
                used |= center_used;

                let stroke =
                    egui::Stroke::new(1.0, Color32::from_rgba_unmultiplied(160, 235, 255, 130));
                match params.mode {
                    SpeedLinesMode::Radial => {
                        let max_radius = distance_to_farthest_rect_corner(center, rect).max(1.0);
                        painter.circle_stroke(center, max_radius * params.inner_radius, stroke);
                        painter.circle_stroke(center, max_radius * params.outer_radius, stroke);
                    }
                    SpeedLinesMode::Parallel => {
                        let angle = params.angle_degrees.to_radians();
                        let dir = egui::vec2(angle.cos(), angle.sin());
                        let half = rect.width().hypot(rect.height()) * 0.5;
                        painter.line_segment([center - dir * half, center + dir * half], stroke);
                    }
                }
            }
            LocalEffect::Spotlight(params) => {
                let center = norm_to_screen(rect, params.center);
                let (center_changed, center_used) = drag_norm_handle(
                    ui,
                    rect,
                    ui.id().with(("spotlight_center", layer_idx)),
                    center,
                    &mut params.center,
                    "スポットライト位置",
                );
                changed |= center_changed;
                used |= center_used;

                let center = norm_to_screen(rect, params.center);
                let radius = params.radius.clamp(0.0, 1.0);
                let feather = params.feather.clamp(0.001, 1.0);
                let max_dim = rect.width().max(rect.height());
                let radius_px = max_dim * radius * 0.5;
                let outer_px = max_dim * (radius + feather).min(1.5) * 0.5;
                let soft_stroke =
                    egui::Stroke::new(1.0, Color32::from_rgba_unmultiplied(255, 230, 130, 120));
                let ring_stroke =
                    egui::Stroke::new(1.5, Color32::from_rgba_unmultiplied(255, 238, 160, 180));
                let handle_stroke = egui::Stroke::new(2.0, Color32::from_rgb(45, 35, 10));
                painter.circle_stroke(center, outer_px.max(2.0), soft_stroke);
                painter.circle_stroke(center, radius_px.max(2.0), ring_stroke);
                painter.circle_filled(center, 7.0, Color32::from_rgb(255, 224, 110));
                painter.circle_stroke(center, 7.0, handle_stroke);
            }
            _ => {}
        }

        if changed {
            self.hide_mask_preview();
            self.mark_dirty();
        }
        used
    }

    fn draw_tilt_shift_handles(&mut self, ui: &mut egui::Ui, rect: Rect) -> bool {
        let Some(layer_idx) = self
            .layers
            .get(self.selected_layer)
            .map(|_| self.selected_layer)
        else {
            return false;
        };
        let mut changed = false;
        let used;
        let painter = ui.painter().clone();
        let stroke = egui::Stroke::new(2.0, Color32::from_rgb(100, 220, 255));
        let soft_stroke =
            egui::Stroke::new(1.0, Color32::from_rgba_unmultiplied(100, 220, 255, 150));
        let focus_stroke =
            egui::Stroke::new(1.0, Color32::from_rgba_unmultiplied(255, 230, 120, 180));
        let handle_fill = Color32::from_rgb(210, 245, 255);
        let focus_fill = Color32::from_rgb(255, 238, 150);
        let outer_fill = Color32::from_rgb(120, 220, 255);
        let handle_stroke = egui::Stroke::new(2.0, Color32::from_rgb(10, 30, 36));

        let LocalEffect::TiltShift(params) = &mut self.layers[layer_idx].effect else {
            return false;
        };
        if !params.range_initialized {
            return false;
        }

        match params.mode {
            TiltShiftMode::Linear => {
                let center = norm_to_screen(rect, params.center);
                let angle = params.angle_degrees.to_radians();
                let dir = [angle.cos(), angle.sin()];
                let perp = [-dir[1], dir[0]];
                let focus = params.focus_width.max(0.0);
                let outer = focus + params.falloff.max(0.001);

                let draw_boundary = |amount: f32, stroke: egui::Stroke| {
                    let base = offset_norm(params.center, dir, amount);
                    let a = norm_to_screen_unclamped(rect, offset_norm(base, perp, -1.6));
                    let b = norm_to_screen_unclamped(rect, offset_norm(base, perp, 1.6));
                    painter.line_segment([a, b], stroke);
                };

                if params.far_only {
                    painter.line_segment(
                        [
                            center,
                            norm_to_screen_unclamped(rect, offset_norm(params.center, dir, outer)),
                        ],
                        stroke,
                    );
                    draw_boundary(focus, focus_stroke);
                    draw_boundary(outer, stroke);
                } else {
                    painter.line_segment(
                        [
                            norm_to_screen_unclamped(rect, offset_norm(params.center, dir, -outer)),
                            norm_to_screen_unclamped(rect, offset_norm(params.center, dir, outer)),
                        ],
                        stroke,
                    );
                    draw_boundary(-focus, focus_stroke);
                    draw_boundary(focus, focus_stroke);
                    draw_boundary(-outer, soft_stroke);
                    draw_boundary(outer, stroke);
                }

                let focus_handle =
                    norm_to_screen_unclamped(rect, offset_norm(params.center, dir, focus));
                let outer_handle =
                    norm_to_screen_unclamped(rect, offset_norm(params.center, dir, outer));
                let (center_changed, center_used) = drag_norm_handle(
                    ui,
                    rect,
                    ui.id().with(("tilt_center", layer_idx)),
                    center,
                    &mut params.center,
                    "中心",
                );
                let focus_resp = ui
                    .interact(
                        Rect::from_center_size(focus_handle, egui::vec2(26.0, 26.0)),
                        ui.id().with(("tilt_focus", layer_idx)),
                        Sense::drag(),
                    )
                    .lab_hover_tip("焦点幅");
                if focus_resp.dragged()
                    && let Some(pos) = focus_resp.interact_pointer_pos()
                {
                    let n = screen_to_norm(rect, pos);
                    let dx = n[0] - params.center[0];
                    let dy = n[1] - params.center[1];
                    let distance = (dx * dx + dy * dy).sqrt();
                    if distance > 0.001 {
                        params.angle_degrees = dy.atan2(dx).to_degrees();
                        params.focus_width = distance.min(0.8);
                        params.falloff = params.falloff.max(0.001);
                        changed = true;
                    }
                }
                let outer_resp = ui
                    .interact(
                        Rect::from_center_size(outer_handle, egui::vec2(28.0, 28.0)),
                        ui.id().with(("tilt_outer", layer_idx)),
                        Sense::drag(),
                    )
                    .lab_hover_tip("ぼかし境界");
                if outer_resp.dragged()
                    && let Some(pos) = outer_resp.interact_pointer_pos()
                {
                    let n = screen_to_norm(rect, pos);
                    let dx = n[0] - params.center[0];
                    let dy = n[1] - params.center[1];
                    let distance = (dx * dx + dy * dy).sqrt();
                    if distance > 0.001 {
                        params.angle_degrees = dy.atan2(dx).to_degrees();
                        params.falloff =
                            (distance - params.focus_width.max(0.0)).max(0.001).min(1.2);
                        changed = true;
                    }
                }
                painter.circle_filled(center, 6.0, handle_fill);
                painter.circle_stroke(center, 6.0, handle_stroke);
                painter.circle_filled(focus_handle, 5.0, focus_fill);
                painter.circle_stroke(focus_handle, 5.0, handle_stroke);
                painter.circle_filled(outer_handle, 6.0, outer_fill);
                painter.circle_stroke(outer_handle, 6.0, handle_stroke);
                changed |= center_changed;
                used = center_used
                    || focus_resp.hovered()
                    || focus_resp.dragged()
                    || outer_resp.hovered()
                    || outer_resp.dragged();
            }
            TiltShiftMode::Radial => {
                let center = norm_to_screen(rect, params.center);
                let inner_rx = params.radius[0].max(0.001) * rect.width();
                let inner_ry = params.radius[1].max(0.001) * rect.height();
                let outer_rx =
                    params.radius[0].max(0.001) * (1.0 + params.falloff.max(0.001)) * rect.width();
                let outer_ry =
                    params.radius[1].max(0.001) * (1.0 + params.falloff.max(0.001)) * rect.height();
                draw_ellipse_stroke(&painter, center, inner_rx, inner_ry, focus_stroke);
                draw_ellipse_stroke(&painter, center, outer_rx, outer_ry, stroke);

                let inner_x_handle = Pos2::new(center.x + inner_rx, center.y);
                let inner_y_handle = Pos2::new(center.x, center.y + inner_ry);
                let outer_x_handle = Pos2::new(center.x + outer_rx, center.y);
                let outer_y_handle = Pos2::new(center.x, center.y + outer_ry);

                let (center_changed, center_used) = drag_norm_handle(
                    ui,
                    rect,
                    ui.id().with(("tilt_radial_center", layer_idx)),
                    center,
                    &mut params.center,
                    "中心",
                );
                let inner_x_resp = ui
                    .interact(
                        Rect::from_center_size(inner_x_handle, egui::vec2(26.0, 26.0)),
                        ui.id().with(("tilt_inner_x", layer_idx)),
                        Sense::drag(),
                    )
                    .lab_hover_tip("焦点 横");
                if inner_x_resp.dragged()
                    && let Some(pos) = inner_x_resp.interact_pointer_pos()
                {
                    let n = screen_to_norm(rect, pos);
                    params.radius[0] = (n[0] - params.center[0]).abs().clamp(0.001, 1.2);
                    changed = true;
                }
                let inner_y_resp = ui
                    .interact(
                        Rect::from_center_size(inner_y_handle, egui::vec2(26.0, 26.0)),
                        ui.id().with(("tilt_inner_y", layer_idx)),
                        Sense::drag(),
                    )
                    .lab_hover_tip("焦点 縦");
                if inner_y_resp.dragged()
                    && let Some(pos) = inner_y_resp.interact_pointer_pos()
                {
                    let n = screen_to_norm(rect, pos);
                    params.radius[1] = (n[1] - params.center[1]).abs().clamp(0.001, 1.2);
                    changed = true;
                }
                let outer_x_resp = ui
                    .interact(
                        Rect::from_center_size(outer_x_handle, egui::vec2(28.0, 28.0)),
                        ui.id().with(("tilt_outer_x", layer_idx)),
                        Sense::drag(),
                    )
                    .lab_hover_tip("ぼかし境界 横");
                if outer_x_resp.dragged()
                    && let Some(pos) = outer_x_resp.interact_pointer_pos()
                {
                    let n = screen_to_norm(rect, pos);
                    let outer = (n[0] - params.center[0]).abs();
                    params.falloff = (outer / params.radius[0].max(0.001) - 1.0)
                        .max(0.001)
                        .min(1.2);
                    changed = true;
                }
                let outer_y_resp = ui
                    .interact(
                        Rect::from_center_size(outer_y_handle, egui::vec2(28.0, 28.0)),
                        ui.id().with(("tilt_outer_y", layer_idx)),
                        Sense::drag(),
                    )
                    .lab_hover_tip("ぼかし境界 縦");
                if outer_y_resp.dragged()
                    && let Some(pos) = outer_y_resp.interact_pointer_pos()
                {
                    let n = screen_to_norm(rect, pos);
                    let outer = (n[1] - params.center[1]).abs();
                    params.falloff = (outer / params.radius[1].max(0.001) - 1.0)
                        .max(0.001)
                        .min(1.2);
                    changed = true;
                }
                painter.line_segment(
                    [Pos2::new(center.x - outer_rx, center.y), outer_x_handle],
                    egui::Stroke::new(1.0, Color32::from_rgba_unmultiplied(100, 220, 255, 110)),
                );
                painter.line_segment(
                    [Pos2::new(center.x, center.y - outer_ry), outer_y_handle],
                    egui::Stroke::new(1.0, Color32::from_rgba_unmultiplied(100, 220, 255, 110)),
                );
                painter.circle_filled(center, 6.0, handle_fill);
                painter.circle_stroke(center, 6.0, handle_stroke);
                painter.circle_filled(inner_x_handle, 5.0, focus_fill);
                painter.circle_stroke(inner_x_handle, 5.0, handle_stroke);
                painter.circle_filled(inner_y_handle, 5.0, focus_fill);
                painter.circle_stroke(inner_y_handle, 5.0, handle_stroke);
                painter.circle_filled(outer_x_handle, 6.0, outer_fill);
                painter.circle_stroke(outer_x_handle, 6.0, handle_stroke);
                painter.circle_filled(outer_y_handle, 6.0, outer_fill);
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
        }

        if changed {
            params.range_initialized = true;
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
        let crop_active = self.crop_is_active();
        if !self.crop_edit_mode && !crop_active {
            self.crop_drag = None;
            self.crop_create_drag = None;
            return false;
        }
        let crop = self
            .ensure_crop_rect()
            .unwrap_or_else(|| CropRect::full(img_w, img_h));
        let crop_screen = crop.to_screen_rect(image_rect, img_w, img_h);
        let painter = ui.painter().clone();
        if self.crop_edit_mode || crop_active {
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

        // Handles / dragging only exist in the crop edit panel. A non-edit view that
        // still has an active crop just shows the boundary above.
        if !self.crop_edit_mode {
            return false;
        }

        // Anchor points for the 8 resize handles, clamped inward so they stay grabbable
        // when the crop touches the image edge. `handle_points` is reused for both the
        // visual dots and the manual hit-test below, so they can never drift apart.
        let handle_bounds = image_rect.shrink(14.0);
        let handle_points = crop_handle_points(crop_screen);
        for (handle, center) in handle_points {
            if handle == CropHandle::Body {
                continue;
            }
            let handle_center = clamp_pos_to_rect(center, handle_bounds);
            painter.circle_filled(handle_center, 5.5, Color32::from_rgb(255, 245, 180));
            painter.circle_stroke(
                handle_center,
                5.5,
                egui::Stroke::new(1.5, Color32::from_rgb(30, 20, 0)),
            );
        }

        // While panning, or while the pointer is over a side panel, we must not start or
        // continue a crop gesture. Drop any in-flight gesture so it can't silently resume
        // later with a stale anchor.
        if !pointer_allowed {
            self.crop_drag = None;
            self.crop_create_drag = None;
            return false;
        }

        // Crop editing is driven from the *raw* pointer state instead of per-widget
        // `ui.interact()` results. Two reasons:
        //   1. The canvas allocates a full-size `Sense::click_and_drag` response, and
        //      egui's hit-test can route a press to that background widget instead of the
        //      overlay, starving the handles' `drag_started()`.
        //   2. The old code keyed the create-vs-move branch on `crop_is_active()` captured
        //      at frame start. The first drag frame turned the rect non-full, so the next
        //      frame flipped `crop_active` to true and abandoned the in-progress create
        //      gesture (the move branch couldn't take over because `crop_drag` was None),
        //      which made a freshly dragged crop snap back instantly.
        // A latched gesture (`crop_drag` / `crop_create_drag`) plus `total_drag_delta`
        // (cumulative since the press, not the per-frame `drag_delta`) fixes both.
        let (primary_pressed, primary_down, press_origin, total_delta) = ui.input(|i| {
            (
                i.pointer.primary_pressed(),
                i.pointer.primary_down(),
                i.pointer.press_origin(),
                i.pointer.total_drag_delta().unwrap_or(egui::Vec2::ZERO),
            )
        });
        let scale_x = img_w.max(1) as f32 / image_rect.width().max(1.0);
        let scale_y = img_h.max(1) as f32 / image_rect.height().max(1.0);
        const HANDLE_HIT: f32 = 32.0;
        // Below this much pointer travel (screen px) a press is treated as a click, not a
        // drag, so a stray click never collapses the crop into a 1px rectangle.
        const CREATE_DRAG_THRESHOLD: f32 = 4.0;
        let to_image =
            |p: Pos2| screen_to_image(image_rect, img_w, img_h, clamp_pos_to_rect(p, image_rect));
        let press_target = |p: Pos2| {
            crop_press_target(
                p,
                image_rect,
                crop_screen,
                crop_active,
                &handle_points,
                handle_bounds,
                HANDLE_HIT,
            )
        };

        // Classify the gesture from the *press origin*, not the live pointer, so a press
        // that already moved within the first frame still latches the right target.
        let create_moved_enough = match (press_origin, pointer_screen) {
            (Some(origin), Some(cur)) => (cur - origin).length() >= CREATE_DRAG_THRESHOLD,
            _ => false,
        };
        let gesture_input = CropGestureInput {
            primary_pressed,
            primary_down,
            press_target: press_origin.and_then(press_target),
            press_image: press_origin.and_then(to_image),
            current_image: pointer_screen.and_then(to_image),
            create_moved_enough,
            base_at_press: crop,
            resize_aspect: self.crop_resize_aspect_ratio(),
            create_aspect: self.crop_resize_aspect_ratio(),
            total_delta_image: (total_delta.x * scale_x, total_delta.y * scale_y),
            img_w,
            img_h,
        };

        let (next_gesture, next_crop) = crop_gesture_step(self.crop_gesture(), &gesture_input);
        self.set_crop_gesture(next_gesture);
        let mut used = false;
        if let Some(next) = next_crop {
            self.crop_enabled = !next.is_full(img_w, img_h);
            self.crop_rect = Some(next);
        }
        if primary_down {
            match next_gesture {
                CropGesture::Resize(drag) => {
                    ui.ctx()
                        .set_cursor_icon(if drag.handle == CropHandle::Body {
                            egui::CursorIcon::Grabbing
                        } else {
                            crop_handle_cursor(drag.handle)
                        });
                    used = true;
                }
                CropGesture::Create(_) => {
                    ui.ctx().set_cursor_icon(egui::CursorIcon::Crosshair);
                    used = true;
                }
                CropGesture::Idle => {}
            }
        }

        // Idle hover cursor feedback (resize / move / create affordance).
        if !used && let Some(p) = pointer_screen {
            match press_target(p) {
                Some(CropPressTarget::Resize(handle)) => {
                    ui.ctx().set_cursor_icon(crop_handle_cursor(handle));
                    used = true;
                }
                Some(CropPressTarget::Move) => {
                    ui.ctx().set_cursor_icon(egui::CursorIcon::Grab);
                    used = true;
                }
                Some(CropPressTarget::Create) => {
                    ui.ctx().set_cursor_icon(egui::CursorIcon::Crosshair);
                    used = true;
                }
                None => {}
            }
        }

        used || !matches!(self.crop_gesture(), CropGesture::Idle)
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
                self.mark_mask_changed();
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
                self.mark_mask_changed();
            }
            _ => {}
        }
    }

    fn drag_effect_gradient_line(&mut self, image_pos: Pos2, started: bool) {
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
        let changed = match &mut layer.effect {
            LocalEffect::ColorFill(params) => {
                let mut geometry = color_fill_gradient_geometry(params);
                let changed = drag_color_gradient_geometry(&mut geometry, n, started);
                if changed {
                    apply_color_fill_gradient_geometry(params, geometry);
                }
                changed
            }
            LocalEffect::ColorOverlay(params) => {
                let mut geometry = color_overlay_gradient_geometry(params);
                let changed = drag_color_gradient_geometry(&mut geometry, n, started);
                if changed {
                    apply_color_overlay_gradient_geometry(params, geometry);
                }
                changed
            }
            _ => false,
        };
        if changed {
            self.mark_dirty();
        }
    }

    fn drag_tilt_shift_range(&mut self, image_pos: Pos2, started: bool) {
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
        let LocalEffect::TiltShift(params) = &mut layer.effect else {
            return;
        };
        if started {
            params.mode_selected = false;
            params.range_initialized = true;
            params.center = n;
            if params.strength <= f32::EPSILON {
                params.strength = 1.0;
            }
            if params.max_radius_px <= f32::EPSILON {
                params.max_radius_px = 20.0;
            }
            match params.mode {
                TiltShiftMode::Linear => {
                    params.focus_width = 0.0;
                    params.falloff = 0.001;
                }
                TiltShiftMode::Radial => {
                    params.radius = [0.001, 0.001];
                    params.falloff = 0.40;
                }
            }
        } else {
            let dx = n[0] - params.center[0];
            let dy = n[1] - params.center[1];
            match params.mode {
                TiltShiftMode::Linear => {
                    let distance = (dx * dx + dy * dy).sqrt();
                    if distance > 0.001 {
                        params.angle_degrees = dy.atan2(dx).to_degrees();
                        params.focus_width = (distance * 0.35).clamp(0.0, 0.8);
                        params.falloff = (distance * 0.65).clamp(0.001, 1.2);
                    }
                }
                TiltShiftMode::Radial => {
                    let distance = (dx * dx + dy * dy).sqrt();
                    if distance > 0.001 {
                        let rx = dx.abs().max(distance * 0.35).clamp(0.001, 1.2);
                        let ry = dy.abs().max(distance * 0.35).clamp(0.001, 1.2);
                        params.radius = [rx, ry];
                    }
                }
            }
        }
        self.mark_dirty();
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
            self.effect_gradient_drag_active = false;
            self.tilt_shift_drag_active = false;
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
                    self.mark_mask_changed();
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

        self.draw_menu_bar(ctx);
        self.poll_segmentation(ctx);
        self.poll_lut_load(ctx);
        self.poll_render(ctx);
        self.maybe_start_render(ctx);
        if self.pending.is_some()
            || self.segmentation_pending.is_some()
            || self.lut_load_pending.is_some()
        {
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
        self.draw_effect_picker_dialog(ctx);
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
        LocalMask::Subject(_) => {
            ui.label("AI生成した被写体マスクを使います。必要な修正は左パネルの追加/削除マスクから行います。");
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

fn draw_mask_application_button(
    ui: &mut egui::Ui,
    label: &'static str,
    active: bool,
) -> egui::Response {
    let fill = if active {
        Color32::from_rgba_unmultiplied(92, 132, 190, 230)
    } else {
        Color32::from_rgba_unmultiplied(36, 38, 42, 170)
    };
    let text_color = if active {
        Color32::WHITE
    } else {
        Color32::from_gray(165)
    };
    ui.add_sized(
        egui::vec2(20.0, 18.0),
        egui::Button::new(egui::RichText::new(label).size(10.0).color(text_color))
            .fill(fill)
            .corner_radius(3.0),
    )
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
        LocalEffect::RgbToneCurve(_) => "RGBカーブ".to_string(),
        LocalEffect::ColorBalance(_) => "カラーバランス".to_string(),
        LocalEffect::ThreeWayColorGrading(_) => "3-wayグレーディング".to_string(),
        LocalEffect::SelectiveColor(params) => {
            format!("選択色 {:.0}°", params.target_hue_degrees.rem_euclid(360.0))
        }
        LocalEffect::ChannelMixer(params) => {
            if params.monochrome {
                "チャンネル白黒".to_string()
            } else {
                "チャンネルミキサー".to_string()
            }
        }
        LocalEffect::Clarity(_) => "明瞭度".to_string(),
        LocalEffect::Texture(params) => format!("テクスチャ {:+.0}%", params.amount * 100.0),
        LocalEffect::HighPass(params) => {
            if params.detail_only {
                format!("ハイパス抽出 {:.0}px", params.radius_px)
            } else {
                format!("ハイパス {:.0}%", params.amount * 100.0)
            }
        }
        LocalEffect::HighlightsShadows(_) => "ハイライト/シャドウ".to_string(),
        LocalEffect::Dehaze(params) => format!("かすみ除去 {:.0}%", params.amount * 100.0),
        LocalEffect::Blur(params) => format!("ぼかし {:.0}px", params.radius_px),
        LocalEffect::MotionBlur(params) => {
            format!(
                "移動ぼかし {:.0}px {:.0}°",
                params.distance_px, params.angle_degrees
            )
        }
        LocalEffect::Wind(params) => {
            let direction = match params.direction {
                WindDirection::Right => "右へ",
                WindDirection::Left => "左へ",
                WindDirection::Down => "下へ",
                WindDirection::Up => "上へ",
            };
            format!("風 {direction} {:.0}px", params.distance_px)
        }
        LocalEffect::SpeedLines(params) => match params.mode {
            SpeedLinesMode::Radial => format!("集中線 {}本", params.line_count),
            SpeedLinesMode::Parallel => format!("スピード線 {}本", params.line_count),
        },
        LocalEffect::TiltShift(params) => {
            let mode = match params.mode {
                TiltShiftMode::Linear => "線形",
                TiltShiftMode::Radial => "円形",
            };
            format!("チルトシフト {mode} {:.0}px", params.max_radius_px)
        }
        LocalEffect::LensBlur(params) => {
            let aperture = match params.aperture {
                LensBlurAperture::Circular => "円",
                LensBlurAperture::Hexagon => "6角",
                LensBlurAperture::Octagon => "8角",
            };
            format!("レンズぼかし {aperture} {:.0}px", params.radius_px)
        }
        LocalEffect::RadialBlur(params) => match params.mode {
            RadialBlurMode::Zoom => format!("ズームぼかし {:.0}px", params.zoom_px),
            RadialBlurMode::Spin => format!("回転ぼかし {:+.0}°", params.spin_degrees),
        },
        LocalEffect::WaveDistortion(params) => {
            format!("波形ゆがみ {:.0}px", params.amplitude_px)
        }
        LocalEffect::PinchSpherize(params) => {
            if params.amount >= 0.0 {
                format!("魚眼 {:.0}%", params.amount * 100.0)
            } else {
                format!("つまむ {:.0}%", -params.amount * 100.0)
            }
        }
        LocalEffect::Twirl(params) => format!("渦巻き {:+.0}°", params.angle_degrees),
        LocalEffect::PolarCoordinates(params) => {
            let mode = match params.mode {
                PolarCoordinatesMode::RectToPolar => "矩形→円形",
                PolarCoordinatesMode::PolarToRect => "円形→矩形",
            };
            format!("極座標 {mode}")
        }
        LocalEffect::GlassDisplacement(params) => {
            let mode = match params.mode {
                GlassDisplacementMode::Frosted => "すりガラス",
                GlassDisplacementMode::Ripple => "波ガラス",
                GlassDisplacementMode::Faceted => "面ガラス",
            };
            format!("ガラス変位 {mode} {:.0}px", params.displacement_px)
        }
        LocalEffect::LensCorrection(params) => {
            if params.distortion.abs() > f32::EPSILON {
                let mode = if params.distortion >= 0.0 {
                    "樽型補正"
                } else {
                    "糸巻き補正"
                };
                format!("レンズ補正 {mode} {:.0}%", params.distortion.abs() * 100.0)
            } else {
                format!("周辺減光補正 {:.0}%", params.vignette_correction * 100.0)
            }
        }
        LocalEffect::LineExtract(params) => {
            let mode = match params.mode {
                LineExtractMode::BlackOnWhite => "黒線",
                LineExtractMode::WhiteOnBlack => "白線",
                LineExtractMode::DarkenOriginal => "元画像に黒線",
                LineExtractMode::LightenOriginal => "元画像に白線",
            };
            format!("線画抽出 {mode}")
        }
        LocalEffect::ArtisticMedia(params) => {
            let mode = match params.mode {
                ArtisticMediaMode::Watercolor => "水彩",
                ArtisticMediaMode::ColoredPencil => "色鉛筆",
                ArtisticMediaMode::PencilSketch => "鉛筆画",
            };
            format!("絵画調 {mode}")
        }
        LocalEffect::BrushStroke(params) => {
            let mode = match params.mode {
                BrushStrokeMode::DryBrush => "ドライブラシ",
                BrushStrokeMode::PaintDaubs => "塗料",
                BrushStrokeMode::PaletteKnife => "パレットナイフ",
            };
            format!("筆致 {mode}")
        }
        LocalEffect::Cutout(params) => format!("切り絵 {}段", params.levels),
        LocalEffect::Emboss(params) => format!("エンボス {:.0}°", params.angle_degrees),
        LocalEffect::PixelStylize(params) => {
            let mode = match params.mode {
                PixelStylizeMode::Crystallize => "結晶化",
                PixelStylizeMode::Pointillize => "点描",
                PixelStylizeMode::Facet => "Facet",
                PixelStylizeMode::Mezzotint => "メゾチント",
            };
            format!("粒状スタイル {mode}")
        }
        LocalEffect::Solarize(params) => {
            format!("ソラリゼーション {:.0}%", params.threshold * 100.0)
        }
        LocalEffect::GlowingEdges(params) => {
            format!("エッジ光彩 {:.0}px", params.glow_radius_px)
        }
        LocalEffect::OilPaint(params) => format!("油彩 {:.0}px", params.radius_px),
        LocalEffect::SoftFocus(params) => format!("ソフトフォーカス {:.0}px", params.radius_px),
        LocalEffect::Mosaic(params) => format!(
            "モザイク {}",
            mosaic_tile_mode_label(params.effective_tile_mode())
        ),
        LocalEffect::Sharpen(params) => {
            if params.threshold > 0.0 {
                format!(
                    "シャープ {:.0}% / しきい値 {:.0}",
                    params.amount * 100.0,
                    params.threshold
                )
            } else {
                format!("シャープ {:.0}%", params.amount * 100.0)
            }
        }
        LocalEffect::SmartSharpen(params) => {
            format!("スマートシャープ {:.0}%", params.amount * 100.0)
        }
        LocalEffect::Hsl(params) => format!("色相 {:+.0}°", params.hue_degrees),
        LocalEffect::ColorMixer(params) => {
            format!("カラーミキサー {}色", color_mixer_adjusted_count(params))
        }
        LocalEffect::Look(params) => format!("ルック {}", look_preset_label(params.preset)),
        LocalEffect::CubeLut(params) => {
            if params.is_loaded() {
                format!("3D LUT {}", params.name)
            } else {
                "3D LUT 未読込".to_string()
            }
        }
        LocalEffect::Posterize(params) => format!("ポスタリゼーション {}段", params.levels),
        LocalEffect::Threshold(params) => {
            let suffix = if params.invert { " 反転" } else { "" };
            format!("2値化 {:.0}%{suffix}", params.threshold * 100.0)
        }
        LocalEffect::Invert(params) => format!("ネガ {:.0}%", params.strength * 100.0),
        LocalEffect::Duotone(params) => {
            format!("ダブルトーン {}", duotone_preset_label(params.preset))
        }
        LocalEffect::Equalize(params) => format!("平坦化 {:.0}%", params.strength * 100.0),
        LocalEffect::GradientMap(params) => {
            format!(
                "グラデーション {}",
                gradient_map_preset_label(params.preset)
            )
        }
        LocalEffect::ColorFill(params) => {
            if params.shape == ColorOverlayShape::Unselected {
                "塗りつぶし 選択してください".to_string()
            } else {
                format!(
                    "塗りつぶし {} {:.0}%",
                    color_overlay_shape_label(params.shape),
                    params.opacity * 100.0
                )
            }
        }
        LocalEffect::OutlineStroke(params) => format!(
            "縁取り {} {:.0}px",
            outline_stroke_placement_label(params.placement),
            params.width_px
        ),
        LocalEffect::ColorOverlay(params) => format!(
            "塗り {} {:.0}%",
            color_overlay_blend_mode_label(params.blend_mode),
            params.opacity * 100.0
        ),
        LocalEffect::NeonGlow(params) => format!(
            "ネオングロー {:.0}/{:.0}px {:.0}%",
            params.inner_radius_px,
            params.outer_radius_px,
            params.strength * 100.0
        ),
        LocalEffect::DiffuseGlow(params) => format!("拡散光彩 {:.0}px", params.radius_px),
        LocalEffect::Bloom(params) => format!("ブルーム {:.0}px", params.radius_px),
        LocalEffect::GodRays(params) => format!("光芒 {:.0}px", params.length_px),
        LocalEffect::LensFlare(params) => {
            format!("レンズフレア {:.0}%", params.strength * 100.0)
        }
        LocalEffect::CloudFog(params) => match params.mode {
            CloudFogMode::Fog => format!("霧 {:.0}%", params.opacity * 100.0),
            CloudFogMode::Clouds => format!("雲 {:.0}%", params.opacity * 100.0),
        },
        LocalEffect::Spotlight(params) => format!(
            "スポットライト +{:.0}% / 影 {:.0}%",
            params.light_strength * 100.0,
            params.shadow_strength * 100.0
        ),
        LocalEffect::Vignette(params) => format!("ビネット {:.0}%", params.strength * 100.0),
        LocalEffect::FilmGrain(params) => format!("粒子 {:.0}%", params.amount * 100.0),
        LocalEffect::Noise(params) => format!(
            "ノイズ {} {:.0}%",
            noise_distribution_label(params.distribution),
            params.amount * 100.0
        ),
        LocalEffect::ChromaticAberration(params) => {
            format!("色収差 {:.1}px", params.offset_px)
        }
        LocalEffect::Halftone(params) => format!("ハーフトーン {}px", params.cell_px),
        LocalEffect::ScreenTone(params) => format!(
            "スクリーントーン {} {:.0}px",
            screen_tone_mode_label(params.mode),
            params.cell_px
        ),
        LocalEffect::ColorHalftone(params) => {
            format!("カラーハーフトーン {:.0}px", params.cell_px)
        }
        LocalEffect::Textureizer(params) => format!(
            "テクスチャ {} {:.0}px",
            textureizer_mode_label(params.mode),
            params.scale_px
        ),
        LocalEffect::StarGlow(params) => {
            format!("クロス光 {}本 {:.0}px", params.ray_count, params.length_px)
        }
        LocalEffect::EdgeSmooth(params) => {
            format!("エッジ保持ぼかし {:.0}px", params.radius_px)
        }
        LocalEffect::Despeckle(params) => {
            format!("ディスペックル {:.0}px", params.radius_px)
        }
        LocalEffect::Median(params) => format!("メディアン {:.0}px", params.radius_px),
    }
}

fn effect_has_position_handles(effect: &LocalEffect) -> bool {
    matches!(
        effect,
        LocalEffect::RadialBlur(_)
            | LocalEffect::WaveDistortion(WaveDistortionParams {
                mode: WaveDistortionMode::Ripple,
                ..
            })
            | LocalEffect::PinchSpherize(_)
            | LocalEffect::Twirl(_)
            | LocalEffect::PolarCoordinates(_)
            | LocalEffect::LensCorrection(_)
            | LocalEffect::GodRays(_)
            | LocalEffect::LensFlare(_)
            | LocalEffect::SpeedLines(_)
            | LocalEffect::Spotlight(_)
    )
}

fn screen_tone_mode_label(mode: ScreenToneMode) -> &'static str {
    match mode {
        ScreenToneMode::Dots => "網点",
        ScreenToneMode::Lines => "線",
        ScreenToneMode::CrossHatch => "カケアミ",
    }
}

fn noise_distribution_label(distribution: NoiseDistribution) -> &'static str {
    match distribution {
        NoiseDistribution::Uniform => "均一",
        NoiseDistribution::Gaussian => "ガウス",
    }
}

fn textureizer_mode_label(mode: TextureizerMode) -> &'static str {
    match mode {
        TextureizerMode::Paper => "紙目",
        TextureizerMode::Canvas => "キャンバス",
        TextureizerMode::Linen => "リネン",
    }
}

fn mosaic_tile_mode_label(mode: MosaicTileMode) -> String {
    match mode {
        MosaicTileMode::LongEdgeRatio(multiplier) => format!("長辺x{multiplier:.2}"),
        MosaicTileMode::FixedPx(px) => {
            if px <= 1 {
                "1px(無効)".to_string()
            } else {
                format!("{px}px")
            }
        }
    }
}

fn mosaic_boundary_label(boundary: MosaicBoundary) -> &'static str {
    match boundary {
        MosaicBoundary::Opaque => "タイル不透明",
        MosaicBoundary::Translucent => "割合で半透明",
        MosaicBoundary::MaskShape => "マスク形状",
    }
}

fn look_preset_label(preset: LookPreset) -> &'static str {
    match preset {
        LookPreset::None => "選択してください",
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

fn color_mixer_adjusted_count(params: &ColorMixerParams) -> usize {
    params
        .bands
        .iter()
        .filter(|band| {
            band.hue_degrees.abs() > f32::EPSILON
                || band.saturation.abs() > f32::EPSILON
                || band.lightness.abs() > f32::EPSILON
        })
        .count()
}

fn color_mixer_band_label(index: usize) -> &'static str {
    match index {
        0 => "赤",
        1 => "橙/肌",
        2 => "黄",
        3 => "緑",
        4 => "シアン",
        5 => "青",
        6 => "紫",
        7 => "マゼンタ",
        _ => "色帯",
    }
}

fn gradient_map_preset_label(preset: GradientMapPreset) -> &'static str {
    match preset {
        GradientMapPreset::None => "選択してください",
        GradientMapPreset::Mono => "モノクロ",
        GradientMapPreset::Sepia => "セピア",
        GradientMapPreset::Sunset => "夕焼け",
        GradientMapPreset::Twilight => "薄暮",
        GradientMapPreset::TealOrange => "ティール&オレンジ",
        GradientMapPreset::Cherry => "桜色",
        GradientMapPreset::Forest => "森",
        GradientMapPreset::Fire => "炎",
        GradientMapPreset::Ice => "氷",
    }
}

fn color_overlay_shape_label(shape: ColorOverlayShape) -> &'static str {
    match shape {
        ColorOverlayShape::Unselected => "選択してください",
        ColorOverlayShape::Solid => "単色",
        ColorOverlayShape::Linear => "線形グラデーション",
        ColorOverlayShape::Radial => "円形グラデーション",
    }
}

fn outline_stroke_placement_label(placement: OutlineStrokePlacement) -> &'static str {
    match placement {
        OutlineStrokePlacement::Outside => "外側",
        OutlineStrokePlacement::Inside => "内側",
        OutlineStrokePlacement::Center => "中央",
    }
}

fn color_overlay_blend_mode_label(mode: ColorOverlayBlendMode) -> &'static str {
    match mode {
        ColorOverlayBlendMode::Normal => "通常",
        ColorOverlayBlendMode::Multiply => "乗算",
        ColorOverlayBlendMode::Screen => "スクリーン",
        ColorOverlayBlendMode::Overlay => "オーバーレイ",
        ColorOverlayBlendMode::SoftLight => "ソフトライト",
        ColorOverlayBlendMode::Color => "カラー",
    }
}

#[derive(Debug, Default)]
struct RgbColorControlResponse {
    changed: bool,
    start_pick: Option<RgbPickTarget>,
    cancel_pick: bool,
}

fn draw_rgb_color_control(
    ui: &mut egui::Ui,
    label: &str,
    rgb: &mut [u8; 3],
    target: RgbPickTarget,
    active_pick: Option<RgbPickTarget>,
) -> RgbColorControlResponse {
    let before = *rgb;
    let mut start_pick = None;
    let mut cancel_pick = false;
    let pick_active = active_pick == Some(target);
    ui.horizontal(|ui| {
        ui.label(egui::RichText::new(label).color(Color32::from_gray(190)));
        ui.label(
            egui::RichText::new(format!("#{:02X}{:02X}{:02X}", rgb[0], rgb[1], rgb[2]))
                .monospace()
                .color(Color32::from_gray(170)),
        );
        let response = ui.color_edit_button_srgb(rgb);
        response.lab_hover_tip(format!("{label}を選びます。"));
        let button_text = if pick_active {
            "スポイト解除"
        } else {
            "スポイト"
        };
        let pick_response = ui.selectable_label(pick_active, button_text);
        if pick_response.clicked() {
            if pick_active {
                cancel_pick = true;
            } else {
                start_pick = Some(target);
            }
        }
        pick_response.lab_hover_tip("画像上をクリックしてこの色へ取り込みます。");
    });
    ui.indent((label, "rgb_sliders"), |ui| {
        let mut r = rgb[0] as i32;
        let mut g = rgb[1] as i32;
        let mut b = rgb[2] as i32;
        let red = ui.add(egui::Slider::new(&mut r, 0..=255).text("R"));
        let green = ui.add(egui::Slider::new(&mut g, 0..=255).text("G"));
        let blue = ui.add(egui::Slider::new(&mut b, 0..=255).text("B"));
        if red.changed() || green.changed() || blue.changed() {
            *rgb = [r as u8, g as u8, b as u8];
        }
        red.lab_hover_tip("赤チャンネルです。");
        green.lab_hover_tip("緑チャンネルです。");
        blue.lab_hover_tip("青チャンネルです。");
    });
    RgbColorControlResponse {
        changed: *rgb != before,
        start_pick,
        cancel_pick,
    }
}

fn merge_rgb_color_response(
    response: RgbColorControlResponse,
    changed: &mut bool,
    start_rgb_pick: &mut Option<RgbPickTarget>,
    cancel_rgb_pick: &mut bool,
) {
    *changed |= response.changed;
    if response.cancel_pick {
        *cancel_rgb_pick = true;
    }
    if response.start_pick.is_some() {
        *start_rgb_pick = response.start_pick;
    }
}

fn set_rgb_pick_target(effect: &mut LocalEffect, target: RgbPickTarget, rgb: [u8; 3]) -> bool {
    match (effect, target) {
        (LocalEffect::ColorFill(params), RgbPickTarget::ColorFillStart) => {
            params.start_rgb = rgb;
            true
        }
        (LocalEffect::ColorFill(params), RgbPickTarget::ColorFillMiddle) => {
            params.middle_rgb = rgb;
            true
        }
        (LocalEffect::ColorFill(params), RgbPickTarget::ColorFillEnd) => {
            params.end_rgb = rgb;
            true
        }
        (LocalEffect::ColorOverlay(params), RgbPickTarget::ColorOverlayStart) => {
            params.start_rgb = rgb;
            true
        }
        (LocalEffect::ColorOverlay(params), RgbPickTarget::ColorOverlayEnd) => {
            params.end_rgb = rgb;
            true
        }
        (LocalEffect::NeonGlow(params), RgbPickTarget::NeonGlowSource) => {
            params.source_rgb = rgb;
            params.source_color_enabled = true;
            true
        }
        (LocalEffect::NeonGlow(params), RgbPickTarget::NeonGlowTint) => {
            params.tint_rgb = rgb;
            true
        }
        (LocalEffect::SpeedLines(params), RgbPickTarget::SpeedLinesColor) => {
            params.color_rgb = rgb;
            true
        }
        (LocalEffect::CloudFog(params), RgbPickTarget::CloudFogColor) => {
            params.color_rgb = rgb;
            true
        }
        (LocalEffect::Spotlight(params), RgbPickTarget::SpotlightTint) => {
            params.tint_rgb = rgb;
            true
        }
        (LocalEffect::OutlineStroke(params), RgbPickTarget::OutlineStrokeColor) => {
            params.color_rgb = rgb;
            true
        }
        _ => false,
    }
}

fn duotone_preset_label(preset: DuotonePreset) -> &'static str {
    match preset {
        DuotonePreset::None => "選択してください",
        DuotonePreset::SepiaInk => "セピアインク",
        DuotonePreset::Cyanotype => "青写真",
        DuotonePreset::BlackRed => "黒赤",
        DuotonePreset::PurpleGold => "紫金",
        DuotonePreset::TealCream => "ティールクリーム",
        DuotonePreset::SunsetTritone => "夕暮れ3色",
        DuotonePreset::ComicTritone => "コミック3色",
        DuotonePreset::NoirTritone => "ノワール3色",
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

fn preset_button(ui: &mut egui::Ui, label: &str) -> bool {
    ui.add(egui::Button::new(label).small()).clicked()
}

fn draw_tone_curve_preview(ui: &mut egui::Ui, params: ToneCurveParams) {
    draw_curve_preview_lines(
        ui,
        &[(
            params.points,
            Color32::from_rgb(120, 210, 255),
            egui::Stroke::new(2.0, Color32::from_rgb(120, 210, 255)),
        )],
    );
}

fn draw_rgb_tone_curve_preview(ui: &mut egui::Ui, params: RgbToneCurveParams) {
    draw_curve_preview_lines(
        ui,
        &[
            (
                params.master,
                Color32::from_rgb(230, 230, 230),
                egui::Stroke::new(1.5, Color32::from_rgb(230, 230, 230)),
            ),
            (
                params.red,
                Color32::from_rgb(255, 95, 115),
                egui::Stroke::new(2.0, Color32::from_rgb(255, 95, 115)),
            ),
            (
                params.green,
                Color32::from_rgb(95, 220, 120),
                egui::Stroke::new(2.0, Color32::from_rgb(95, 220, 120)),
            ),
            (
                params.blue,
                Color32::from_rgb(110, 150, 255),
                egui::Stroke::new(2.0, Color32::from_rgb(110, 150, 255)),
            ),
        ],
    );
}

fn draw_curve_preview_lines(ui: &mut egui::Ui, curves: &[([f32; 5], Color32, egui::Stroke)]) {
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
    for &(points, _color, stroke) in curves {
        let mut prev = None;
        for i in 0..=64 {
            let x01 = i as f32 / 64.0;
            let y01 = preview_tone_curve_value(x01, points);
            let p = Pos2::new(
                egui::lerp(rect.left()..=rect.right(), x01),
                egui::lerp(rect.bottom()..=rect.top(), y01),
            );
            if let Some(prev) = prev {
                painter.line_segment([prev, p], stroke);
            }
            prev = Some(p);
        }
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

fn draw_curve_point_sliders(ui: &mut egui::Ui, points: &mut [f32; 5]) -> bool {
    let mut changed = false;
    for (idx, label) in ["黒", "暗部", "中間", "明部", "白"].iter().enumerate() {
        let response = ui.add(egui::Slider::new(&mut points[idx], 0.0..=1.0).text(*label));
        changed |= response.changed();
        response.lab_hover_tip("左ほど暗部、右ほど明部の出力明るさです。");
    }
    changed
}

fn draw_color_balance_range_sliders(ui: &mut egui::Ui, range: &mut ColorBalanceRange) -> bool {
    let mut changed = false;
    let cyan_red =
        ui.add(egui::Slider::new(&mut range.cyan_red, -100.0..=100.0).text("シアン / 赤"));
    changed |= cyan_red.changed();
    cyan_red.lab_hover_tip("負の値でシアン寄り、正の値で赤寄りにします。");
    let magenta_green =
        ui.add(egui::Slider::new(&mut range.magenta_green, -100.0..=100.0).text("マゼンタ / 緑"));
    changed |= magenta_green.changed();
    magenta_green.lab_hover_tip("負の値でマゼンタ寄り、正の値で緑寄りにします。");
    let yellow_blue =
        ui.add(egui::Slider::new(&mut range.yellow_blue, -100.0..=100.0).text("黄 / 青"));
    changed |= yellow_blue.changed();
    yellow_blue.lab_hover_tip("負の値で黄寄り、正の値で青寄りにします。");
    changed
}

fn draw_color_grade_wheel_sliders(ui: &mut egui::Ui, wheel: &mut ColorGradeWheel) -> bool {
    let mut changed = false;
    let hue = ui.add(
        egui::Slider::new(&mut wheel.hue_degrees, 0.0..=360.0)
            .text("色相")
            .suffix("°"),
    );
    changed |= hue.changed();
    hue.lab_hover_tip("この明るさ帯に足す色味です。彩度が0のときは色相だけでは変化しません。");
    let saturation = ui.add(egui::Slider::new(&mut wheel.saturation, 0.0..=100.0).text("彩度"));
    changed |= saturation.changed();
    saturation.lab_hover_tip("色相で選んだ色味をどれだけ足すかを調整します。");
    let luminance = ui.add(egui::Slider::new(&mut wheel.luminance, -100.0..=100.0).text("明るさ"));
    changed |= luminance.changed();
    luminance.lab_hover_tip("この明るさ帯だけを明るく、または暗くします。");
    changed
}

fn draw_channel_coeff_sliders(ui: &mut egui::Ui, coeffs: &mut [f32; 3]) -> bool {
    let mut changed = false;
    let red = ui.add(egui::Slider::new(&mut coeffs[0], -200.0..=200.0).text("赤"));
    changed |= red.changed();
    red.lab_hover_tip("元画像の赤チャンネルをどれだけ混ぜるかです。100で等倍、0で不使用です。");
    let green = ui.add(egui::Slider::new(&mut coeffs[1], -200.0..=200.0).text("緑"));
    changed |= green.changed();
    green.lab_hover_tip("元画像の緑チャンネルをどれだけ混ぜるかです。");
    let blue = ui.add(egui::Slider::new(&mut coeffs[2], -200.0..=200.0).text("青"));
    changed |= blue.changed();
    blue.lab_hover_tip("元画像の青チャンネルをどれだけ混ぜるかです。負の値も使えます。");
    changed
}

#[derive(Debug, Default)]
struct EffectParamResponse {
    changed: bool,
    load_cube_lut: bool,
    start_selective_color_pick: bool,
    cancel_selective_color_pick: bool,
    start_rgb_pick: Option<RgbPickTarget>,
    cancel_rgb_pick: bool,
    set_effect_position_handles_visible: Option<bool>,
}

fn draw_effect_position_handle_toggle(ui: &mut egui::Ui, visible: bool) -> Option<bool> {
    let mut show_handles = visible;
    let response = ui.checkbox(&mut show_handles, "画像ハンドルを表示");
    let changed = response.changed();
    response.lab_hover_tip("ONの間、画像上の位置ハンドルをドラッグして中心位置を調整できます。");
    changed.then_some(show_handles)
}

fn draw_effect_center_controls(
    ui: &mut egui::Ui,
    center: &mut [f32; 2],
    x_tip: impl Into<egui::WidgetText>,
    y_tip: impl Into<egui::WidgetText>,
    effect_position_handles_visible: bool,
    set_effect_position_handles_visible: &mut Option<bool>,
) -> bool {
    if let Some(visible) = draw_effect_position_handle_toggle(ui, effect_position_handles_visible) {
        *set_effect_position_handles_visible = Some(visible);
    }

    let mut changed = false;
    let center_x = ui.add(egui::Slider::new(&mut center[0], 0.0..=1.0).text("中心 X"));
    changed |= center_x.changed();
    center_x.lab_hover_tip(x_tip);
    let center_y = ui.add(egui::Slider::new(&mut center[1], 0.0..=1.0).text("中心 Y"));
    changed |= center_y.changed();
    center_y.lab_hover_tip(y_tip);
    changed
}

fn draw_effect_params(
    ui: &mut egui::Ui,
    layer: &mut LocalAdjustmentLayer,
    image_dims: (usize, usize),
    selective_color_pick_active: bool,
    rgb_pick_active: Option<RgbPickTarget>,
    effect_position_handles_visible: bool,
) -> EffectParamResponse {
    let mut changed = false;
    let mut load_cube_lut = false;
    let mut start_selective_color_pick = false;
    let mut cancel_selective_color_pick = false;
    let mut start_rgb_pick = None;
    let mut cancel_rgb_pick = false;
    let mut set_effect_position_handles_visible = None;
    let effect_kind = EffectKind::from_effect(&layer.effect);
    let has_effect = !matches!(&layer.effect, LocalEffect::None);
    ui.horizontal(|ui| {
        ui.label(
            egui::RichText::new("加工パラメータ")
                .size(14.0)
                .strong()
                .color(Color32::WHITE),
        );
        if has_effect {
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.button("リセット").clicked() {
                    layer.effect = default_effect(effect_kind);
                    changed = true;
                }
            });
        }
    });
    if changed {
        return EffectParamResponse {
            changed,
            load_cube_lut,
            start_selective_color_pick,
            cancel_selective_color_pick,
            start_rgb_pick,
            cancel_rgb_pick,
            set_effect_position_handles_visible,
        };
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
                if preset_button(ui, "緑かぶり補正") {
                    *params = ToneParams {
                        tint: 28.0,
                        ..Default::default()
                    };
                    changed = true;
                }
                if preset_button(ui, "マゼンタ補正") {
                    *params = ToneParams {
                        tint: -28.0,
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
            let tint_response = ui.add(
                egui::Slider::new(&mut params.tint, -100.0..=100.0)
                    .text("色かぶり補正")
                    .custom_formatter(|v, _| {
                        if v.abs() < 0.5 {
                            "0".to_string()
                        } else if v > 0.0 {
                            format!("マゼンタ {:.0}", v)
                        } else {
                            format!("緑 {:.0}", -v)
                        }
                    }),
            );
            changed |= tint_response.changed();
            tint_response.lab_hover_tip(
                "緑-マゼンタ方向の色かぶりを補正します。右へ動かすとマゼンタ寄り、左へ動かすと緑寄りになります。",
            );
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
                egui::RichText::new(
                    "RGB共通の簡易カーブです。色チャンネルは RGBカーブ で調整します。",
                )
                .size(10.0)
                .color(Color32::from_gray(170)),
            );
            for (idx, label) in ["黒", "暗部", "中間", "明部", "白"].iter().enumerate() {
                changed |= ui
                    .add(egui::Slider::new(&mut params.points[idx], 0.0..=1.0).text(*label))
                    .changed();
            }
        }
        LocalEffect::RgbToneCurve(params) => {
            ui.label(egui::RichText::new("プリセット").color(Color32::from_gray(190)));
            ui.horizontal_wrapped(|ui| {
                if preset_button(ui, "暖色") {
                    *params = RgbToneCurveParams {
                        red: [0.0, 0.30, 0.58, 0.82, 1.0],
                        blue: [0.0, 0.20, 0.44, 0.70, 1.0],
                        ..Default::default()
                    };
                    changed = true;
                }
                if preset_button(ui, "寒色") {
                    *params = RgbToneCurveParams {
                        red: [0.0, 0.20, 0.44, 0.70, 1.0],
                        blue: [0.0, 0.31, 0.60, 0.84, 1.0],
                        ..Default::default()
                    };
                    changed = true;
                }
                if preset_button(ui, "フィルム") {
                    *params = RgbToneCurveParams {
                        master: [0.06, 0.24, 0.50, 0.76, 0.96],
                        red: [0.0, 0.25, 0.53, 0.82, 1.0],
                        blue: [0.06, 0.30, 0.52, 0.72, 0.95],
                        ..Default::default()
                    };
                    changed = true;
                }
                if preset_button(ui, "影を青く") {
                    *params = RgbToneCurveParams {
                        red: [0.0, 0.18, 0.46, 0.75, 1.0],
                        blue: [0.08, 0.34, 0.54, 0.76, 1.0],
                        ..Default::default()
                    };
                    changed = true;
                }
                if preset_button(ui, "明部を暖かく") {
                    *params = RgbToneCurveParams {
                        red: [0.0, 0.25, 0.52, 0.84, 1.0],
                        green: [0.0, 0.25, 0.51, 0.78, 1.0],
                        blue: [0.0, 0.25, 0.48, 0.66, 0.94],
                        ..Default::default()
                    };
                    changed = true;
                }
                if preset_button(ui, "クロス") {
                    *params = RgbToneCurveParams {
                        master: [0.04, 0.22, 0.50, 0.78, 0.98],
                        red: [0.0, 0.20, 0.48, 0.82, 1.0],
                        green: [0.0, 0.27, 0.52, 0.74, 1.0],
                        blue: [0.08, 0.34, 0.54, 0.72, 0.94],
                    };
                    changed = true;
                }
            });
            draw_rgb_tone_curve_preview(ui, *params);
            ui.label(
                egui::RichText::new(
                    "白い線が全体、赤/緑/青の線が各チャンネルです。全体カーブ後に各チャンネルを適用します。",
                )
                .size(10.0)
                .color(Color32::from_gray(170)),
            );
            ui.collapsing("全体", |ui| {
                changed |= draw_curve_point_sliders(ui, &mut params.master);
            });
            ui.collapsing("赤", |ui| {
                changed |= draw_curve_point_sliders(ui, &mut params.red);
            });
            ui.collapsing("緑", |ui| {
                changed |= draw_curve_point_sliders(ui, &mut params.green);
            });
            ui.collapsing("青", |ui| {
                changed |= draw_curve_point_sliders(ui, &mut params.blue);
            });
        }
        LocalEffect::ColorBalance(params) => {
            ui.label(egui::RichText::new("プリセット").color(Color32::from_gray(190)));
            ui.horizontal_wrapped(|ui| {
                if preset_button(ui, "影を青く") {
                    *params = ColorBalanceParams {
                        shadows: ColorBalanceRange {
                            yellow_blue: 42.0,
                            cyan_red: -10.0,
                            ..Default::default()
                        },
                        ..Default::default()
                    };
                    changed = true;
                }
                if preset_button(ui, "影を青緑") {
                    *params = ColorBalanceParams {
                        shadows: ColorBalanceRange {
                            cyan_red: -28.0,
                            magenta_green: 12.0,
                            yellow_blue: 30.0,
                        },
                        ..Default::default()
                    };
                    changed = true;
                }
                if preset_button(ui, "明部を暖かく") {
                    *params = ColorBalanceParams {
                        highlights: ColorBalanceRange {
                            cyan_red: 24.0,
                            yellow_blue: -34.0,
                            ..Default::default()
                        },
                        ..Default::default()
                    };
                    changed = true;
                }
                if preset_button(ui, "夕景") {
                    *params = ColorBalanceParams {
                        shadows: ColorBalanceRange {
                            yellow_blue: 16.0,
                            ..Default::default()
                        },
                        midtones: ColorBalanceRange {
                            cyan_red: 12.0,
                            yellow_blue: -14.0,
                            ..Default::default()
                        },
                        highlights: ColorBalanceRange {
                            cyan_red: 30.0,
                            yellow_blue: -42.0,
                            ..Default::default()
                        },
                        ..Default::default()
                    };
                    changed = true;
                }
                if preset_button(ui, "緑かぶり補正") {
                    *params = ColorBalanceParams {
                        midtones: ColorBalanceRange {
                            magenta_green: -26.0,
                            ..Default::default()
                        },
                        highlights: ColorBalanceRange {
                            magenta_green: -12.0,
                            ..Default::default()
                        },
                        ..Default::default()
                    };
                    changed = true;
                }
                if preset_button(ui, "シネマ") {
                    *params = ColorBalanceParams {
                        shadows: ColorBalanceRange {
                            cyan_red: -30.0,
                            yellow_blue: 26.0,
                            ..Default::default()
                        },
                        highlights: ColorBalanceRange {
                            cyan_red: 24.0,
                            yellow_blue: -24.0,
                            ..Default::default()
                        },
                        ..Default::default()
                    };
                    changed = true;
                }
            });
            ui.label(
                egui::RichText::new(
                    "明るさの帯ごとに色を寄せます。RGBカーブより直感的に色かぶりや空気感を調整できます。",
                )
                .size(10.0)
                .color(Color32::from_gray(170)),
            );
            ui.collapsing("シャドウ", |ui| {
                changed |= draw_color_balance_range_sliders(ui, &mut params.shadows);
            });
            ui.collapsing("中間", |ui| {
                changed |= draw_color_balance_range_sliders(ui, &mut params.midtones);
            });
            ui.collapsing("ハイライト", |ui| {
                changed |= draw_color_balance_range_sliders(ui, &mut params.highlights);
            });
            let preserve = ui.checkbox(&mut params.preserve_luma, "明るさを保つ");
            changed |= preserve.changed();
            preserve.lab_hover_tip(
                "色だけを寄せたいときに使います。オフにすると色変更による明るさ変化も残します。",
            );
        }
        LocalEffect::ThreeWayColorGrading(params) => {
            ui.label(egui::RichText::new("プリセット").color(Color32::from_gray(190)));
            ui.horizontal_wrapped(|ui| {
                if preset_button(ui, "シネマ") {
                    *params = ThreeWayColorGradingParams {
                        shadows: ColorGradeWheel {
                            hue_degrees: 205.0,
                            saturation: 42.0,
                            luminance: -8.0,
                        },
                        highlights: ColorGradeWheel {
                            hue_degrees: 36.0,
                            saturation: 36.0,
                            luminance: 6.0,
                        },
                        balance: 0.0,
                        ..Default::default()
                    };
                    changed = true;
                }
                if preset_button(ui, "夕焼け") {
                    *params = ThreeWayColorGradingParams {
                        shadows: ColorGradeWheel {
                            hue_degrees: 250.0,
                            saturation: 22.0,
                            luminance: -4.0,
                        },
                        midtones: ColorGradeWheel {
                            hue_degrees: 18.0,
                            saturation: 18.0,
                            luminance: 2.0,
                        },
                        highlights: ColorGradeWheel {
                            hue_degrees: 42.0,
                            saturation: 48.0,
                            luminance: 8.0,
                        },
                        ..Default::default()
                    };
                    changed = true;
                }
                if preset_button(ui, "月明かり") {
                    *params = ThreeWayColorGradingParams {
                        shadows: ColorGradeWheel {
                            hue_degrees: 220.0,
                            saturation: 34.0,
                            luminance: -6.0,
                        },
                        midtones: ColorGradeWheel {
                            hue_degrees: 210.0,
                            saturation: 16.0,
                            luminance: -2.0,
                        },
                        highlights: ColorGradeWheel {
                            hue_degrees: 190.0,
                            saturation: 12.0,
                            luminance: 5.0,
                        },
                        ..Default::default()
                    };
                    changed = true;
                }
                if preset_button(ui, "桜色") {
                    *params = ThreeWayColorGradingParams {
                        midtones: ColorGradeWheel {
                            hue_degrees: 335.0,
                            saturation: 20.0,
                            luminance: 3.0,
                        },
                        highlights: ColorGradeWheel {
                            hue_degrees: 350.0,
                            saturation: 24.0,
                            luminance: 8.0,
                        },
                        ..Default::default()
                    };
                    changed = true;
                }
                if preset_button(ui, "サイバー") {
                    *params = ThreeWayColorGradingParams {
                        shadows: ColorGradeWheel {
                            hue_degrees: 270.0,
                            saturation: 36.0,
                            luminance: -4.0,
                        },
                        midtones: ColorGradeWheel {
                            hue_degrees: 190.0,
                            saturation: 18.0,
                            luminance: 0.0,
                        },
                        highlights: ColorGradeWheel {
                            hue_degrees: 315.0,
                            saturation: 34.0,
                            luminance: 7.0,
                        },
                        ..Default::default()
                    };
                    changed = true;
                }
                if preset_button(ui, "淡い光") {
                    *params = ThreeWayColorGradingParams {
                        shadows: ColorGradeWheel {
                            hue_degrees: 225.0,
                            saturation: 12.0,
                            luminance: 4.0,
                        },
                        midtones: ColorGradeWheel {
                            hue_degrees: 32.0,
                            saturation: 10.0,
                            luminance: 6.0,
                        },
                        highlights: ColorGradeWheel {
                            hue_degrees: 48.0,
                            saturation: 18.0,
                            luminance: 12.0,
                        },
                        ..Default::default()
                    };
                    changed = true;
                }
            });
            ui.label(
                egui::RichText::new(
                    "カラーバランスより演出的な仕上げ向けです。色相と彩度で足す色を選び、明るさで帯ごとの持ち上げ/締めを調整します。",
                )
                .size(10.0)
                .color(Color32::from_gray(170)),
            );
            ui.collapsing("シャドウ", |ui| {
                changed |= draw_color_grade_wheel_sliders(ui, &mut params.shadows);
            });
            ui.collapsing("中間", |ui| {
                changed |= draw_color_grade_wheel_sliders(ui, &mut params.midtones);
            });
            ui.collapsing("ハイライト", |ui| {
                changed |= draw_color_grade_wheel_sliders(ui, &mut params.highlights);
            });
            let balance =
                ui.add(egui::Slider::new(&mut params.balance, -100.0..=100.0).text("バランス"));
            changed |= balance.changed();
            balance.lab_hover_tip(
                "負の値でシャドウ寄り、正の値でハイライト寄りに効果範囲をずらします。",
            );
        }
        LocalEffect::SelectiveColor(params) => {
            ui.label(egui::RichText::new("プリセット").color(Color32::from_gray(190)));
            ui.horizontal_wrapped(|ui| {
                if preset_button(ui, "赤を桜色") {
                    *params = SelectiveColorParams {
                        target_hue_degrees: 0.0,
                        range_degrees: 18.0,
                        feather_degrees: 18.0,
                        hue_degrees: 18.0,
                        saturation: -12.0,
                        lightness: 10.0,
                    };
                    changed = true;
                }
                if preset_button(ui, "肌を明るく") {
                    *params = SelectiveColorParams {
                        target_hue_degrees: 28.0,
                        range_degrees: 24.0,
                        feather_degrees: 24.0,
                        saturation: 4.0,
                        lightness: 12.0,
                        ..Default::default()
                    };
                    changed = true;
                }
                if preset_button(ui, "空を青く") {
                    *params = SelectiveColorParams {
                        target_hue_degrees: 205.0,
                        range_degrees: 28.0,
                        feather_degrees: 24.0,
                        saturation: 30.0,
                        lightness: -8.0,
                        ..Default::default()
                    };
                    changed = true;
                }
                if preset_button(ui, "緑を鮮やか") {
                    *params = SelectiveColorParams {
                        target_hue_degrees: 120.0,
                        range_degrees: 28.0,
                        feather_degrees: 24.0,
                        saturation: 34.0,
                        lightness: 4.0,
                        ..Default::default()
                    };
                    changed = true;
                }
                if preset_button(ui, "青を紫へ") {
                    *params = SelectiveColorParams {
                        target_hue_degrees: 235.0,
                        range_degrees: 26.0,
                        feather_degrees: 22.0,
                        hue_degrees: 35.0,
                        saturation: 12.0,
                        ..Default::default()
                    };
                    changed = true;
                }
                if preset_button(ui, "黄を橙へ") {
                    *params = SelectiveColorParams {
                        target_hue_degrees: 58.0,
                        range_degrees: 24.0,
                        feather_degrees: 18.0,
                        hue_degrees: -18.0,
                        saturation: 12.0,
                        lightness: -2.0,
                    };
                    changed = true;
                }
            });
            ui.label(
                egui::RichText::new(
                    "対象色相に近い色だけを補正します。色が広く変わりすぎる場合は範囲やぼかしを小さくしてください。",
                )
                .size(10.0)
                .color(Color32::from_gray(170)),
            );
            ui.horizontal_wrapped(|ui| {
                let swatch = hsl_swatch_color(params.target_hue_degrees, 0.8, 0.55);
                let (rect, _) = ui.allocate_exact_size(egui::vec2(22.0, 22.0), Sense::hover());
                ui.painter().rect_filled(rect, 4.0, swatch);
                ui.painter().rect_stroke(
                    rect,
                    4.0,
                    egui::Stroke::new(1.0, Color32::from_rgba_unmultiplied(255, 255, 255, 110)),
                    egui::StrokeKind::Inside,
                );
                let label = if selective_color_pick_active {
                    "スポイト解除"
                } else {
                    "スポイトで対象色を取得"
                };
                let response = ui.button(label);
                if response.clicked() {
                    if selective_color_pick_active {
                        cancel_selective_color_pick = true;
                    } else {
                        start_selective_color_pick = true;
                    }
                }
                response.lab_hover_tip(
                    "画像上をクリックしたピクセルの色相を、対象色相として設定します。",
                );
            });
            ui.horizontal_wrapped(|ui| {
                if preset_button(ui, "対象: 赤") {
                    params.target_hue_degrees = 0.0;
                    changed = true;
                }
                if preset_button(ui, "対象: 肌") {
                    params.target_hue_degrees = 28.0;
                    changed = true;
                }
                if preset_button(ui, "対象: 黄") {
                    params.target_hue_degrees = 58.0;
                    changed = true;
                }
                if preset_button(ui, "対象: 緑") {
                    params.target_hue_degrees = 120.0;
                    changed = true;
                }
                if preset_button(ui, "対象: 空") {
                    params.target_hue_degrees = 205.0;
                    changed = true;
                }
                if preset_button(ui, "対象: 青") {
                    params.target_hue_degrees = 235.0;
                    changed = true;
                }
                if preset_button(ui, "対象: 紫") {
                    params.target_hue_degrees = 285.0;
                    changed = true;
                }
            });
            let target = ui.add(
                egui::Slider::new(&mut params.target_hue_degrees, 0.0..=360.0)
                    .text("対象色相")
                    .suffix("°"),
            );
            changed |= target.changed();
            target.lab_hover_tip(
                "補正したい色の中心です。赤は0°、黄は60°、緑は120°、青は240°付近です。",
            );
            let range =
                ui.add(egui::Slider::new(&mut params.range_degrees, 1.0..=90.0).text("範囲"));
            changed |= range.changed();
            range.lab_hover_tip("この角度以内の色は強く補正します。小さいほど一点狙いになります。");
            let feather =
                ui.add(egui::Slider::new(&mut params.feather_degrees, 0.0..=90.0).text("ぼかし"));
            changed |= feather.changed();
            feather.lab_hover_tip("範囲の外側へ、どれだけなだらかに効果を弱めるかです。");
            let hue = ui.add(
                egui::Slider::new(&mut params.hue_degrees, -180.0..=180.0)
                    .text("色相補正")
                    .suffix("°"),
            );
            changed |= hue.changed();
            hue.lab_hover_tip("対象色だけ色相をずらします。");
            let saturation =
                ui.add(egui::Slider::new(&mut params.saturation, -100.0..=100.0).text("彩度"));
            changed |= saturation.changed();
            saturation.lab_hover_tip("対象色だけ鮮やかさを増減します。");
            let lightness =
                ui.add(egui::Slider::new(&mut params.lightness, -100.0..=100.0).text("明度"));
            changed |= lightness.changed();
            lightness.lab_hover_tip("対象色だけ明るさを増減します。");
        }
        LocalEffect::ChannelMixer(params) => {
            ui.label(egui::RichText::new("プリセット").color(Color32::from_gray(190)));
            ui.horizontal_wrapped(|ui| {
                if preset_button(ui, "白黒標準") {
                    *params = ChannelMixerParams {
                        monochrome: true,
                        mono_output: [30.0, 59.0, 11.0],
                        ..Default::default()
                    };
                    changed = true;
                }
                if preset_button(ui, "赤フィルター") {
                    *params = ChannelMixerParams {
                        monochrome: true,
                        mono_output: [75.0, 25.0, 0.0],
                        ..Default::default()
                    };
                    changed = true;
                }
                if preset_button(ui, "緑フィルター") {
                    *params = ChannelMixerParams {
                        monochrome: true,
                        mono_output: [15.0, 75.0, 10.0],
                        ..Default::default()
                    };
                    changed = true;
                }
                if preset_button(ui, "青フィルター") {
                    *params = ChannelMixerParams {
                        monochrome: true,
                        mono_output: [5.0, 35.0, 60.0],
                        ..Default::default()
                    };
                    changed = true;
                }
                if preset_button(ui, "赤青入替") {
                    *params = ChannelMixerParams {
                        red_output: [0.0, 0.0, 100.0],
                        green_output: [0.0, 100.0, 0.0],
                        blue_output: [100.0, 0.0, 0.0],
                        ..Default::default()
                    };
                    changed = true;
                }
                if preset_button(ui, "暖色ブースト") {
                    *params = ChannelMixerParams {
                        red_output: [115.0, 8.0, 0.0],
                        green_output: [0.0, 100.0, 0.0],
                        blue_output: [0.0, 0.0, 82.0],
                        ..Default::default()
                    };
                    changed = true;
                }
            });
            ui.label(
                egui::RichText::new(
                    "白黒化では元画像の赤/緑/青をどれだけ明度へ混ぜるかを調整します。カラー時は各出力チャンネルの混合率を直接編集します。",
                )
                .size(10.0)
                .color(Color32::from_gray(170)),
            );
            let mono = ui.checkbox(&mut params.monochrome, "白黒化");
            changed |= mono.changed();
            mono.lab_hover_tip("オンにすると、赤/緑/青の寄与率から1枚のグレー画像を作ります。");
            if params.monochrome {
                ui.collapsing("白黒の寄与率", |ui| {
                    changed |= draw_channel_coeff_sliders(ui, &mut params.mono_output);
                });
            } else {
                ui.collapsing("赤出力", |ui| {
                    changed |= draw_channel_coeff_sliders(ui, &mut params.red_output);
                });
                ui.collapsing("緑出力", |ui| {
                    changed |= draw_channel_coeff_sliders(ui, &mut params.green_output);
                });
                ui.collapsing("青出力", |ui| {
                    changed |= draw_channel_coeff_sliders(ui, &mut params.blue_output);
                });
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
        LocalEffect::Texture(params) => {
            ui.label(egui::RichText::new("プリセット").color(Color32::from_gray(190)));
            ui.horizontal_wrapped(|ui| {
                if preset_button(ui, "質感+") {
                    *params = TextureParams {
                        amount: 0.45,
                        radius_px: 10.0,
                    };
                    changed = true;
                }
                if preset_button(ui, "質感-") {
                    *params = TextureParams {
                        amount: -0.45,
                        radius_px: 10.0,
                    };
                    changed = true;
                }
                if preset_button(ui, "塗り面なめらか") {
                    *params = TextureParams {
                        amount: -0.65,
                        radius_px: 7.0,
                    };
                    changed = true;
                }
            });
            let amount = ui.add(egui::Slider::new(&mut params.amount, -1.0..=1.0).text("量"));
            changed |= amount.changed();
            amount.lab_hover_tip("正で中くらいの細部を強め、負でざらつきを抑えます。");
            let radius = ui.add(egui::Slider::new(&mut params.radius_px, 2.0..=40.0).text("半径"));
            changed |= radius.changed();
            radius.lab_hover_tip("拾う質感の大きさです。大きい値ほど広めの凹凸を対象にします。");
        }
        LocalEffect::HighPass(params) => {
            ui.label(egui::RichText::new("プリセット").color(Color32::from_gray(190)));
            ui.horizontal_wrapped(|ui| {
                if preset_button(ui, "弱く") {
                    *params = HighPassParams {
                        amount: 0.45,
                        radius_px: 8.0,
                        contrast: 1.0,
                        detail_only: false,
                    };
                    changed = true;
                }
                if preset_button(ui, "くっきり") {
                    *params = HighPassParams {
                        amount: 0.85,
                        radius_px: 6.0,
                        contrast: 1.2,
                        detail_only: false,
                    };
                    changed = true;
                }
                if preset_button(ui, "線/細部") {
                    *params = HighPassParams {
                        amount: 1.2,
                        radius_px: 3.0,
                        contrast: 1.6,
                        detail_only: false,
                    };
                    changed = true;
                }
                if preset_button(ui, "抽出表示") {
                    *params = HighPassParams {
                        amount: 0.0,
                        radius_px: 8.0,
                        contrast: 1.4,
                        detail_only: true,
                    };
                    changed = true;
                }
            });
            let detail_only = ui.checkbox(&mut params.detail_only, "抽出だけ表示");
            changed |= detail_only.changed();
            detail_only.lab_hover_tip(
                "ONにすると、元画像に合成せず中間グレー上のディテール抽出結果を表示します。",
            );
            let amount = ui.add_enabled(
                !params.detail_only,
                egui::Slider::new(&mut params.amount, 0.0..=2.0).text("量"),
            );
            changed |= amount.changed();
            amount.lab_hover_tip("ハイパス抽出をオーバーレイ合成する強さです。");
            let radius = ui.add(egui::Slider::new(&mut params.radius_px, 1.0..=60.0).text("半径"));
            changed |= radius.changed();
            radius.lab_hover_tip("大きい値ほど広い輪郭、小さい値ほど細部を抽出します。");
            let contrast = ui
                .add(egui::Slider::new(&mut params.contrast, 0.25..=4.0).text("抽出コントラスト"));
            changed |= contrast.changed();
            contrast.lab_hover_tip("抽出したディテールを中間グレーからどれだけ離すかです。");
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
        LocalEffect::MotionBlur(params) => {
            ui.label(egui::RichText::new("プリセット").color(Color32::from_gray(190)));
            ui.horizontal_wrapped(|ui| {
                if preset_button(ui, "横") {
                    *params = MotionBlurParams {
                        distance_px: 24.0,
                        angle_degrees: 0.0,
                        strength: 1.0,
                    };
                    changed = true;
                }
                if preset_button(ui, "縦") {
                    *params = MotionBlurParams {
                        distance_px: 24.0,
                        angle_degrees: 90.0,
                        strength: 1.0,
                    };
                    changed = true;
                }
                if preset_button(ui, "斜め") {
                    *params = MotionBlurParams {
                        distance_px: 30.0,
                        angle_degrees: -35.0,
                        strength: 0.9,
                    };
                    changed = true;
                }
                if preset_button(ui, "高速感") {
                    *params = MotionBlurParams {
                        distance_px: 56.0,
                        angle_degrees: 0.0,
                        strength: 0.85,
                    };
                    changed = true;
                }
            });
            ui.label(
                egui::RichText::new(
                    "指定方向へ画像を流すぼかしです。背景やエフェクトに部分適用すると動きの表現に使えます。",
                )
                .size(10.0)
                .color(Color32::from_gray(170)),
            );
            let distance =
                ui.add(egui::Slider::new(&mut params.distance_px, 0.0..=160.0).text("距離"));
            changed |= distance.changed();
            distance
                .lab_hover_tip("ぼかしを伸ばす長さです。値を大きくすると流れる幅が長くなります。");
            let angle = ui.add(
                egui::Slider::new(&mut params.angle_degrees, -180.0..=180.0)
                    .text("角度")
                    .suffix("°"),
            );
            changed |= angle.changed();
            angle.lab_hover_tip("ぼかしの方向です。0°で横方向、90°で縦方向になります。");
            let strength = ui.add(egui::Slider::new(&mut params.strength, 0.0..=1.0).text("強さ"));
            changed |= strength.changed();
            strength.lab_hover_tip("元画像から移動ぼかし結果へどれだけ近づけるかです。");
        }
        LocalEffect::Wind(params) => {
            ui.label(egui::RichText::new("プリセット").color(Color32::from_gray(190)));
            ui.horizontal_wrapped(|ui| {
                if preset_button(ui, "右へ") {
                    *params = WindParams {
                        direction: WindDirection::Right,
                        source: WindSource::Bright,
                        distance_px: 34.0,
                        threshold: 0.42,
                        softness: 0.16,
                        turbulence: 0.08,
                        strength: 0.85,
                        seed: params.seed,
                    };
                    changed = true;
                }
                if preset_button(ui, "左へ") {
                    *params = WindParams {
                        direction: WindDirection::Left,
                        source: WindSource::Bright,
                        distance_px: 34.0,
                        threshold: 0.42,
                        softness: 0.16,
                        turbulence: 0.08,
                        strength: 0.85,
                        seed: params.seed,
                    };
                    changed = true;
                }
                if preset_button(ui, "強風") {
                    *params = WindParams {
                        direction: WindDirection::Right,
                        source: WindSource::Edge,
                        distance_px: 62.0,
                        threshold: 0.18,
                        softness: 0.14,
                        turbulence: 0.22,
                        strength: 0.95,
                        seed: params.seed,
                    };
                    changed = true;
                }
                if preset_button(ui, "暗線") {
                    *params = WindParams {
                        direction: WindDirection::Right,
                        source: WindSource::Dark,
                        distance_px: 42.0,
                        threshold: 0.46,
                        softness: 0.12,
                        turbulence: 0.04,
                        strength: 0.8,
                        seed: params.seed,
                    };
                    changed = true;
                }
            });
            ui.label(
                egui::RichText::new(
                    "指定した起点を片方向へ引きずります。明部は光の尾、暗部は漫画的な暗線、輪郭は速度感の強い流線に向いています。",
                )
                .size(10.0)
                .color(Color32::from_gray(170)),
            );
            ui.horizontal_wrapped(|ui| {
                let right = params.direction == WindDirection::Right;
                if ui.selectable_label(right, "右へ").clicked() && !right {
                    params.direction = WindDirection::Right;
                    changed = true;
                }
                let left = params.direction == WindDirection::Left;
                if ui.selectable_label(left, "左へ").clicked() && !left {
                    params.direction = WindDirection::Left;
                    changed = true;
                }
                let down = params.direction == WindDirection::Down;
                if ui.selectable_label(down, "下へ").clicked() && !down {
                    params.direction = WindDirection::Down;
                    changed = true;
                }
                let up = params.direction == WindDirection::Up;
                if ui.selectable_label(up, "上へ").clicked() && !up {
                    params.direction = WindDirection::Up;
                    changed = true;
                }
            });
            ui.horizontal_wrapped(|ui| {
                let bright = params.source == WindSource::Bright;
                if ui.selectable_label(bright, "明部").clicked() && !bright {
                    params.source = WindSource::Bright;
                    changed = true;
                }
                let dark = params.source == WindSource::Dark;
                if ui.selectable_label(dark, "暗部").clicked() && !dark {
                    params.source = WindSource::Dark;
                    changed = true;
                }
                let edge = params.source == WindSource::Edge;
                if ui.selectable_label(edge, "輪郭").clicked() && !edge {
                    params.source = WindSource::Edge;
                    changed = true;
                }
            });
            let distance = ui.add(
                egui::Slider::new(&mut params.distance_px, 0.0..=160.0)
                    .text("距離")
                    .suffix("px"),
            );
            changed |= distance.changed();
            distance.lab_hover_tip("流線を伸ばす長さです。");
            let threshold =
                ui.add(egui::Slider::new(&mut params.threshold, 0.0..=1.0).text("しきい値"));
            changed |= threshold.changed();
            threshold.lab_hover_tip("流線の起点として拾う明るさ・暗さ・輪郭の強さです。");
            let softness =
                ui.add(egui::Slider::new(&mut params.softness, 0.001..=0.5).text("柔らかさ"));
            changed |= softness.changed();
            softness.lab_hover_tip("しきい値付近の起点をどれだけなだらかに拾うかです。");
            let turbulence =
                ui.add(egui::Slider::new(&mut params.turbulence, 0.0..=1.0).text("乱れ"));
            changed |= turbulence.changed();
            turbulence.lab_hover_tip("流線の横揺れです。上げるほど風のムラが出ます。");
            let strength = ui.add(egui::Slider::new(&mut params.strength, 0.0..=1.0).text("強さ"));
            changed |= strength.changed();
            strength.lab_hover_tip("元画像から風/スピード結果へどれだけ近づけるかです。");
            let mut seed = params.seed as i32;
            let seed_response = ui.add(egui::Slider::new(&mut seed, 0..=9999).text("seed"));
            changed |= seed_response.changed();
            seed_response.lab_hover_tip("乱れのパターンを変えます。");
            params.seed = seed.max(0) as u32;
        }
        LocalEffect::SpeedLines(params) => {
            ui.label(egui::RichText::new("プリセット").color(Color32::from_gray(190)));
            ui.horizontal_wrapped(|ui| {
                if preset_button(ui, "白集中") {
                    *params = SpeedLinesParams {
                        mode: SpeedLinesMode::Radial,
                        center: [0.5, 0.5],
                        angle_degrees: 0.0,
                        line_count: 96,
                        line_width_px: 2.4,
                        length: 0.92,
                        inner_radius: 0.18,
                        outer_radius: 1.0,
                        softness: 0.25,
                        strength: 0.82,
                        color_rgb: [255, 255, 255],
                        seed: params.seed,
                    };
                    changed = true;
                }
                if preset_button(ui, "黒集中") {
                    *params = SpeedLinesParams {
                        mode: SpeedLinesMode::Radial,
                        center: [0.5, 0.5],
                        angle_degrees: 0.0,
                        line_count: 72,
                        line_width_px: 2.0,
                        length: 0.86,
                        inner_radius: 0.22,
                        outer_radius: 1.0,
                        softness: 0.18,
                        strength: 0.78,
                        color_rgb: [0, 0, 0],
                        seed: params.seed,
                    };
                    changed = true;
                }
                if preset_button(ui, "横流れ") {
                    *params = SpeedLinesParams {
                        mode: SpeedLinesMode::Parallel,
                        center: [0.5, 0.5],
                        angle_degrees: 0.0,
                        line_count: 44,
                        line_width_px: 2.2,
                        length: 0.90,
                        inner_radius: 0.08,
                        outer_radius: 1.0,
                        softness: 0.30,
                        strength: 0.68,
                        color_rgb: [255, 255, 255],
                        seed: params.seed,
                    };
                    changed = true;
                }
                if preset_button(ui, "斜め流れ") {
                    *params = SpeedLinesParams {
                        mode: SpeedLinesMode::Parallel,
                        center: [0.5, 0.5],
                        angle_degrees: -28.0,
                        line_count: 58,
                        line_width_px: 1.8,
                        length: 0.72,
                        inner_radius: 0.04,
                        outer_radius: 1.0,
                        softness: 0.22,
                        strength: 0.74,
                        color_rgb: [255, 255, 255],
                        seed: params.seed,
                    };
                    changed = true;
                }
            });
            ui.label(
                egui::RichText::new(
                    "放射状の集中線、または指定方向へ流れる平行スピード線を自動生成します。",
                )
                .size(10.0)
                .color(Color32::from_gray(170)),
            );
            ui.horizontal_wrapped(|ui| {
                let radial = params.mode == SpeedLinesMode::Radial;
                if ui.selectable_label(radial, "放射").clicked() && !radial {
                    params.mode = SpeedLinesMode::Radial;
                    changed = true;
                }
                let parallel = params.mode == SpeedLinesMode::Parallel;
                if ui.selectable_label(parallel, "平行").clicked() && !parallel {
                    params.mode = SpeedLinesMode::Parallel;
                    changed = true;
                }
            });
            changed |= draw_effect_center_controls(
                ui,
                &mut params.center,
                "放射では集中点、平行では線の基準位置です。",
                "放射では集中点、平行では線の基準位置です。",
                effect_position_handles_visible,
                &mut set_effect_position_handles_visible,
            );
            if params.mode != SpeedLinesMode::Radial {
                let angle = ui.add(
                    egui::Slider::new(&mut params.angle_degrees, -180.0..=180.0)
                        .text("角度")
                        .suffix("°"),
                );
                changed |= angle.changed();
                angle.lab_hover_tip("スピード線が流れる方向です。0°で横方向、90°で縦方向です。");
            }
            let mut line_count = params.line_count as i32;
            let line_count_response =
                ui.add(egui::Slider::new(&mut line_count, 4..=240).text("線数"));
            changed |= line_count_response.changed();
            line_count_response.lab_hover_tip("生成する線の本数です。");
            params.line_count = line_count.clamp(4, 240) as u32;
            let line_width =
                ui.add(egui::Slider::new(&mut params.line_width_px, 0.25..=24.0).text("線幅"));
            changed |= line_width.changed();
            line_width.lab_hover_tip("1本あたりの太さです。");
            let length = ui.add(egui::Slider::new(&mut params.length, 0.05..=1.0).text("線長"));
            changed |= length.changed();
            length.lab_hover_tip("線をどれだけ長く伸ばすかです。");
            let inner =
                ui.add(egui::Slider::new(&mut params.inner_radius, 0.0..=0.98).text("中心抜き"));
            changed |= inner.changed();
            inner.lab_hover_tip("放射では中央の空白、平行では中央付近の弱まりを調整します。");
            let outer =
                ui.add(egui::Slider::new(&mut params.outer_radius, 0.02..=1.0).text("外側範囲"));
            changed |= outer.changed();
            outer.lab_hover_tip("線が出る外側の範囲です。");
            if params.outer_radius < params.inner_radius {
                params.outer_radius = (params.inner_radius + 0.02).min(1.0);
            }
            let softness =
                ui.add(egui::Slider::new(&mut params.softness, 0.0..=1.0).text("柔らかさ"));
            changed |= softness.changed();
            softness.lab_hover_tip("線の縁をぼかします。");
            let strength = ui.add(egui::Slider::new(&mut params.strength, 0.0..=1.0).text("強さ"));
            changed |= strength.changed();
            strength.lab_hover_tip("元画像から線色へどれだけ近づけるかです。");
            merge_rgb_color_response(
                draw_rgb_color_control(
                    ui,
                    "線色",
                    &mut params.color_rgb,
                    RgbPickTarget::SpeedLinesColor,
                    rgb_pick_active,
                ),
                &mut changed,
                &mut start_rgb_pick,
                &mut cancel_rgb_pick,
            );
            let mut seed = params.seed as i32;
            let seed_response = ui.add(egui::Slider::new(&mut seed, 0..=9999).text("seed"));
            changed |= seed_response.changed();
            seed_response.lab_hover_tip("線のばらつきパターンを変えます。");
            params.seed = seed.max(0) as u32;
        }
        LocalEffect::TiltShift(params) => {
            if !params.range_initialized && !params.mode_selected {
                params.mode_selected = true;
            }
            ui.label(egui::RichText::new("プリセット").color(Color32::from_gray(190)));
            ui.horizontal_wrapped(|ui| {
                if preset_button(ui, "奥ぼかし") {
                    *params = TiltShiftParams {
                        mode: TiltShiftMode::Linear,
                        mode_selected: true,
                        range_initialized: true,
                        center: [0.5, 0.58],
                        angle_degrees: -90.0,
                        focus_width: 0.10,
                        falloff: 0.34,
                        radius: [0.32, 0.32],
                        max_radius_px: 24.0,
                        strength: 1.0,
                        far_only: true,
                    };
                    changed = true;
                }
                if preset_button(ui, "ミニチュア") {
                    *params = TiltShiftParams {
                        mode: TiltShiftMode::Linear,
                        mode_selected: true,
                        range_initialized: true,
                        center: [0.5, 0.52],
                        angle_degrees: -90.0,
                        focus_width: 0.08,
                        falloff: 0.22,
                        radius: [0.32, 0.32],
                        max_radius_px: 34.0,
                        strength: 1.0,
                        far_only: false,
                    };
                    changed = true;
                }
                if preset_button(ui, "円形") {
                    *params = TiltShiftParams {
                        mode: TiltShiftMode::Radial,
                        mode_selected: true,
                        range_initialized: true,
                        center: [0.5, 0.5],
                        angle_degrees: -90.0,
                        focus_width: 0.12,
                        falloff: 0.34,
                        radius: [0.32, 0.32],
                        max_radius_px: 28.0,
                        strength: 1.0,
                        far_only: false,
                    };
                    changed = true;
                }
                if preset_button(ui, "斜め") {
                    *params = TiltShiftParams {
                        mode: TiltShiftMode::Linear,
                        mode_selected: true,
                        range_initialized: true,
                        center: [0.5, 0.5],
                        angle_degrees: -35.0,
                        focus_width: 0.10,
                        falloff: 0.28,
                        radius: [0.32, 0.32],
                        max_radius_px: 26.0,
                        strength: 0.9,
                        far_only: false,
                    };
                    changed = true;
                }
            });
            ui.label(
                egui::RichText::new(
                    "焦点帯または焦点円を残し、外側だけをぼかします。背景だけに使う場合は線形の奥ぼかしから試してください。",
                )
                .size(10.0)
                .color(Color32::from_gray(170)),
            );
            ui.horizontal_wrapped(|ui| {
                let linear_create_active = !params.range_initialized
                    && params.mode_selected
                    && params.mode == TiltShiftMode::Linear;
                if ui
                    .selectable_label(linear_create_active, "線形範囲を作成")
                    .clicked()
                {
                    params.mode = TiltShiftMode::Linear;
                    params.mode_selected = true;
                    params.range_initialized = false;
                    changed = true;
                }
                let radial_create_active = !params.range_initialized
                    && params.mode_selected
                    && params.mode == TiltShiftMode::Radial;
                if ui
                    .selectable_label(radial_create_active, "円形範囲を作成")
                    .clicked()
                {
                    params.mode = TiltShiftMode::Radial;
                    params.mode_selected = true;
                    params.range_initialized = false;
                    changed = true;
                }
                if ui.button("範囲クリア").clicked() {
                    params.range_initialized = false;
                    params.mode_selected = true;
                    changed = true;
                }
            });
            if params.range_initialized {
                let center_x =
                    ui.add(egui::Slider::new(&mut params.center[0], 0.0..=1.0).text("中心 X"));
                changed |= center_x.changed();
                center_x.lab_hover_tip("焦点帯または焦点円の中心位置です。");
                let center_y =
                    ui.add(egui::Slider::new(&mut params.center[1], 0.0..=1.0).text("中心 Y"));
                changed |= center_y.changed();
                center_y.lab_hover_tip("焦点帯または焦点円の中心位置です。");
                if params.mode == TiltShiftMode::Linear {
                    let angle = ui.add(
                        egui::Slider::new(&mut params.angle_degrees, -180.0..=180.0)
                            .text("奥行き方向")
                            .suffix("°"),
                    );
                    changed |= angle.changed();
                    angle.lab_hover_tip("ぼかしが強くなる方向です。-90°は上側を奥として扱います。");
                    let far_only = ui.checkbox(&mut params.far_only, "奥だけぼかす");
                    changed |= far_only.changed();
                    far_only.lab_hover_tip("ONにすると、焦点帯より奥側だけをぼかします。OFFでは手前と奥の両側をぼかします。");
                    let focus_width = ui
                        .add(egui::Slider::new(&mut params.focus_width, 0.0..=0.5).text("焦点幅"));
                    changed |= focus_width.changed();
                    focus_width.lab_hover_tip("線形モードで、シャープに残す帯の幅です。");
                } else {
                    let rx = ui
                        .add(egui::Slider::new(&mut params.radius[0], 0.02..=1.0).text("焦点 横"));
                    changed |= rx.changed();
                    rx.lab_hover_tip("円形モードで、シャープに残す範囲の横半径です。");
                    let ry = ui
                        .add(egui::Slider::new(&mut params.radius[1], 0.02..=1.0).text("焦点 縦"));
                    changed |= ry.changed();
                    ry.lab_hover_tip("円形モードで、シャープに残す範囲の縦半径です。");
                }
                let falloff =
                    ui.add(egui::Slider::new(&mut params.falloff, 0.02..=1.0).text("ぼかし境界"));
                changed |= falloff.changed();
                falloff
                    .lab_hover_tip("焦点範囲の外側で、ぼかしがどれだけなだらかに強くなるかです。");
            } else {
                ui.label(
                    egui::RichText::new(
                        "範囲未設定です。アクティブな作成ボタンの形で、画像上をドラッグして範囲を作成します。",
                    )
                    .size(10.0)
                    .color(Color32::from_gray(190)),
                );
            }
            let max_radius =
                ui.add(egui::Slider::new(&mut params.max_radius_px, 0.0..=80.0).text("最大半径"));
            changed |= max_radius.changed();
            max_radius.lab_hover_tip("最もぼける場所で使うぼかし半径です。");
            let strength = ui.add(egui::Slider::new(&mut params.strength, 0.0..=1.0).text("強さ"));
            changed |= strength.changed();
            strength.lab_hover_tip("元画像からチルトシフト結果へどれだけ近づけるかです。");
        }
        LocalEffect::LensBlur(params) => {
            ui.label(egui::RichText::new("プリセット").color(Color32::from_gray(190)));
            ui.horizontal_wrapped(|ui| {
                if preset_button(ui, "弱") {
                    *params = LensBlurParams {
                        radius_px: 10.0,
                        aperture: LensBlurAperture::Circular,
                        rotation_degrees: 0.0,
                        highlight_threshold: 0.94,
                        highlight_boost: 0.3,
                        strength: 0.55,
                    };
                    changed = true;
                }
                if preset_button(ui, "背景ぼかし") {
                    *params = LensBlurParams {
                        radius_px: 24.0,
                        aperture: LensBlurAperture::Circular,
                        rotation_degrees: 0.0,
                        highlight_threshold: 0.94,
                        highlight_boost: 0.5,
                        strength: 0.9,
                    };
                    changed = true;
                }
                if preset_button(ui, "玉ボケ") {
                    *params = LensBlurParams {
                        radius_px: 34.0,
                        aperture: LensBlurAperture::Circular,
                        rotation_degrees: 0.0,
                        highlight_threshold: 0.86,
                        highlight_boost: 1.2,
                        strength: 1.0,
                    };
                    changed = true;
                }
                if preset_button(ui, "6角光") {
                    *params = LensBlurParams {
                        radius_px: 32.0,
                        aperture: LensBlurAperture::Hexagon,
                        rotation_degrees: 30.0,
                        highlight_threshold: 0.88,
                        highlight_boost: 1.0,
                        strength: 1.0,
                    };
                    changed = true;
                }
            });
            ui.label(
                egui::RichText::new(
                    "絞り形状で画像を集めるぼかしです。明るい点がある背景に使うと、通常のぼかしよりレンズらしい玉ボケになります。",
                )
                .size(10.0)
                .color(Color32::from_gray(170)),
            );
            ui.horizontal(|ui| {
                let circular = params.aperture == LensBlurAperture::Circular;
                if ui.selectable_label(circular, "円形").clicked() && !circular {
                    params.aperture = LensBlurAperture::Circular;
                    changed = true;
                }
                let hexagon = params.aperture == LensBlurAperture::Hexagon;
                if ui.selectable_label(hexagon, "6角").clicked() && !hexagon {
                    params.aperture = LensBlurAperture::Hexagon;
                    changed = true;
                }
                let octagon = params.aperture == LensBlurAperture::Octagon;
                if ui.selectable_label(octagon, "8角").clicked() && !octagon {
                    params.aperture = LensBlurAperture::Octagon;
                    changed = true;
                }
            });
            let radius = ui.add(egui::Slider::new(&mut params.radius_px, 0.0..=64.0).text("半径"));
            changed |= radius.changed();
            radius.lab_hover_tip("ぼかしの大きさです。値を大きくすると玉ボケも大きくなります。");
            if params.aperture != LensBlurAperture::Circular {
                let rotation = ui.add(
                    egui::Slider::new(&mut params.rotation_degrees, -180.0..=180.0)
                        .text("絞り回転")
                        .suffix("°"),
                );
                changed |= rotation.changed();
                rotation.lab_hover_tip("6角・8角の絞り形状の向きを回転します。");
            }
            let threshold = ui.add(
                egui::Slider::new(&mut params.highlight_threshold, 0.50..=0.995)
                    .text("明部しきい値"),
            );
            changed |= threshold.changed();
            threshold.lab_hover_tip(
                "玉ボケとして膨らませる明るさのしきい値です。低いほど多くの明部が強調されます。",
            );
            let boost = ui.add(
                egui::Slider::new(&mut params.highlight_boost, 0.0..=3.0).text("明部ブースト"),
            );
            changed |= boost.changed();
            boost.lab_hover_tip("しきい値を超えた明るい点を、ぼかし内でどれだけ強く扱うかです。");
            let strength = ui.add(egui::Slider::new(&mut params.strength, 0.0..=1.0).text("強さ"));
            changed |= strength.changed();
            strength.lab_hover_tip("元画像からレンズぼかし結果へどれだけ近づけるかです。");
        }
        LocalEffect::RadialBlur(params) => {
            ui.label(egui::RichText::new("プリセット").color(Color32::from_gray(190)));
            ui.horizontal_wrapped(|ui| {
                if preset_button(ui, "ズーム弱") {
                    *params = RadialBlurParams {
                        mode: RadialBlurMode::Zoom,
                        center: [0.5, 0.5],
                        zoom_px: 28.0,
                        spin_degrees: 0.0,
                        samples: 21,
                        strength: 0.65,
                    };
                    changed = true;
                }
                if preset_button(ui, "集中") {
                    *params = RadialBlurParams {
                        mode: RadialBlurMode::Zoom,
                        center: [0.5, 0.5],
                        zoom_px: 78.0,
                        spin_degrees: 0.0,
                        samples: 33,
                        strength: 0.95,
                    };
                    changed = true;
                }
                if preset_button(ui, "回転") {
                    *params = RadialBlurParams {
                        mode: RadialBlurMode::Spin,
                        center: [0.5, 0.5],
                        zoom_px: 0.0,
                        spin_degrees: 24.0,
                        samples: 25,
                        strength: 0.85,
                    };
                    changed = true;
                }
                if preset_button(ui, "強回転") {
                    *params = RadialBlurParams {
                        mode: RadialBlurMode::Spin,
                        center: [0.5, 0.5],
                        zoom_px: 0.0,
                        spin_degrees: 64.0,
                        samples: 41,
                        strength: 0.9,
                    };
                    changed = true;
                }
            });
            ui.label(
                egui::RichText::new(
                    "中心から外へ伸びるズームぼかし、または中心周りに回るぼかしです。集中線的な速度感や渦巻き感に使えます。",
                )
                .size(10.0)
                .color(Color32::from_gray(170)),
            );
            ui.horizontal(|ui| {
                let zoom = params.mode == RadialBlurMode::Zoom;
                if ui.selectable_label(zoom, "ズーム").clicked() && !zoom {
                    params.mode = RadialBlurMode::Zoom;
                    changed = true;
                }
                let spin = params.mode == RadialBlurMode::Spin;
                if ui.selectable_label(spin, "回転").clicked() && !spin {
                    params.mode = RadialBlurMode::Spin;
                    changed = true;
                }
            });
            changed |= draw_effect_center_controls(
                ui,
                &mut params.center,
                "ぼかしの中心位置です。ズームでは集中点、回転では回転中心になります。",
                "ぼかしの中心位置です。",
                effect_position_handles_visible,
                &mut set_effect_position_handles_visible,
            );
            match params.mode {
                RadialBlurMode::Zoom => {
                    let zoom =
                        ui.add(egui::Slider::new(&mut params.zoom_px, 0.0..=160.0).text("距離"));
                    changed |= zoom.changed();
                    zoom.lab_hover_tip("画像の端でどれだけ外向きにサンプルを伸ばすかです。");
                }
                RadialBlurMode::Spin => {
                    let spin = ui.add(
                        egui::Slider::new(&mut params.spin_degrees, -180.0..=180.0)
                            .text("回転角")
                            .suffix("°"),
                    );
                    changed |= spin.changed();
                    spin.lab_hover_tip("画像の端でどれだけ回転方向にサンプルを広げるかです。符号で回転方向が変わります。");
                }
            }
            let samples = ui.add(egui::Slider::new(&mut params.samples, 3..=65).text("サンプル数"));
            changed |= samples.changed();
            samples.lab_hover_tip(
                "ぼかしの滑らかさです。大きいほど滑らかですが再合成は重くなります。",
            );
            let strength = ui.add(egui::Slider::new(&mut params.strength, 0.0..=1.0).text("強さ"));
            changed |= strength.changed();
            strength.lab_hover_tip("元画像から放射/回転ぼかし結果へどれだけ近づけるかです。");
        }
        LocalEffect::WaveDistortion(params) => {
            ui.label(egui::RichText::new("プリセット").color(Color32::from_gray(190)));
            ui.horizontal_wrapped(|ui| {
                if preset_button(ui, "水面横波") {
                    *params = WaveDistortionParams {
                        mode: WaveDistortionMode::Horizontal,
                        amplitude_px: 12.0,
                        wavelength_px: 72.0,
                        phase_degrees: 0.0,
                        center: [0.5, 0.5],
                        strength: 1.0,
                    };
                    changed = true;
                }
                if preset_button(ui, "縦ゆらぎ") {
                    *params = WaveDistortionParams {
                        mode: WaveDistortionMode::Vertical,
                        amplitude_px: 10.0,
                        wavelength_px: 64.0,
                        phase_degrees: 0.0,
                        center: [0.5, 0.5],
                        strength: 0.9,
                    };
                    changed = true;
                }
                if preset_button(ui, "さざ波") {
                    *params = WaveDistortionParams {
                        mode: WaveDistortionMode::Ripple,
                        amplitude_px: 8.0,
                        wavelength_px: 36.0,
                        phase_degrees: 0.0,
                        center: [0.5, 0.5],
                        strength: 0.85,
                    };
                    changed = true;
                }
                if preset_button(ui, "ジグザグ") {
                    *params = WaveDistortionParams {
                        mode: WaveDistortionMode::Zigzag,
                        amplitude_px: 14.0,
                        wavelength_px: 48.0,
                        phase_degrees: 0.0,
                        center: [0.5, 0.5],
                        strength: 1.0,
                    };
                    changed = true;
                }
            });
            ui.label(
                egui::RichText::new(
                    "画像を波の形にサンプルし直します。反射、水面、熱気、背景の揺らぎに使えます。",
                )
                .size(10.0)
                .color(Color32::from_gray(170)),
            );
            ui.horizontal_wrapped(|ui| {
                let horizontal = params.mode == WaveDistortionMode::Horizontal;
                if ui.selectable_label(horizontal, "横波").clicked() && !horizontal {
                    params.mode = WaveDistortionMode::Horizontal;
                    changed = true;
                }
                let vertical = params.mode == WaveDistortionMode::Vertical;
                if ui.selectable_label(vertical, "縦波").clicked() && !vertical {
                    params.mode = WaveDistortionMode::Vertical;
                    changed = true;
                }
                let ripple = params.mode == WaveDistortionMode::Ripple;
                if ui.selectable_label(ripple, "さざ波").clicked() && !ripple {
                    params.mode = WaveDistortionMode::Ripple;
                    changed = true;
                }
                let zigzag = params.mode == WaveDistortionMode::Zigzag;
                if ui.selectable_label(zigzag, "ジグザグ").clicked() && !zigzag {
                    params.mode = WaveDistortionMode::Zigzag;
                    changed = true;
                }
            });
            let amplitude =
                ui.add(egui::Slider::new(&mut params.amplitude_px, -80.0..=80.0).text("振幅"));
            changed |= amplitude.changed();
            amplitude.lab_hover_tip(
                "どれだけ大きく画素をずらすかです。符号を変えると揺れの向きが反転します。",
            );
            let wavelength =
                ui.add(egui::Slider::new(&mut params.wavelength_px, 4.0..=240.0).text("波長"));
            changed |= wavelength.changed();
            wavelength
                .lab_hover_tip("波の間隔です。小さい値ほど細かく、大きい値ほどゆったり揺れます。");
            let phase = ui.add(
                egui::Slider::new(&mut params.phase_degrees, -180.0..=180.0)
                    .text("位相")
                    .suffix("°"),
            );
            changed |= phase.changed();
            phase.lab_hover_tip(
                "波の開始位置をずらします。アニメーション用ではなく、静止画の位置合わせ用です。",
            );
            if params.mode == WaveDistortionMode::Ripple {
                changed |= draw_effect_center_controls(
                    ui,
                    &mut params.center,
                    "さざ波の中心位置です。",
                    "さざ波の中心位置です。",
                    effect_position_handles_visible,
                    &mut set_effect_position_handles_visible,
                );
            }
            let strength = ui.add(egui::Slider::new(&mut params.strength, 0.0..=1.0).text("強さ"));
            changed |= strength.changed();
            strength.lab_hover_tip("元画像からゆがみ結果へどれだけ近づけるかです。");
        }
        LocalEffect::PinchSpherize(params) => {
            ui.label(egui::RichText::new("プリセット").color(Color32::from_gray(190)));
            ui.horizontal_wrapped(|ui| {
                if preset_button(ui, "魚眼") {
                    *params = PinchSpherizeParams {
                        amount: 0.72,
                        radius_px: 0.0,
                        center: [0.5, 0.5],
                        strength: 1.0,
                    };
                    changed = true;
                }
                if preset_button(ui, "ふくらむ") {
                    *params = PinchSpherizeParams {
                        amount: 0.45,
                        radius_px: 260.0,
                        center: [0.5, 0.5],
                        strength: 0.9,
                    };
                    changed = true;
                }
                if preset_button(ui, "つまむ") {
                    *params = PinchSpherizeParams {
                        amount: -0.65,
                        radius_px: 260.0,
                        center: [0.5, 0.5],
                        strength: 1.0,
                    };
                    changed = true;
                }
                if preset_button(ui, "小顔/圧縮") {
                    *params = PinchSpherizeParams {
                        amount: -0.35,
                        radius_px: 180.0,
                        center: [0.5, 0.45],
                        strength: 0.75,
                    };
                    changed = true;
                }
            });
            ui.label(
                egui::RichText::new(
                    "中心からの距離を変えて、魚眼レンズのようなふくらみや、内側へつまむ変形を作ります。",
                )
                .size(10.0)
                .color(Color32::from_gray(170)),
            );
            let amount = ui.add(egui::Slider::new(&mut params.amount, -1.0..=1.0).text("変形量"));
            changed |= amount.changed();
            amount.lab_hover_tip("正で魚眼/ふくらみ、負で中心へつまむ変形になります。");
            let radius = ui.add(egui::Slider::new(&mut params.radius_px, 0.0..=800.0).text("半径"));
            changed |= radius.changed();
            radius.lab_hover_tip("効果の範囲です。0 のときは中心から画像の角までを使います。");
            changed |= draw_effect_center_controls(
                ui,
                &mut params.center,
                "変形の中心位置です。",
                "変形の中心位置です。",
                effect_position_handles_visible,
                &mut set_effect_position_handles_visible,
            );
            let strength = ui.add(egui::Slider::new(&mut params.strength, 0.0..=1.0).text("強さ"));
            changed |= strength.changed();
            strength.lab_hover_tip("元画像から変形結果へどれだけ近づけるかです。");
        }
        LocalEffect::Twirl(params) => {
            ui.label(egui::RichText::new("プリセット").color(Color32::from_gray(190)));
            ui.horizontal_wrapped(|ui| {
                if preset_button(ui, "渦弱") {
                    *params = TwirlParams {
                        angle_degrees: 120.0,
                        radius_px: 0.0,
                        center: [0.5, 0.5],
                        strength: 0.75,
                    };
                    changed = true;
                }
                if preset_button(ui, "渦強") {
                    *params = TwirlParams {
                        angle_degrees: 360.0,
                        radius_px: 0.0,
                        center: [0.5, 0.5],
                        strength: 1.0,
                    };
                    changed = true;
                }
                if preset_button(ui, "逆回転") {
                    *params = TwirlParams {
                        angle_degrees: -260.0,
                        radius_px: 0.0,
                        center: [0.5, 0.5],
                        strength: 0.95,
                    };
                    changed = true;
                }
                if preset_button(ui, "魔法陣") {
                    *params = TwirlParams {
                        angle_degrees: 540.0,
                        radius_px: 320.0,
                        center: [0.5, 0.5],
                        strength: 0.9,
                    };
                    changed = true;
                }
            });
            ui.label(
                egui::RichText::new(
                    "中心に近いほど強く回転させ、外側へ自然に弱まる渦巻き変形を作ります。",
                )
                .size(10.0)
                .color(Color32::from_gray(170)),
            );
            let angle = ui.add(
                egui::Slider::new(&mut params.angle_degrees, -720.0..=720.0)
                    .text("回転量")
                    .suffix("°"),
            );
            changed |= angle.changed();
            angle.lab_hover_tip("中心で最大になる回転量です。符号を変えると渦の向きが反転します。");
            let radius = ui.add(egui::Slider::new(&mut params.radius_px, 0.0..=800.0).text("半径"));
            changed |= radius.changed();
            radius.lab_hover_tip("効果の範囲です。0 のときは中心から画像の角までを使います。");
            changed |= draw_effect_center_controls(
                ui,
                &mut params.center,
                "渦巻きの中心位置です。",
                "渦巻きの中心位置です。",
                effect_position_handles_visible,
                &mut set_effect_position_handles_visible,
            );
            let strength = ui.add(egui::Slider::new(&mut params.strength, 0.0..=1.0).text("強さ"));
            changed |= strength.changed();
            strength.lab_hover_tip("元画像から渦巻き結果へどれだけ近づけるかです。");
        }
        LocalEffect::PolarCoordinates(params) => {
            ui.label(egui::RichText::new("プリセット").color(Color32::from_gray(190)));
            ui.horizontal_wrapped(|ui| {
                if preset_button(ui, "Tiny planet") {
                    *params = PolarCoordinatesParams {
                        mode: PolarCoordinatesMode::RectToPolar,
                        center: [0.5, 0.5],
                        radius_px: 0.0,
                        angle_offset_degrees: -90.0,
                        invert_radius: true,
                        strength: 1.0,
                    };
                    changed = true;
                }
                if preset_button(ui, "円形構図") {
                    *params = PolarCoordinatesParams {
                        mode: PolarCoordinatesMode::RectToPolar,
                        center: [0.5, 0.5],
                        radius_px: 0.0,
                        angle_offset_degrees: 0.0,
                        invert_radius: false,
                        strength: 1.0,
                    };
                    changed = true;
                }
                if preset_button(ui, "パノラマ展開") {
                    *params = PolarCoordinatesParams {
                        mode: PolarCoordinatesMode::PolarToRect,
                        center: [0.5, 0.5],
                        radius_px: 0.0,
                        angle_offset_degrees: 0.0,
                        invert_radius: false,
                        strength: 1.0,
                    };
                    changed = true;
                }
                if preset_button(ui, "内外反転") {
                    params.invert_radius = !params.invert_radius;
                    changed = true;
                }
            });
            ui.label(
                egui::RichText::new(
                    "横方向を角度、縦方向を半径として扱い、画像を円形に巻いたり横長へ展開したりします。",
                )
                .size(10.0)
                .color(Color32::from_gray(170)),
            );
            ui.horizontal_wrapped(|ui| {
                let rect_to_polar = params.mode == PolarCoordinatesMode::RectToPolar;
                if ui.selectable_label(rect_to_polar, "矩形→円形").clicked() && !rect_to_polar
                {
                    params.mode = PolarCoordinatesMode::RectToPolar;
                    changed = true;
                }
                let polar_to_rect = params.mode == PolarCoordinatesMode::PolarToRect;
                if ui.selectable_label(polar_to_rect, "円形→矩形").clicked() && !polar_to_rect
                {
                    params.mode = PolarCoordinatesMode::PolarToRect;
                    changed = true;
                }
            });
            let invert = ui.checkbox(&mut params.invert_radius, "内外反転");
            changed |= invert.changed();
            invert.lab_hover_tip(
                "半径方向の対応を反転します。Tiny planet では地面側を中心へ寄せる用途に使います。",
            );
            let radius =
                ui.add(egui::Slider::new(&mut params.radius_px, 0.0..=1200.0).text("半径"));
            changed |= radius.changed();
            radius.lab_hover_tip(
                "円形変換に使う半径です。0 のときは中心から画像の角までを使います。",
            );
            let angle = ui.add(
                egui::Slider::new(&mut params.angle_offset_degrees, -180.0..=180.0)
                    .text("角度オフセット")
                    .suffix("°"),
            );
            changed |= angle.changed();
            angle.lab_hover_tip("巻き始めの角度を回します。継ぎ目や上方向の位置合わせに使います。");
            changed |= draw_effect_center_controls(
                ui,
                &mut params.center,
                "円形変換の中心位置です。",
                "円形変換の中心位置です。",
                effect_position_handles_visible,
                &mut set_effect_position_handles_visible,
            );
            let strength = ui.add(egui::Slider::new(&mut params.strength, 0.0..=1.0).text("強さ"));
            changed |= strength.changed();
            strength.lab_hover_tip("元画像から極座標変換結果へどれだけ近づけるかです。");
        }
        LocalEffect::GlassDisplacement(params) => {
            ui.label(egui::RichText::new("プリセット").color(Color32::from_gray(190)));
            ui.horizontal_wrapped(|ui| {
                if preset_button(ui, "すりガラス") {
                    *params = GlassDisplacementParams {
                        mode: GlassDisplacementMode::Frosted,
                        displacement_px: 7.0,
                        scale_px: 28.0,
                        detail: 0.7,
                        angle_degrees: 0.0,
                        seed: params.seed,
                        strength: 0.85,
                    };
                    changed = true;
                }
                if preset_button(ui, "水面ガラス") {
                    *params = GlassDisplacementParams {
                        mode: GlassDisplacementMode::Ripple,
                        displacement_px: 14.0,
                        scale_px: 64.0,
                        detail: 0.45,
                        angle_degrees: 0.0,
                        seed: params.seed,
                        strength: 0.9,
                    };
                    changed = true;
                }
                if preset_button(ui, "面ガラス") {
                    *params = GlassDisplacementParams {
                        mode: GlassDisplacementMode::Faceted,
                        displacement_px: 18.0,
                        scale_px: 46.0,
                        detail: 0.0,
                        angle_degrees: 0.0,
                        seed: params.seed,
                        strength: 1.0,
                    };
                    changed = true;
                }
                if preset_button(ui, "細かい歪み") {
                    *params = GlassDisplacementParams {
                        mode: GlassDisplacementMode::Frosted,
                        displacement_px: 4.0,
                        scale_px: 12.0,
                        detail: 1.0,
                        angle_degrees: 18.0,
                        seed: params.seed,
                        strength: 0.8,
                    };
                    changed = true;
                }
            });
            ui.label(
                egui::RichText::new(
                    "ノイズや波形を変位マップとして使い、元画像のサンプル位置をずらします。",
                )
                .size(10.0)
                .color(Color32::from_gray(170)),
            );
            ui.horizontal_wrapped(|ui| {
                let frosted = params.mode == GlassDisplacementMode::Frosted;
                if ui.selectable_label(frosted, "すりガラス").clicked() && !frosted {
                    params.mode = GlassDisplacementMode::Frosted;
                    changed = true;
                }
                let ripple = params.mode == GlassDisplacementMode::Ripple;
                if ui.selectable_label(ripple, "波ガラス").clicked() && !ripple {
                    params.mode = GlassDisplacementMode::Ripple;
                    changed = true;
                }
                let faceted = params.mode == GlassDisplacementMode::Faceted;
                if ui.selectable_label(faceted, "面ガラス").clicked() && !faceted {
                    params.mode = GlassDisplacementMode::Faceted;
                    changed = true;
                }
            });
            let displacement = ui.add(
                egui::Slider::new(&mut params.displacement_px, 0.0..=64.0)
                    .text("変位量")
                    .suffix("px"),
            );
            changed |= displacement.changed();
            displacement.lab_hover_tip("サンプル位置を最大でどれだけずらすかです。");
            let scale = ui
                .add(egui::Slider::new(&mut params.scale_px, 2.0..=240.0).text("テクスチャサイズ"));
            changed |= scale.changed();
            scale.lab_hover_tip(
                "変位マップの大きさです。小さいほど細かく、大きいほど大きく歪みます。",
            );
            let detail =
                ui.add(egui::Slider::new(&mut params.detail, 0.0..=1.0).text("ディテール"));
            changed |= detail.changed();
            detail.lab_hover_tip(
                "すりガラスでは細かいノイズ量、波ガラスでは交差方向の波量として働きます。",
            );
            let angle = ui.add(
                egui::Slider::new(&mut params.angle_degrees, -180.0..=180.0)
                    .text("角度")
                    .suffix("°"),
            );
            changed |= angle.changed();
            angle.lab_hover_tip(
                "変位マップの向きを回します。波や面ガラスの流れを合わせるときに使います。",
            );
            let mut seed = params.seed as i32;
            let seed_response = ui.add(egui::Slider::new(&mut seed, 0..=9999).text("seed"));
            changed |= seed_response.changed();
            seed_response.lab_hover_tip("ノイズや面ガラスの模様を変えます。");
            params.seed = seed.max(0) as u32;
            let strength = ui.add(egui::Slider::new(&mut params.strength, 0.0..=1.0).text("強さ"));
            changed |= strength.changed();
            strength.lab_hover_tip("元画像からガラス変位結果へどれだけ近づけるかです。");
        }
        LocalEffect::LensCorrection(params) => {
            ui.label(egui::RichText::new("プリセット").color(Color32::from_gray(190)));
            ui.horizontal_wrapped(|ui| {
                if preset_button(ui, "樽型補正") {
                    *params = LensCorrectionParams {
                        distortion: 0.35,
                        zoom: 0.06,
                        center: [0.5, 0.5],
                        vignette_correction: 0.0,
                        strength: 1.0,
                    };
                    changed = true;
                }
                if preset_button(ui, "糸巻き補正") {
                    *params = LensCorrectionParams {
                        distortion: -0.35,
                        zoom: 0.03,
                        center: [0.5, 0.5],
                        vignette_correction: 0.0,
                        strength: 1.0,
                    };
                    changed = true;
                }
                if preset_button(ui, "広角強め") {
                    *params = LensCorrectionParams {
                        distortion: 0.62,
                        zoom: 0.14,
                        center: [0.5, 0.5],
                        vignette_correction: 0.12,
                        strength: 1.0,
                    };
                    changed = true;
                }
                if preset_button(ui, "周辺減光補正") {
                    *params = LensCorrectionParams {
                        distortion: 0.0,
                        zoom: 0.0,
                        center: [0.5, 0.5],
                        vignette_correction: 0.48,
                        strength: 1.0,
                    };
                    changed = true;
                }
            });
            ui.label(
                egui::RichText::new(
                    "中心から外側へ向かうレンズ歪みを補正します。ズームは補正後の端の伸びやにじみを切るために使います。",
                )
                .size(10.0)
                .color(Color32::from_gray(170)),
            );
            let distortion =
                ui.add(egui::Slider::new(&mut params.distortion, -1.0..=1.0).text("歪み補正"));
            changed |= distortion.changed();
            distortion.lab_hover_tip("正で樽型歪みの補正、負で糸巻き歪みの補正です。");
            let vignette = ui.add(
                egui::Slider::new(&mut params.vignette_correction, -1.0..=1.0).text("周辺減光補正"),
            );
            changed |= vignette.changed();
            vignette.lab_hover_tip(
                "正で周辺を持ち上げ、負で周辺を落とします。写真補正では正側を使います。",
            );
            let zoom =
                ui.add(egui::Slider::new(&mut params.zoom, 0.0..=0.5).text("ズーム/切り抜き"));
            changed |= zoom.changed();
            zoom.lab_hover_tip("歪み補正で端が伸びるとき、少し拡大して端を切ります。");
            changed |= draw_effect_center_controls(
                ui,
                &mut params.center,
                "レンズ補正の中心位置です。",
                "レンズ補正の中心位置です。",
                effect_position_handles_visible,
                &mut set_effect_position_handles_visible,
            );
            let strength = ui.add(egui::Slider::new(&mut params.strength, 0.0..=1.0).text("強さ"));
            changed |= strength.changed();
            strength.lab_hover_tip("元画像からレンズ補正結果へどれだけ近づけるかです。");
        }
        LocalEffect::LineExtract(params) => {
            ui.label(egui::RichText::new("プリセット").color(Color32::from_gray(190)));
            ui.horizontal_wrapped(|ui| {
                if preset_button(ui, "白地黒線") {
                    *params = LineExtractParams {
                        mode: LineExtractMode::BlackOnWhite,
                        threshold: 0.18,
                        softness: 0.1,
                        thickness_px: 1.0,
                        strength: 1.0,
                    };
                    changed = true;
                }
                if preset_button(ui, "黒地白線") {
                    *params = LineExtractParams {
                        mode: LineExtractMode::WhiteOnBlack,
                        threshold: 0.18,
                        softness: 0.1,
                        thickness_px: 1.0,
                        strength: 1.0,
                    };
                    changed = true;
                }
                if preset_button(ui, "元画像に黒線") {
                    *params = LineExtractParams {
                        mode: LineExtractMode::DarkenOriginal,
                        threshold: 0.16,
                        softness: 0.12,
                        thickness_px: 1.0,
                        strength: 0.75,
                    };
                    changed = true;
                }
                if preset_button(ui, "太線") {
                    *params = LineExtractParams {
                        mode: LineExtractMode::BlackOnWhite,
                        threshold: 0.12,
                        softness: 0.08,
                        thickness_px: 2.0,
                        strength: 1.0,
                    };
                    changed = true;
                }
            });
            ui.label(
                egui::RichText::new(
                    "Sobel エッジから線を作ります。しきい値を下げるほど薄い差も線になり、柔らかさで境界をなじませます。",
                )
                .size(10.0)
                .color(Color32::from_gray(170)),
            );
            ui.horizontal_wrapped(|ui| {
                let black_on_white = params.mode == LineExtractMode::BlackOnWhite;
                if ui.selectable_label(black_on_white, "白地黒線").clicked() && !black_on_white
                {
                    params.mode = LineExtractMode::BlackOnWhite;
                    changed = true;
                }
                let white_on_black = params.mode == LineExtractMode::WhiteOnBlack;
                if ui.selectable_label(white_on_black, "黒地白線").clicked() && !white_on_black
                {
                    params.mode = LineExtractMode::WhiteOnBlack;
                    changed = true;
                }
                let darken_original = params.mode == LineExtractMode::DarkenOriginal;
                if ui
                    .selectable_label(darken_original, "元画像に黒線")
                    .clicked()
                    && !darken_original
                {
                    params.mode = LineExtractMode::DarkenOriginal;
                    changed = true;
                }
                let lighten_original = params.mode == LineExtractMode::LightenOriginal;
                if ui
                    .selectable_label(lighten_original, "元画像に白線")
                    .clicked()
                    && !lighten_original
                {
                    params.mode = LineExtractMode::LightenOriginal;
                    changed = true;
                }
            });
            let threshold =
                ui.add(egui::Slider::new(&mut params.threshold, 0.0..=1.0).text("しきい値"));
            changed |= threshold.changed();
            threshold
                .lab_hover_tip("線として拾うエッジの強さです。低いほど細かい差も線になります。");
            let softness =
                ui.add(egui::Slider::new(&mut params.softness, 0.001..=0.5).text("柔らかさ"));
            changed |= softness.changed();
            softness.lab_hover_tip("しきい値付近の線をどれだけなだらかに出すかです。");
            let thickness = ui.add(
                egui::Slider::new(&mut params.thickness_px, 1.0..=8.0)
                    .text("太さ")
                    .suffix("px"),
            );
            changed |= thickness.changed();
            thickness.lab_hover_tip("検出したエッジを周囲へ広げる量です。");
            let strength = ui.add(egui::Slider::new(&mut params.strength, 0.0..=1.0).text("強さ"));
            changed |= strength.changed();
            strength.lab_hover_tip("元画像から線画抽出結果へどれだけ近づけるかです。");
        }
        LocalEffect::ArtisticMedia(params) => {
            ui.label(egui::RichText::new("プリセット").color(Color32::from_gray(190)));
            ui.horizontal_wrapped(|ui| {
                if preset_button(ui, "水彩") {
                    *params = ArtisticMediaParams {
                        mode: ArtisticMediaMode::Watercolor,
                        radius_px: 5.0,
                        edge_strength: 0.35,
                        texture: 0.24,
                        color_amount: 0.85,
                        strength: 1.0,
                        seed: params.seed,
                    };
                    changed = true;
                }
                if preset_button(ui, "淡彩") {
                    *params = ArtisticMediaParams {
                        mode: ArtisticMediaMode::Watercolor,
                        radius_px: 8.0,
                        edge_strength: 0.18,
                        texture: 0.12,
                        color_amount: 0.55,
                        strength: 0.85,
                        seed: params.seed,
                    };
                    changed = true;
                }
                if preset_button(ui, "色鉛筆") {
                    *params = ArtisticMediaParams {
                        mode: ArtisticMediaMode::ColoredPencil,
                        radius_px: 2.0,
                        edge_strength: 0.55,
                        texture: 0.48,
                        color_amount: 0.95,
                        strength: 1.0,
                        seed: params.seed,
                    };
                    changed = true;
                }
                if preset_button(ui, "鉛筆画") {
                    *params = ArtisticMediaParams {
                        mode: ArtisticMediaMode::PencilSketch,
                        radius_px: 1.0,
                        edge_strength: 0.75,
                        texture: 0.55,
                        color_amount: 0.0,
                        strength: 1.0,
                        seed: params.seed,
                    };
                    changed = true;
                }
            });
            ui.label(
                egui::RichText::new(
                    "色をなじませ、輪郭と紙目を足して絵画調に寄せます。鉛筆画では色量を上げると淡い色付きになります。",
                )
                .size(10.0)
                .color(Color32::from_gray(170)),
            );
            ui.horizontal_wrapped(|ui| {
                let watercolor = params.mode == ArtisticMediaMode::Watercolor;
                if ui.selectable_label(watercolor, "水彩").clicked() && !watercolor {
                    params.mode = ArtisticMediaMode::Watercolor;
                    changed = true;
                }
                let colored_pencil = params.mode == ArtisticMediaMode::ColoredPencil;
                if ui.selectable_label(colored_pencil, "色鉛筆").clicked() && !colored_pencil {
                    params.mode = ArtisticMediaMode::ColoredPencil;
                    changed = true;
                }
                let pencil = params.mode == ArtisticMediaMode::PencilSketch;
                if ui.selectable_label(pencil, "鉛筆画").clicked() && !pencil {
                    params.mode = ArtisticMediaMode::PencilSketch;
                    changed = true;
                }
            });
            let radius = ui.add(
                egui::Slider::new(&mut params.radius_px, 0.0..=24.0)
                    .text("なじませ")
                    .suffix("px"),
            );
            changed |= radius.changed();
            radius.lab_hover_tip(
                "色を面としてなじませる量です。水彩では大きめ、鉛筆では小さめが向いています。",
            );
            let edge = ui.add(egui::Slider::new(&mut params.edge_strength, 0.0..=1.0).text("輪郭"));
            changed |= edge.changed();
            edge.lab_hover_tip("輪郭や筆致をどれだけ強調するかです。");
            let texture =
                ui.add(egui::Slider::new(&mut params.texture, 0.0..=1.0).text("紙目/筆致"));
            changed |= texture.changed();
            texture.lab_hover_tip("紙目ノイズや鉛筆のハッチング量です。");
            let color = ui.add(egui::Slider::new(&mut params.color_amount, 0.0..=1.0).text("色量"));
            changed |= color.changed();
            color.lab_hover_tip("色の残し方です。鉛筆画では 0 にすると白黒寄りになります。");
            let strength = ui.add(egui::Slider::new(&mut params.strength, 0.0..=1.0).text("強さ"));
            changed |= strength.changed();
            strength.lab_hover_tip("元画像から絵画調結果へどれだけ近づけるかです。");
            let mut seed = params.seed as i32;
            let seed_response = ui.add(egui::Slider::new(&mut seed, 0..=9999).text("seed"));
            changed |= seed_response.changed();
            seed_response.lab_hover_tip("紙目や鉛筆線のパターンを変えます。");
            params.seed = seed.max(0) as u32;
        }
        LocalEffect::BrushStroke(params) => {
            ui.label(egui::RichText::new("プリセット").color(Color32::from_gray(190)));
            ui.horizontal_wrapped(|ui| {
                if preset_button(ui, "ドライブラシ") {
                    *params = BrushStrokeParams {
                        mode: BrushStrokeMode::DryBrush,
                        length_px: 14.0,
                        radius_px: 1.0,
                        angle_degrees: -12.0,
                        texture: 0.72,
                        edge_strength: 0.45,
                        color_amount: 0.85,
                        strength: 1.0,
                        seed: params.seed,
                    };
                    changed = true;
                }
                if preset_button(ui, "塗料") {
                    *params = BrushStrokeParams {
                        mode: BrushStrokeMode::PaintDaubs,
                        length_px: 18.0,
                        radius_px: 3.0,
                        angle_degrees: -24.0,
                        texture: 0.34,
                        edge_strength: 0.28,
                        color_amount: 1.0,
                        strength: 0.92,
                        seed: params.seed,
                    };
                    changed = true;
                }
                if preset_button(ui, "ナイフ") {
                    *params = BrushStrokeParams {
                        mode: BrushStrokeMode::PaletteKnife,
                        length_px: 34.0,
                        radius_px: 2.0,
                        angle_degrees: 0.0,
                        texture: 0.48,
                        edge_strength: 0.55,
                        color_amount: 0.9,
                        strength: 1.0,
                        seed: params.seed,
                    };
                    changed = true;
                }
                if preset_button(ui, "斜め筆致") {
                    *params = BrushStrokeParams {
                        mode: BrushStrokeMode::DryBrush,
                        length_px: 24.0,
                        radius_px: 2.0,
                        angle_degrees: -38.0,
                        texture: 0.58,
                        edge_strength: 0.42,
                        color_amount: 0.9,
                        strength: 0.9,
                        seed: params.seed,
                    };
                    changed = true;
                }
            });
            ui.label(
                egui::RichText::new(
                    "方向のあるサンプルで色を引き、筆跡・厚塗り・ナイフ跡のテクスチャを重ねます。",
                )
                .size(10.0)
                .color(Color32::from_gray(170)),
            );
            ui.horizontal_wrapped(|ui| {
                let dry = params.mode == BrushStrokeMode::DryBrush;
                if ui.selectable_label(dry, "ドライ").clicked() && !dry {
                    params.mode = BrushStrokeMode::DryBrush;
                    changed = true;
                }
                let paint = params.mode == BrushStrokeMode::PaintDaubs;
                if ui.selectable_label(paint, "塗料").clicked() && !paint {
                    params.mode = BrushStrokeMode::PaintDaubs;
                    changed = true;
                }
                let knife = params.mode == BrushStrokeMode::PaletteKnife;
                if ui.selectable_label(knife, "ナイフ").clicked() && !knife {
                    params.mode = BrushStrokeMode::PaletteKnife;
                    changed = true;
                }
            });
            let length = ui.add(
                egui::Slider::new(&mut params.length_px, 0.0..=72.0)
                    .text("ストローク長")
                    .suffix("px"),
            );
            changed |= length.changed();
            length.lab_hover_tip("筆跡として色を引く長さです。");
            let radius = ui.add(
                egui::Slider::new(&mut params.radius_px, 0.0..=12.0)
                    .text("幅")
                    .suffix("px"),
            );
            changed |= radius.changed();
            radius.lab_hover_tip("筆跡の横方向の揺れや幅です。");
            let angle = ui.add(
                egui::Slider::new(&mut params.angle_degrees, -180.0..=180.0)
                    .text("角度")
                    .suffix("°"),
            );
            changed |= angle.changed();
            angle.lab_hover_tip("筆跡の方向です。");
            let texture = ui.add(egui::Slider::new(&mut params.texture, 0.0..=1.0).text("筆致"));
            changed |= texture.changed();
            texture.lab_hover_tip("ドライ感、塗料の凹凸、ナイフ跡の強さです。");
            let edge = ui.add(egui::Slider::new(&mut params.edge_strength, 0.0..=1.0).text("輪郭"));
            changed |= edge.changed();
            edge.lab_hover_tip("輪郭やストロークの硬さをどれだけ出すかです。");
            let color = ui.add(egui::Slider::new(&mut params.color_amount, 0.0..=1.0).text("色量"));
            changed |= color.changed();
            color.lab_hover_tip("元の色の鮮やかさをどれだけ残すかです。");
            let strength = ui.add(egui::Slider::new(&mut params.strength, 0.0..=1.0).text("強さ"));
            changed |= strength.changed();
            strength.lab_hover_tip("元画像から筆致結果へどれだけ近づけるかです。");
            let mut seed = params.seed as i32;
            let seed_response = ui.add(egui::Slider::new(&mut seed, 0..=9999).text("seed"));
            changed |= seed_response.changed();
            seed_response.lab_hover_tip("筆致テクスチャのパターンを変えます。");
            params.seed = seed.max(0) as u32;
        }
        LocalEffect::Cutout(params) => {
            ui.label(egui::RichText::new("プリセット").color(Color32::from_gray(190)));
            ui.horizontal_wrapped(|ui| {
                if preset_button(ui, "フラット") {
                    *params = CutoutParams {
                        levels: 5,
                        radius_px: 6.0,
                        edge_strength: 0.22,
                        color_amount: 0.9,
                        strength: 1.0,
                    };
                    changed = true;
                }
                if preset_button(ui, "ポスター") {
                    *params = CutoutParams {
                        levels: 4,
                        radius_px: 3.0,
                        edge_strength: 0.12,
                        color_amount: 1.0,
                        strength: 1.0,
                    };
                    changed = true;
                }
                if preset_button(ui, "柔らかめ") {
                    *params = CutoutParams {
                        levels: 6,
                        radius_px: 10.0,
                        edge_strength: 0.08,
                        color_amount: 0.75,
                        strength: 0.85,
                    };
                    changed = true;
                }
                if preset_button(ui, "輪郭強め") {
                    *params = CutoutParams {
                        levels: 5,
                        radius_px: 5.0,
                        edge_strength: 0.55,
                        color_amount: 0.85,
                        strength: 1.0,
                    };
                    changed = true;
                }
            });
            ui.label(
                egui::RichText::new(
                    "色面をなじませて階調を減らし、切り絵やフラットなベクター調に寄せます。",
                )
                .size(10.0)
                .color(Color32::from_gray(170)),
            );
            let mut levels = params.levels as i32;
            let levels_response = ui.add(egui::Slider::new(&mut levels, 2..=12).text("階調"));
            changed |= levels_response.changed();
            levels_response.lab_hover_tip("色面の明るさを何段階にまとめるかです。");
            params.levels = levels.clamp(2, 12) as u8;
            let radius = ui.add(
                egui::Slider::new(&mut params.radius_px, 0.0..=24.0)
                    .text("面のなじませ")
                    .suffix("px"),
            );
            changed |= radius.changed();
            radius
                .lab_hover_tip("階調化の前に色をなじませる量です。大きいほど大きな面になります。");
            let edge = ui.add(egui::Slider::new(&mut params.edge_strength, 0.0..=1.0).text("輪郭"));
            changed |= edge.changed();
            edge.lab_hover_tip("面の境界や元画像のエッジをどれだけ締めるかです。");
            let color = ui.add(egui::Slider::new(&mut params.color_amount, 0.0..=1.0).text("色量"));
            changed |= color.changed();
            color.lab_hover_tip("元の色の鮮やかさをどれだけ残すかです。");
            let strength = ui.add(egui::Slider::new(&mut params.strength, 0.0..=1.0).text("強さ"));
            changed |= strength.changed();
            strength.lab_hover_tip("元画像から切り絵結果へどれだけ近づけるかです。");
        }
        LocalEffect::Emboss(params) => {
            ui.label(egui::RichText::new("プリセット").color(Color32::from_gray(190)));
            ui.horizontal_wrapped(|ui| {
                if preset_button(ui, "浅い") {
                    *params = EmbossParams {
                        angle_degrees: 135.0,
                        depth: 0.7,
                        contrast: 0.12,
                        color_amount: 0.0,
                        strength: 0.85,
                    };
                    changed = true;
                }
                if preset_button(ui, "くっきり") {
                    *params = EmbossParams {
                        angle_degrees: 135.0,
                        depth: 1.35,
                        contrast: 0.45,
                        color_amount: 0.0,
                        strength: 1.0,
                    };
                    changed = true;
                }
                if preset_button(ui, "金属調") {
                    *params = EmbossParams {
                        angle_degrees: 120.0,
                        depth: 1.65,
                        contrast: 0.8,
                        color_amount: 0.0,
                        strength: 1.0,
                    };
                    changed = true;
                }
                if preset_button(ui, "色付き") {
                    *params = EmbossParams {
                        angle_degrees: 135.0,
                        depth: 1.0,
                        contrast: 0.35,
                        color_amount: 0.55,
                        strength: 0.9,
                    };
                    changed = true;
                }
            });
            ui.label(
                egui::RichText::new(
                    "明るさの傾きから陰影を作り、紙や金属の浮き彫りのように見せます。",
                )
                .size(10.0)
                .color(Color32::from_gray(170)),
            );
            let angle = ui.add(
                egui::Slider::new(&mut params.angle_degrees, -180.0..=180.0)
                    .text("角度")
                    .suffix("°"),
            );
            changed |= angle.changed();
            angle.lab_hover_tip("光が当たる方向です。180度変えると凹凸の向きが反転します。");
            let depth = ui.add(egui::Slider::new(&mut params.depth, 0.0..=4.0).text("深さ"));
            changed |= depth.changed();
            depth.lab_hover_tip("明暗差をどれだけ浮き彫りの陰影へ変換するかです。");
            let contrast =
                ui.add(egui::Slider::new(&mut params.contrast, -1.0..=1.0).text("コントラスト"));
            changed |= contrast.changed();
            contrast.lab_hover_tip("エンボス陰影の硬さです。高いほど金属的に締まります。");
            let color = ui.add(egui::Slider::new(&mut params.color_amount, 0.0..=1.0).text("色量"));
            changed |= color.changed();
            color.lab_hover_tip("0ではモノクロ、上げると元画像の色を浮き彫りに残します。");
            let strength = ui.add(egui::Slider::new(&mut params.strength, 0.0..=1.0).text("強さ"));
            changed |= strength.changed();
            strength.lab_hover_tip("元画像からエンボス結果へどれだけ近づけるかです。");
        }
        LocalEffect::PixelStylize(params) => {
            ui.label(egui::RichText::new("プリセット").color(Color32::from_gray(190)));
            ui.horizontal_wrapped(|ui| {
                if preset_button(ui, "結晶化") {
                    *params = PixelStylizeParams {
                        mode: PixelStylizeMode::Crystallize,
                        cell_px: 16.0,
                        edge_strength: 0.35,
                        color_amount: 0.9,
                        randomness: 0.65,
                        strength: 1.0,
                        seed: params.seed,
                    };
                    changed = true;
                }
                if preset_button(ui, "点描") {
                    *params = PixelStylizeParams {
                        mode: PixelStylizeMode::Pointillize,
                        cell_px: 11.0,
                        edge_strength: 0.08,
                        color_amount: 1.0,
                        randomness: 0.55,
                        strength: 1.0,
                        seed: params.seed,
                    };
                    changed = true;
                }
                if preset_button(ui, "Facet") {
                    *params = PixelStylizeParams {
                        mode: PixelStylizeMode::Facet,
                        cell_px: 18.0,
                        edge_strength: 0.25,
                        color_amount: 0.95,
                        randomness: 0.35,
                        strength: 1.0,
                        seed: params.seed,
                    };
                    changed = true;
                }
                if preset_button(ui, "メゾチント") {
                    *params = PixelStylizeParams {
                        mode: PixelStylizeMode::Mezzotint,
                        cell_px: 3.0,
                        edge_strength: 0.0,
                        color_amount: 0.0,
                        randomness: 0.8,
                        strength: 1.0,
                        seed: params.seed,
                    };
                    changed = true;
                }
            });
            ui.label(
                egui::RichText::new(
                    "セルや粒で色を再構成します。結晶化/Facet は面、点描/メゾチントは粒の表現に向いています。",
                )
                .size(10.0)
                .color(Color32::from_gray(170)),
            );
            ui.horizontal_wrapped(|ui| {
                let crystallize = params.mode == PixelStylizeMode::Crystallize;
                if ui.selectable_label(crystallize, "結晶化").clicked() && !crystallize {
                    params.mode = PixelStylizeMode::Crystallize;
                    changed = true;
                }
                let pointillize = params.mode == PixelStylizeMode::Pointillize;
                if ui.selectable_label(pointillize, "点描").clicked() && !pointillize {
                    params.mode = PixelStylizeMode::Pointillize;
                    changed = true;
                }
                let facet = params.mode == PixelStylizeMode::Facet;
                if ui.selectable_label(facet, "Facet").clicked() && !facet {
                    params.mode = PixelStylizeMode::Facet;
                    changed = true;
                }
                let mezzotint = params.mode == PixelStylizeMode::Mezzotint;
                if ui.selectable_label(mezzotint, "メゾチント").clicked() && !mezzotint {
                    params.mode = PixelStylizeMode::Mezzotint;
                    changed = true;
                }
            });
            let size = ui.add(
                egui::Slider::new(&mut params.cell_px, 1.0..=48.0)
                    .text("サイズ")
                    .suffix("px"),
            );
            changed |= size.changed();
            size.lab_hover_tip("結晶や点の大きさです。メゾチントでは粒の粗さとして働きます。");
            let edge = ui.add(egui::Slider::new(&mut params.edge_strength, 0.0..=1.0).text("輪郭"));
            changed |= edge.changed();
            edge.lab_hover_tip("面や粒の境界、元画像の輪郭をどれだけ締めるかです。");
            let color = ui.add(egui::Slider::new(&mut params.color_amount, 0.0..=1.0).text("色量"));
            changed |= color.changed();
            color.lab_hover_tip("0ではモノクロ寄り、上げると元画像の色を強く残します。");
            let randomness =
                ui.add(egui::Slider::new(&mut params.randomness, 0.0..=1.0).text("ばらつき"));
            changed |= randomness.changed();
            randomness.lab_hover_tip("セル位置や粒のランダムさです。下げると規則的になります。");
            let strength = ui.add(egui::Slider::new(&mut params.strength, 0.0..=1.0).text("強さ"));
            changed |= strength.changed();
            strength.lab_hover_tip("元画像から粒状スタイル結果へどれだけ近づけるかです。");
            let mut seed = params.seed as i32;
            let seed_response = ui.add(egui::Slider::new(&mut seed, 0..=9999).text("seed"));
            changed |= seed_response.changed();
            seed_response.lab_hover_tip("セルや粒の配置パターンを変えます。");
            params.seed = seed.max(0) as u32;
        }
        LocalEffect::Solarize(params) => {
            ui.label(egui::RichText::new("プリセット").color(Color32::from_gray(190)));
            ui.horizontal_wrapped(|ui| {
                if preset_button(ui, "標準") {
                    *params = SolarizeParams {
                        threshold: 0.55,
                        softness: 0.08,
                        inversion: 1.0,
                        contrast: 0.05,
                        color_amount: 1.0,
                        strength: 1.0,
                    };
                    changed = true;
                }
                if preset_button(ui, "柔らかい") {
                    *params = SolarizeParams {
                        threshold: 0.52,
                        softness: 0.22,
                        inversion: 0.85,
                        contrast: -0.05,
                        color_amount: 0.85,
                        strength: 0.85,
                    };
                    changed = true;
                }
                if preset_button(ui, "白黒") {
                    *params = SolarizeParams {
                        threshold: 0.50,
                        softness: 0.08,
                        inversion: 1.0,
                        contrast: 0.25,
                        color_amount: 0.0,
                        strength: 1.0,
                    };
                    changed = true;
                }
                if preset_button(ui, "ハイライト") {
                    *params = SolarizeParams {
                        threshold: 0.68,
                        softness: 0.06,
                        inversion: 1.0,
                        contrast: 0.15,
                        color_amount: 1.0,
                        strength: 1.0,
                    };
                    changed = true;
                }
            });
            ui.label(
                egui::RichText::new(
                    "しきい値より明るいトーンを反転します。ネガより部分的で、境目の色ずれや暗室風の効果を作れます。",
                )
                .size(10.0)
                .color(Color32::from_gray(170)),
            );
            let threshold =
                ui.add(egui::Slider::new(&mut params.threshold, 0.0..=1.0).text("しきい値"));
            changed |= threshold.changed();
            threshold
                .lab_hover_tip("反転を始める明るさです。上げるほどハイライトだけが反転します。");
            let softness =
                ui.add(egui::Slider::new(&mut params.softness, 0.0..=0.5).text("柔らかさ"));
            changed |= softness.changed();
            softness.lab_hover_tip("しきい値前後の反転をどれだけなだらかにするかです。");
            let inversion =
                ui.add(egui::Slider::new(&mut params.inversion, 0.0..=1.0).text("反転量"));
            changed |= inversion.changed();
            inversion.lab_hover_tip("明るいトーンを反対側へ折り返す量です。");
            let color = ui.add(egui::Slider::new(&mut params.color_amount, 0.0..=1.0).text("色量"));
            changed |= color.changed();
            color.lab_hover_tip(
                "0では白黒のトーン反転、上げるとRGBチャンネルごとの色ずれを残します。",
            );
            let contrast =
                ui.add(egui::Slider::new(&mut params.contrast, -1.0..=1.0).text("コントラスト"));
            changed |= contrast.changed();
            contrast.lab_hover_tip("反転後の明暗差を締めたり、柔らかくしたりします。");
            let strength = ui.add(egui::Slider::new(&mut params.strength, 0.0..=1.0).text("強さ"));
            changed |= strength.changed();
            strength.lab_hover_tip("元画像からソラリゼーション結果へどれだけ近づけるかです。");
        }
        LocalEffect::GlowingEdges(params) => {
            ui.label(egui::RichText::new("プリセット").color(Color32::from_gray(190)));
            ui.horizontal_wrapped(|ui| {
                if preset_button(ui, "シアン") {
                    *params = GlowingEdgesParams {
                        threshold: 0.18,
                        softness: 0.10,
                        edge_width_px: 1.0,
                        glow_radius_px: 8.0,
                        edge_brightness: 1.20,
                        glow_strength: 0.90,
                        hue_degrees: 190.0,
                        color_amount: 0.90,
                        background_amount: 0.0,
                        strength: 1.0,
                    };
                    changed = true;
                }
                if preset_button(ui, "紫ネオン") {
                    *params = GlowingEdgesParams {
                        threshold: 0.15,
                        softness: 0.12,
                        edge_width_px: 2.0,
                        glow_radius_px: 12.0,
                        edge_brightness: 1.25,
                        glow_strength: 1.05,
                        hue_degrees: 285.0,
                        color_amount: 0.95,
                        background_amount: 0.0,
                        strength: 1.0,
                    };
                    changed = true;
                }
                if preset_button(ui, "元画像") {
                    *params = GlowingEdgesParams {
                        threshold: 0.20,
                        softness: 0.12,
                        edge_width_px: 1.0,
                        glow_radius_px: 7.0,
                        edge_brightness: 0.95,
                        glow_strength: 0.75,
                        hue_degrees: 200.0,
                        color_amount: 0.65,
                        background_amount: 0.65,
                        strength: 0.85,
                    };
                    changed = true;
                }
                if preset_button(ui, "細線") {
                    *params = GlowingEdgesParams {
                        threshold: 0.28,
                        softness: 0.04,
                        edge_width_px: 1.0,
                        glow_radius_px: 3.0,
                        edge_brightness: 1.55,
                        glow_strength: 0.35,
                        hue_degrees: 145.0,
                        color_amount: 1.0,
                        background_amount: 0.0,
                        strength: 1.0,
                    };
                    changed = true;
                }
            });
            ui.label(
                egui::RichText::new(
                    "明るさの輪郭を抽出し、黒背景または元画像上にネオン色の線と光彩を重ねます。",
                )
                .size(10.0)
                .color(Color32::from_gray(170)),
            );
            let threshold =
                ui.add(egui::Slider::new(&mut params.threshold, 0.0..=1.0).text("しきい値"));
            changed |= threshold.changed();
            threshold.lab_hover_tip("光らせる輪郭の強さです。上げるほど強い輪郭だけが残ります。");
            let softness =
                ui.add(egui::Slider::new(&mut params.softness, 0.0..=0.5).text("柔らかさ"));
            changed |= softness.changed();
            softness.lab_hover_tip("しきい値付近の輪郭をどれだけなだらかに出すかです。");
            let edge_width = ui.add(
                egui::Slider::new(&mut params.edge_width_px, 1.0..=12.0)
                    .text("線幅")
                    .suffix("px"),
            );
            changed |= edge_width.changed();
            edge_width.lab_hover_tip("抽出した輪郭を広げる幅です。");
            let glow_radius = ui.add(
                egui::Slider::new(&mut params.glow_radius_px, 0.0..=80.0)
                    .text("光彩半径")
                    .suffix("px"),
            );
            changed |= glow_radius.changed();
            glow_radius.lab_hover_tip("輪郭の周囲へ広げる発光の大きさです。");
            let edge_brightness = ui
                .add(egui::Slider::new(&mut params.edge_brightness, 0.0..=3.0).text("線の明るさ"));
            changed |= edge_brightness.changed();
            edge_brightness.lab_hover_tip("輪郭線そのものの明るさです。");
            let glow_strength =
                ui.add(egui::Slider::new(&mut params.glow_strength, 0.0..=3.0).text("光彩"));
            changed |= glow_strength.changed();
            glow_strength.lab_hover_tip("ぼかした発光をどれだけ加えるかです。");
            ui.horizontal_wrapped(|ui| {
                let hue = ui.add(
                    egui::Slider::new(&mut params.hue_degrees, 0.0..=360.0)
                        .text("色相")
                        .suffix("°"),
                );
                changed |= hue.changed();
                hue.lab_hover_tip("ネオン色の色相です。");
                let swatch = hsl_swatch_color(params.hue_degrees, 1.0, 0.55);
                let (rect, _) = ui.allocate_exact_size(egui::vec2(22.0, 22.0), Sense::hover());
                ui.painter().rect_filled(rect, 4.0, swatch);
                ui.painter().rect_stroke(
                    rect,
                    4.0,
                    egui::Stroke::new(1.0, Color32::from_rgba_unmultiplied(255, 255, 255, 110)),
                    egui::StrokeKind::Inside,
                );
            });
            let color = ui.add(egui::Slider::new(&mut params.color_amount, 0.0..=1.0).text("色量"));
            changed |= color.changed();
            color.lab_hover_tip("0では元画像の輪郭色、上げると指定したネオン色へ寄せます。");
            let background = ui.add(
                egui::Slider::new(&mut params.background_amount, 0.0..=1.0).text("背景を残す"),
            );
            changed |= background.changed();
            background.lab_hover_tip("0では黒背景、上げると元画像を背景として残します。");
            let strength = ui.add(egui::Slider::new(&mut params.strength, 0.0..=1.0).text("強さ"));
            changed |= strength.changed();
            strength.lab_hover_tip("元画像からエッジ光彩結果へどれだけ近づけるかです。");
        }
        LocalEffect::OilPaint(params) => {
            ui.label(egui::RichText::new("プリセット").color(Color32::from_gray(190)));
            ui.horizontal_wrapped(|ui| {
                if preset_button(ui, "標準") {
                    *params = OilPaintParams {
                        radius_px: 5.0,
                        saturation: 0.08,
                        contrast: 0.04,
                        strength: 1.0,
                    };
                    changed = true;
                }
                if preset_button(ui, "厚塗り") {
                    *params = OilPaintParams {
                        radius_px: 8.0,
                        saturation: 0.18,
                        contrast: 0.14,
                        strength: 1.0,
                    };
                    changed = true;
                }
                if preset_button(ui, "柔らかめ") {
                    *params = OilPaintParams {
                        radius_px: 6.0,
                        saturation: -0.04,
                        contrast: -0.08,
                        strength: 0.85,
                    };
                    changed = true;
                }
                if preset_button(ui, "細部残し") {
                    *params = OilPaintParams {
                        radius_px: 3.0,
                        saturation: 0.04,
                        contrast: 0.0,
                        strength: 0.75,
                    };
                    changed = true;
                }
            });
            ui.label(
                egui::RichText::new(
                    "4象限の輝度分散が小さい領域を選んで平均色に置き換える Kuwahara 系の油彩化です。",
                )
                .size(10.0)
                .color(Color32::from_gray(170)),
            );
            let radius = ui.add(
                egui::Slider::new(&mut params.radius_px, 1.0..=12.0)
                    .text("半径")
                    .suffix("px"),
            );
            changed |= radius.changed();
            radius.lab_hover_tip("色面をなじませる範囲です。大きいほど厚塗り風になります。");
            let saturation =
                ui.add(egui::Slider::new(&mut params.saturation, -1.0..=1.0).text("彩度"));
            changed |= saturation.changed();
            saturation.lab_hover_tip("油彩化した色面の鮮やかさです。");
            let contrast =
                ui.add(egui::Slider::new(&mut params.contrast, -1.0..=1.0).text("コントラスト"));
            changed |= contrast.changed();
            contrast.lab_hover_tip("油彩化した色面の明暗差を締めたり柔らかくしたりします。");
            let strength = ui.add(egui::Slider::new(&mut params.strength, 0.0..=1.0).text("強さ"));
            changed |= strength.changed();
            strength.lab_hover_tip("元画像から油彩結果へどれだけ近づけるかです。");
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
                if preset_button(ui, "長辺0.5倍") {
                    params.tile_mode = MosaicTileMode::LongEdgeRatio(0.5);
                    params.clear_legacy_block_px();
                    changed = true;
                }
                if preset_button(ui, "長辺1倍") {
                    params.tile_mode = MosaicTileMode::LongEdgeRatio(1.0);
                    params.clear_legacy_block_px();
                    changed = true;
                }
                if preset_button(ui, "長辺2倍") {
                    params.tile_mode = MosaicTileMode::LongEdgeRatio(2.0);
                    params.clear_legacy_block_px();
                    changed = true;
                }
            });
            let long_edge = image_dims.0.max(image_dims.1) as u32;
            let mut mode = params.effective_tile_mode();
            ui.horizontal(|ui| {
                let ratio_selected = matches!(mode, MosaicTileMode::LongEdgeRatio(_));
                if ui.selectable_label(ratio_selected, "長辺比率").clicked() && !ratio_selected
                {
                    let multiplier = match mode {
                        MosaicTileMode::LongEdgeRatio(value) => value,
                        MosaicTileMode::FixedPx(_) => 1.0,
                    };
                    mode = MosaicTileMode::LongEdgeRatio(multiplier);
                    params.tile_mode = mode;
                    params.clear_legacy_block_px();
                    changed = true;
                }
                let fixed_selected = matches!(mode, MosaicTileMode::FixedPx(_));
                if ui.selectable_label(fixed_selected, "固定px").clicked() && !fixed_selected {
                    let fixed_px = compute_mosaic_tile_size(long_edge, mode).max(1);
                    mode = MosaicTileMode::FixedPx(fixed_px);
                    params.tile_mode = mode;
                    params.clear_legacy_block_px();
                    changed = true;
                }
            });
            match mode {
                MosaicTileMode::LongEdgeRatio(multiplier) => {
                    let mut value = multiplier;
                    let response = ui.add(
                        egui::Slider::new(&mut value, 0.25..=5.0)
                            .step_by(0.25)
                            .text("長辺比率"),
                    );
                    if response.changed() {
                        params.tile_mode = MosaicTileMode::LongEdgeRatio(value);
                        params.clear_legacy_block_px();
                        mode = params.tile_mode;
                        changed = true;
                    }
                }
                MosaicTileMode::FixedPx(px) => {
                    let mut value = px as i32;
                    let response = ui.add(egui::Slider::new(&mut value, 1..=200).text("固定px"));
                    if response.changed() {
                        params.tile_mode = MosaicTileMode::FixedPx(value.max(1) as u32);
                        params.clear_legacy_block_px();
                        mode = params.tile_mode;
                        changed = true;
                    }
                }
            }
            let actual_px = compute_mosaic_tile_size(long_edge, mode);
            ui.label(
                egui::RichText::new(format!("実タイルサイズ: {actual_px}px"))
                    .size(11.0)
                    .color(Color32::from_gray(170)),
            );

            ui.separator();
            let before_boundary = params.boundary;
            lab_combo_box(
                ui,
                "mosaic_boundary",
                mosaic_boundary_label(params.boundary),
                |ui| {
                    for boundary in [
                        MosaicBoundary::Opaque,
                        MosaicBoundary::Translucent,
                        MosaicBoundary::MaskShape,
                    ] {
                        ui.selectable_value(
                            &mut params.boundary,
                            boundary,
                            mosaic_boundary_label(boundary),
                        );
                    }
                },
            );
            if params.boundary != before_boundary {
                changed = true;
            }
            ui.label(
                egui::RichText::new(params.boundary.process_description())
                    .size(10.0)
                    .color(Color32::from_gray(170)),
            );
            if matches!(params.boundary, MosaicBoundary::Opaque) {
                ui.label(
                    egui::RichText::new(
                        "隠蔽加工と同じく、マスクに触れたタイル全体へ効果が広がります。",
                    )
                    .size(10.0)
                    .color(Color32::from_gray(170)),
                );
            }
        }
        LocalEffect::Sharpen(params) => {
            ui.label(egui::RichText::new("プリセット").color(Color32::from_gray(190)));
            ui.horizontal_wrapped(|ui| {
                if preset_button(ui, "弱く") {
                    *params = SharpenParams {
                        amount: 0.35,
                        radius_px: 1.0,
                        threshold: 0.0,
                    };
                    changed = true;
                }
                if preset_button(ui, "くっきり") {
                    *params = SharpenParams {
                        amount: 0.7,
                        radius_px: 1.0,
                        threshold: 4.0,
                    };
                    changed = true;
                }
                if preset_button(ui, "線強調") {
                    *params = SharpenParams {
                        amount: 0.55,
                        radius_px: 2.0,
                        threshold: 8.0,
                    };
                    changed = true;
                }
                if preset_button(ui, "ノイズ抑制") {
                    *params = SharpenParams {
                        amount: 0.95,
                        radius_px: 1.5,
                        threshold: 12.0,
                    };
                    changed = true;
                }
            });
            let amount = ui.add(egui::Slider::new(&mut params.amount, 0.0..=2.0).text("量"));
            changed |= amount.changed();
            amount
                .lab_hover_tip("輪郭へ足し戻す強さです。上げすぎると白黒の縁が出やすくなります。");
            let radius = ui.add(egui::Slider::new(&mut params.radius_px, 0.0..=12.0).text("半径"));
            changed |= radius.changed();
            radius.lab_hover_tip(
                "輪郭として扱う幅です。小さい値は細部、大きい値は太い輪郭に効きます。",
            );
            let threshold =
                ui.add(egui::Slider::new(&mut params.threshold, 0.0..=64.0).text("しきい値"));
            changed |= threshold.changed();
            threshold.lab_hover_tip(
                "小さな明暗差を無視する量です。値を上げるとノイズや微妙なざらつきに効きにくくなります。",
            );
        }
        LocalEffect::SmartSharpen(params) => {
            ui.label(egui::RichText::new("プリセット").color(Color32::from_gray(190)));
            ui.horizontal_wrapped(|ui| {
                if preset_button(ui, "自然") {
                    *params = SmartSharpenParams {
                        amount: 0.65,
                        radius_px: 2.0,
                        edge_threshold: 0.08,
                        halo_suppression: 0.65,
                    };
                    changed = true;
                }
                if preset_button(ui, "細部") {
                    *params = SmartSharpenParams {
                        amount: 0.95,
                        radius_px: 1.2,
                        edge_threshold: 0.05,
                        halo_suppression: 0.45,
                    };
                    changed = true;
                }
                if preset_button(ui, "輪郭") {
                    *params = SmartSharpenParams {
                        amount: 1.15,
                        radius_px: 3.0,
                        edge_threshold: 0.12,
                        halo_suppression: 0.75,
                    };
                    changed = true;
                }
                if preset_button(ui, "フチ抑制") {
                    *params = SmartSharpenParams {
                        amount: 1.2,
                        radius_px: 2.4,
                        edge_threshold: 0.08,
                        halo_suppression: 1.0,
                    };
                    changed = true;
                }
            });
            let amount = ui.add(egui::Slider::new(&mut params.amount, 0.0..=2.0).text("量"));
            changed |= amount.changed();
            amount.lab_hover_tip(
                "輪郭に足し戻す強さです。通常のシャープよりエッジを選んで効きます。",
            );
            let radius = ui.add(egui::Slider::new(&mut params.radius_px, 0.0..=16.0).text("半径"));
            changed |= radius.changed();
            radius.lab_hover_tip(
                "復元する輪郭の幅です。細部は小さく、太い線や境界は大きめにします。",
            );
            let edge_threshold =
                ui.add(egui::Slider::new(&mut params.edge_threshold, 0.0..=0.5).text("エッジ判定"));
            changed |= edge_threshold.changed();
            edge_threshold.lab_hover_tip(
                "どれだけ明暗差がある場所を輪郭として扱うかです。上げると平坦部に効きにくくなります。",
            );
            let halo =
                ui.add(egui::Slider::new(&mut params.halo_suppression, 0.0..=1.0).text("フチ抑制"));
            changed |= halo.changed();
            halo.lab_hover_tip("明るいフチや暗いフチが立ちすぎる方向の強調を抑えます。");
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
        LocalEffect::ColorMixer(params) => {
            ui.label(egui::RichText::new("プリセット").color(Color32::from_gray(190)));
            ui.horizontal_wrapped(|ui| {
                if preset_button(ui, "空を濃く") {
                    *params = ColorMixerParams::default();
                    params.bands[4].saturation = 18.0;
                    params.bands[4].lightness = -6.0;
                    params.bands[5].saturation = 26.0;
                    params.bands[5].lightness = -10.0;
                    changed = true;
                }
                if preset_button(ui, "緑を鮮やか") {
                    *params = ColorMixerParams::default();
                    params.bands[3].saturation = 32.0;
                    params.bands[3].lightness = 4.0;
                    changed = true;
                }
                if preset_button(ui, "肌を明るく") {
                    *params = ColorMixerParams::default();
                    params.bands[1].saturation = 8.0;
                    params.bands[1].lightness = 12.0;
                    params.bands[2].lightness = 4.0;
                    changed = true;
                }
                if preset_button(ui, "赤を桜色") {
                    *params = ColorMixerParams::default();
                    params.bands[0].hue_degrees = 16.0;
                    params.bands[0].saturation = -8.0;
                    params.bands[0].lightness = 8.0;
                    params.bands[7].lightness = 6.0;
                    changed = true;
                }
                if preset_button(ui, "青を紫へ") {
                    *params = ColorMixerParams::default();
                    params.bands[5].hue_degrees = 32.0;
                    params.bands[5].saturation = 10.0;
                    params.bands[6].saturation = 12.0;
                    changed = true;
                }
                if preset_button(ui, "黄を橙へ") {
                    *params = ColorMixerParams::default();
                    params.bands[2].hue_degrees = -18.0;
                    params.bands[2].saturation = 12.0;
                    params.bands[1].saturation = 10.0;
                    changed = true;
                }
            });
            ui.label(
                egui::RichText::new(
                    "色相ごとに補正します。カラー範囲マスクなしでも、近い色だけをまとめて調整できます。",
                )
                .size(10.0)
                .color(Color32::from_gray(170)),
            );
            let range_response =
                ui.add(egui::Slider::new(&mut params.range_degrees, 8.0..=90.0).text("色帯の広さ"));
            changed |= range_response.changed();
            range_response.lab_hover_tip("大きくすると隣の色にもなだらかに効果が広がります。");
            for (idx, band) in params.bands.iter_mut().enumerate() {
                ui.collapsing(color_mixer_band_label(idx), |ui| {
                    let hue = ui
                        .add(egui::Slider::new(&mut band.hue_degrees, -180.0..=180.0).text("色相"));
                    changed |= hue.changed();
                    hue.lab_hover_tip("この色帯だけ色相をずらします。");
                    let saturation = ui
                        .add(egui::Slider::new(&mut band.saturation, -100.0..=100.0).text("彩度"));
                    changed |= saturation.changed();
                    saturation.lab_hover_tip("この色帯だけ鮮やかさを増減します。");
                    let lightness =
                        ui.add(egui::Slider::new(&mut band.lightness, -100.0..=100.0).text("明度"));
                    changed |= lightness.changed();
                    lightness.lab_hover_tip("この色帯だけ明るさを増減します。");
                });
            }
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
                    LookPreset::None,
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
            if params.preset != before {
                if params.preset != LookPreset::None && params.strength <= f32::EPSILON {
                    params.strength = 1.0;
                }
                changed = true;
            }
            changed |= ui
                .add(egui::Slider::new(&mut params.strength, 0.0..=1.0).text("強度"))
                .changed();
        }
        LocalEffect::CubeLut(params) => {
            ui.label(egui::RichText::new("LUTファイル").color(Color32::from_gray(190)));
            if params.is_loaded() {
                ui.label(format!("読み込み済み: {} ({}^3)", params.name, params.size));
            } else {
                ui.label("未読み込みです。`.cube` ファイルを選択してください。");
            }
            if ui.button("LUTファイルを選択").clicked() {
                load_cube_lut = true;
            }
            ui.label(
                egui::RichText::new(
                    "3D LUT は RGB の組み合わせごとに色を変換する外部カラープリセットです。読み込んだ LUT データは設定ファイルにも保存されます。",
                )
                .size(10.0)
                .color(Color32::from_gray(170)),
            );
            let strength = ui.add(egui::Slider::new(&mut params.strength, 0.0..=1.0).text("強度"));
            changed |= strength.changed();
            strength.lab_hover_tip("元画像から LUT 変換後の色へどれだけ近づけるかです。");
        }
        LocalEffect::Posterize(params) => {
            ui.label(egui::RichText::new("プリセット").color(Color32::from_gray(190)));
            ui.horizontal_wrapped(|ui| {
                if preset_button(ui, "弱 16段") {
                    *params = PosterizeParams {
                        levels: 16,
                        strength: 1.0,
                    };
                    changed = true;
                }
                if preset_button(ui, "中 8段") {
                    *params = PosterizeParams {
                        levels: 8,
                        strength: 1.0,
                    };
                    changed = true;
                }
                if preset_button(ui, "強 5段") {
                    *params = PosterizeParams {
                        levels: 5,
                        strength: 1.0,
                    };
                    changed = true;
                }
                if preset_button(ui, "超強 3段") {
                    *params = PosterizeParams {
                        levels: 3,
                        strength: 1.0,
                    };
                    changed = true;
                }
            });
            ui.label(
                egui::RichText::new(
                    "RGB各チャンネルの階調を指定段数へ丸めます。色数を減らしたポスター調やレトロ調に使います。",
                )
                .size(10.0)
                .color(Color32::from_gray(170)),
            );
            let levels = ui.add(egui::Slider::new(&mut params.levels, 2..=256).text("階調数"));
            changed |= levels.changed();
            levels.lab_hover_tip("値を小さくすると、使われる明るさの段数が減ってフラットになります。256でほぼ無加工です。");
            let strength = ui.add(egui::Slider::new(&mut params.strength, 0.0..=1.0).text("強度"));
            changed |= strength.changed();
            strength.lab_hover_tip("元画像から階調を減らした色へどれだけ近づけるかです。");
        }
        LocalEffect::Threshold(params) => {
            ui.label(egui::RichText::new("プリセット").color(Color32::from_gray(190)));
            ui.horizontal_wrapped(|ui| {
                if preset_button(ui, "標準") {
                    *params = ThresholdParams {
                        threshold: 0.50,
                        invert: false,
                        strength: 1.0,
                    };
                    changed = true;
                }
                if preset_button(ui, "明るめ") {
                    *params = ThresholdParams {
                        threshold: 0.40,
                        invert: false,
                        strength: 1.0,
                    };
                    changed = true;
                }
                if preset_button(ui, "暗め") {
                    *params = ThresholdParams {
                        threshold: 0.62,
                        invert: false,
                        strength: 1.0,
                    };
                    changed = true;
                }
                if preset_button(ui, "反転") {
                    *params = ThresholdParams {
                        threshold: 0.50,
                        invert: true,
                        strength: 1.0,
                    };
                    changed = true;
                }
            });
            ui.label(
                egui::RichText::new(
                    "輝度がしきい値以上なら白、それ未満なら黒にします。線画確認やモノクロ風の加工に使えます。",
                )
                .size(10.0)
                .color(Color32::from_gray(170)),
            );
            let threshold =
                ui.add(egui::Slider::new(&mut params.threshold, 0.0..=1.0).text("しきい値"));
            changed |= threshold.changed();
            threshold.lab_hover_tip(
                "白にする明るさの境目です。値を大きくすると、より明るい部分だけが白になります。",
            );
            let invert = ui.checkbox(&mut params.invert, "反転");
            changed |= invert.changed();
            invert.lab_hover_tip("黒と白を入れ替えます。");
            let strength = ui.add(egui::Slider::new(&mut params.strength, 0.0..=1.0).text("強度"));
            changed |= strength.changed();
            strength.lab_hover_tip("元画像から黒白化した結果へどれだけ近づけるかです。");
        }
        LocalEffect::Invert(params) => {
            ui.label(egui::RichText::new("プリセット").color(Color32::from_gray(190)));
            ui.horizontal_wrapped(|ui| {
                if preset_button(ui, "ネガ") {
                    *params = InvertParams { strength: 1.0 };
                    changed = true;
                }
                if preset_button(ui, "薄め") {
                    *params = InvertParams { strength: 0.35 };
                    changed = true;
                }
            });
            ui.label(
                egui::RichText::new(
                    "RGBの明暗を反転します。強度を下げると元画像とネガを混ぜた特殊な色味になります。",
                )
                .size(10.0)
                .color(Color32::from_gray(170)),
            );
            let strength = ui.add(egui::Slider::new(&mut params.strength, 0.0..=1.0).text("強度"));
            changed |= strength.changed();
            strength.lab_hover_tip("元画像から反転後の色へどれだけ近づけるかです。");
        }
        LocalEffect::Duotone(params) => {
            ui.label(egui::RichText::new("プリセット").color(Color32::from_gray(190)));
            ui.horizontal_wrapped(|ui| {
                if preset_button(ui, "セピア") {
                    *params = DuotoneParams {
                        preset: DuotonePreset::SepiaInk,
                        strength: 0.8,
                        contrast: 0.0,
                    };
                    changed = true;
                }
                if preset_button(ui, "青写真") {
                    *params = DuotoneParams {
                        preset: DuotonePreset::Cyanotype,
                        strength: 0.85,
                        contrast: 8.0,
                    };
                    changed = true;
                }
                if preset_button(ui, "黒赤") {
                    *params = DuotoneParams {
                        preset: DuotonePreset::BlackRed,
                        strength: 0.9,
                        contrast: 12.0,
                    };
                    changed = true;
                }
                if preset_button(ui, "紫金") {
                    *params = DuotoneParams {
                        preset: DuotonePreset::PurpleGold,
                        strength: 0.85,
                        contrast: 6.0,
                    };
                    changed = true;
                }
                if preset_button(ui, "夕暮れ3色") {
                    *params = DuotoneParams {
                        preset: DuotonePreset::SunsetTritone,
                        strength: 0.85,
                        contrast: 5.0,
                    };
                    changed = true;
                }
                if preset_button(ui, "コミック3色") {
                    *params = DuotoneParams {
                        preset: DuotonePreset::ComicTritone,
                        strength: 0.8,
                        contrast: 18.0,
                    };
                    changed = true;
                }
            });
            let before = params.preset;
            lab_combo_box(
                ui,
                "duotone_preset",
                duotone_preset_label(params.preset),
                |ui| {
                    for preset in [
                        DuotonePreset::None,
                        DuotonePreset::SepiaInk,
                        DuotonePreset::Cyanotype,
                        DuotonePreset::BlackRed,
                        DuotonePreset::PurpleGold,
                        DuotonePreset::TealCream,
                        DuotonePreset::SunsetTritone,
                        DuotonePreset::ComicTritone,
                        DuotonePreset::NoirTritone,
                    ] {
                        ui.selectable_value(
                            &mut params.preset,
                            preset,
                            duotone_preset_label(preset),
                        );
                    }
                },
            );
            if params.preset != before {
                if params.preset != DuotonePreset::None && params.strength <= f32::EPSILON {
                    params.strength = 1.0;
                }
                changed = true;
            }
            ui.label(
                egui::RichText::new(
                    "明るさを元に2色または3色のインク風カラーへ置き換えます。グラデーションマップより印刷・ポスター調に寄せた効果です。",
                )
                .size(10.0)
                .color(Color32::from_gray(170)),
            );
            let strength = ui.add(egui::Slider::new(&mut params.strength, 0.0..=1.0).text("強度"));
            changed |= strength.changed();
            strength.lab_hover_tip("元画像の色からダブルトーンの色へどれだけ近づけるかです。");
            let contrast = ui.add(
                egui::Slider::new(&mut params.contrast, -100.0..=100.0).text("明暗コントラスト"),
            );
            changed |= contrast.changed();
            contrast.lab_hover_tip("色を割り当てる前に明暗差を締めたり広げたりします。");
        }
        LocalEffect::Equalize(params) => {
            ui.label(egui::RichText::new("プリセット").color(Color32::from_gray(190)));
            ui.horizontal_wrapped(|ui| {
                if preset_button(ui, "弱") {
                    *params = EqualizeParams {
                        strength: 0.35,
                        preserve_color: true,
                    };
                    changed = true;
                }
                if preset_button(ui, "中") {
                    *params = EqualizeParams {
                        strength: 0.65,
                        preserve_color: true,
                    };
                    changed = true;
                }
                if preset_button(ui, "強") {
                    *params = EqualizeParams {
                        strength: 1.0,
                        preserve_color: true,
                    };
                    changed = true;
                }
                if preset_button(ui, "白黒") {
                    *params = EqualizeParams {
                        strength: 1.0,
                        preserve_color: false,
                    };
                    changed = true;
                }
            });
            ui.label(
                egui::RichText::new(
                    "画像全体の明暗分布を広げます。色を保つと元の色味をなるべく残し、白黒にすると輝度だけで階調を整えます。",
                )
                .size(10.0)
                .color(Color32::from_gray(170)),
            );
            let strength = ui.add(egui::Slider::new(&mut params.strength, 0.0..=1.0).text("強度"));
            changed |= strength.changed();
            strength
                .lab_hover_tip("元画像からヒストグラム平坦化した結果へどれだけ近づけるかです。");
            let preserve = ui.checkbox(&mut params.preserve_color, "色を保つ");
            changed |= preserve.changed();
            preserve.lab_hover_tip("ONにすると、明るさだけを広げて元の色相をなるべく維持します。OFFにすると白黒の平坦化になります。");
        }
        LocalEffect::GradientMap(params) => {
            ui.label(egui::RichText::new("プリセット").color(Color32::from_gray(190)));
            ui.horizontal_wrapped(|ui| {
                if preset_button(ui, "夕焼け") {
                    *params = GradientMapParams {
                        preset: GradientMapPreset::Sunset,
                        strength: 0.65,
                        contrast: 0.0,
                    };
                    changed = true;
                }
                if preset_button(ui, "薄暮") {
                    *params = GradientMapParams {
                        preset: GradientMapPreset::Twilight,
                        strength: 0.65,
                        contrast: 0.0,
                    };
                    changed = true;
                }
                if preset_button(ui, "ティール") {
                    *params = GradientMapParams {
                        preset: GradientMapPreset::TealOrange,
                        strength: 0.65,
                        contrast: 0.0,
                    };
                    changed = true;
                }
                if preset_button(ui, "桜色") {
                    *params = GradientMapParams {
                        preset: GradientMapPreset::Cherry,
                        strength: 0.55,
                        contrast: 0.0,
                    };
                    changed = true;
                }
                if preset_button(ui, "炎") {
                    *params = GradientMapParams {
                        preset: GradientMapPreset::Fire,
                        strength: 0.70,
                        contrast: 0.0,
                    };
                    changed = true;
                }
                if preset_button(ui, "氷") {
                    *params = GradientMapParams {
                        preset: GradientMapPreset::Ice,
                        strength: 0.65,
                        contrast: 0.0,
                    };
                    changed = true;
                }
            });
            let before = params.preset;
            lab_combo_box(
                ui,
                "gradient_map_preset",
                gradient_map_preset_label(params.preset),
                |ui| {
                    for preset in [
                        GradientMapPreset::None,
                        GradientMapPreset::Mono,
                        GradientMapPreset::Sepia,
                        GradientMapPreset::Sunset,
                        GradientMapPreset::Twilight,
                        GradientMapPreset::TealOrange,
                        GradientMapPreset::Cherry,
                        GradientMapPreset::Forest,
                        GradientMapPreset::Fire,
                        GradientMapPreset::Ice,
                    ] {
                        ui.selectable_value(
                            &mut params.preset,
                            preset,
                            gradient_map_preset_label(preset),
                        );
                    }
                },
            );
            if params.preset != before {
                if params.preset != GradientMapPreset::None && params.strength <= f32::EPSILON {
                    params.strength = 1.0;
                }
                changed = true;
            }
            ui.label(
                egui::RichText::new(
                    "輝度をグラデーション色に置き換えます。マスクや強度を弱めると色味だけを乗せる用途にも使えます。",
                )
                .size(10.0)
                .color(Color32::from_gray(170)),
            );
            let strength_response =
                ui.add(egui::Slider::new(&mut params.strength, 0.0..=1.0).text("強度"));
            changed |= strength_response.changed();
            strength_response.lab_hover_tip("元の色からグラデーション色へ置き換える強さです。");
            let contrast_response = ui.add(
                egui::Slider::new(&mut params.contrast, -100.0..=100.0).text("明暗コントラスト"),
            );
            changed |= contrast_response.changed();
            contrast_response
                .lab_hover_tip("色を割り当てる前に、明るさの差を締めたり広げたりします。");
        }
        LocalEffect::ColorFill(params) => {
            ui.label(egui::RichText::new("プリセット").color(Color32::from_gray(190)));
            ui.horizontal_wrapped(|ui| {
                if preset_button(ui, "白背景") {
                    *params = ColorFillParams {
                        shape: ColorOverlayShape::Solid,
                        start_rgb: [255, 255, 255],
                        opacity: 1.0,
                        ..Default::default()
                    };
                    changed = true;
                }
                if preset_button(ui, "黒背景") {
                    *params = ColorFillParams {
                        shape: ColorOverlayShape::Solid,
                        start_rgb: [18, 18, 20],
                        opacity: 1.0,
                        ..Default::default()
                    };
                    changed = true;
                }
                if preset_button(ui, "淡色") {
                    *params = ColorFillParams {
                        shape: ColorOverlayShape::Solid,
                        start_rgb: [246, 238, 224],
                        opacity: 1.0,
                        ..Default::default()
                    };
                    changed = true;
                }
                if preset_button(ui, "青グラデ") {
                    *params = ColorFillParams {
                        shape: ColorOverlayShape::Linear,
                        start_rgb: [232, 242, 255],
                        end_rgb: [128, 170, 245],
                        angle_degrees: -18.0,
                        softness: 0.45,
                        opacity: 1.0,
                        ..Default::default()
                    };
                    changed = true;
                }
                if preset_button(ui, "3色背景") {
                    *params = ColorFillParams {
                        shape: ColorOverlayShape::Radial,
                        start_rgb: [255, 247, 230],
                        middle_rgb: [255, 206, 222],
                        end_rgb: [170, 195, 255],
                        middle_enabled: true,
                        midpoint: 0.48,
                        center: [0.46, 0.34],
                        radius: 0.92,
                        softness: 0.70,
                        opacity: 1.0,
                        ..Default::default()
                    };
                    changed = true;
                }
            });
            let before_shape = params.shape;
            lab_combo_box(
                ui,
                "color_fill_shape",
                color_overlay_shape_label(params.shape),
                |ui| {
                    for shape in [
                        ColorOverlayShape::Unselected,
                        ColorOverlayShape::Solid,
                        ColorOverlayShape::Linear,
                        ColorOverlayShape::Radial,
                    ] {
                        ui.selectable_value(
                            &mut params.shape,
                            shape,
                            color_overlay_shape_label(shape),
                        );
                    }
                },
            );
            if params.shape != before_shape {
                if params.shape != ColorOverlayShape::Unselected && params.opacity <= f32::EPSILON {
                    params.opacity = 1.0;
                }
                changed = true;
            }
            ui.label(
                egui::RichText::new(
                    "マスク範囲の元画像RGBを、指定した色またはグラデーション色へ置き換えます。被写体切り抜きの背景作成や確認用に向きます。",
                )
                .size(10.0)
                .color(Color32::from_gray(170)),
            );
            if params.shape != ColorOverlayShape::Unselected {
                let color_label = if params.shape == ColorOverlayShape::Solid {
                    "塗り色"
                } else {
                    "開始色"
                };
                merge_rgb_color_response(
                    draw_rgb_color_control(
                        ui,
                        color_label,
                        &mut params.start_rgb,
                        RgbPickTarget::ColorFillStart,
                        rgb_pick_active,
                    ),
                    &mut changed,
                    &mut start_rgb_pick,
                    &mut cancel_rgb_pick,
                );
                if params.shape != ColorOverlayShape::Solid {
                    let middle = ui.checkbox(&mut params.middle_enabled, "中間色を使う");
                    changed |= middle.changed();
                    middle.lab_hover_tip(
                        "ONにすると、開始色・中間色・終了色の3色グラデーションになります。",
                    );
                    if params.middle_enabled {
                        merge_rgb_color_response(
                            draw_rgb_color_control(
                                ui,
                                "中間色",
                                &mut params.middle_rgb,
                                RgbPickTarget::ColorFillMiddle,
                                rgb_pick_active,
                            ),
                            &mut changed,
                            &mut start_rgb_pick,
                            &mut cancel_rgb_pick,
                        );
                        let midpoint = ui.add(
                            egui::Slider::new(&mut params.midpoint, 0.01..=0.99).text("中間位置"),
                        );
                        changed |= midpoint.changed();
                        midpoint.lab_hover_tip("グラデーション内で中間色が出る位置です。");
                    }
                    merge_rgb_color_response(
                        draw_rgb_color_control(
                            ui,
                            "終了色",
                            &mut params.end_rgb,
                            RgbPickTarget::ColorFillEnd,
                            rgb_pick_active,
                        ),
                        &mut changed,
                        &mut start_rgb_pick,
                        &mut cancel_rgb_pick,
                    );
                }
                let opacity =
                    ui.add(egui::Slider::new(&mut params.opacity, 0.0..=1.0).text("不透明度"));
                changed |= opacity.changed();
                opacity.lab_hover_tip("元画像から塗りつぶし色へどれだけ置き換えるかです。");
                if params.shape == ColorOverlayShape::Linear {
                    let angle = ui.add(
                        egui::Slider::new(&mut params.angle_degrees, -180.0..=180.0)
                            .text("角度")
                            .suffix("°"),
                    );
                    if angle.changed() {
                        params.linear_points_enabled = false;
                        changed = true;
                    }
                    angle.lab_hover_tip(
                        "線形グラデーションの方向です。0°で左から右へ色が変わります。画像上をドラッグすると開始点と終了点も設定できます。",
                    );
                }
                if params.shape == ColorOverlayShape::Radial {
                    let center_x =
                        ui.add(egui::Slider::new(&mut params.center[0], 0.0..=1.0).text("中心X"));
                    changed |= center_x.changed();
                    center_x.lab_hover_tip("円形グラデーション中心の横位置です。");
                    let center_y =
                        ui.add(egui::Slider::new(&mut params.center[1], 0.0..=1.0).text("中心Y"));
                    changed |= center_y.changed();
                    center_y.lab_hover_tip("円形グラデーション中心の縦位置です。");
                    let radius =
                        ui.add(egui::Slider::new(&mut params.radius, 0.02..=2.0).text("半径"));
                    changed |= radius.changed();
                    radius.lab_hover_tip(
                        "中心色から終了色へ変わる範囲です。画像上をドラッグすると中心と半径を設定できます。",
                    );
                }
                if params.shape != ColorOverlayShape::Solid {
                    let softness = ui
                        .add(egui::Slider::new(&mut params.softness, 0.0..=1.0).text("なめらかさ"));
                    changed |= softness.changed();
                    softness.lab_hover_tip(
                        "グラデーションの変化を直線的にするか、なだらかにするかです。",
                    );
                }
            }
        }
        LocalEffect::OutlineStroke(params) => {
            ui.label(egui::RichText::new("プリセット").color(Color32::from_gray(190)));
            ui.horizontal_wrapped(|ui| {
                if preset_button(ui, "黒フチ") {
                    *params = OutlineStrokeParams {
                        placement: OutlineStrokePlacement::Outside,
                        width_px: 4.0,
                        softness_px: 1.0,
                        opacity: 1.0,
                        color_rgb: [0, 0, 0],
                    };
                    changed = true;
                }
                if preset_button(ui, "白ステッカー") {
                    *params = OutlineStrokeParams {
                        placement: OutlineStrokePlacement::Outside,
                        width_px: 8.0,
                        softness_px: 2.0,
                        opacity: 0.95,
                        color_rgb: [255, 255, 255],
                    };
                    changed = true;
                }
                if preset_button(ui, "内側色線") {
                    *params = OutlineStrokeParams {
                        placement: OutlineStrokePlacement::Inside,
                        width_px: 3.0,
                        softness_px: 1.0,
                        opacity: 0.85,
                        color_rgb: [80, 170, 255],
                    };
                    changed = true;
                }
            });
            ui.label(
                egui::RichText::new(
                    "マスク境界をもとに色枠を描きます。初期状態では前ON/後OFFなので、外側の縁取りがマスクの外へ出ます。",
                )
                .size(10.0)
                .color(Color32::from_gray(170)),
            );
            let before_placement = params.placement;
            lab_combo_box(
                ui,
                "outline_stroke_placement",
                outline_stroke_placement_label(params.placement),
                |ui| {
                    for placement in [
                        OutlineStrokePlacement::Outside,
                        OutlineStrokePlacement::Inside,
                        OutlineStrokePlacement::Center,
                    ] {
                        ui.selectable_value(
                            &mut params.placement,
                            placement,
                            outline_stroke_placement_label(placement),
                        );
                    }
                },
            );
            changed |= params.placement != before_placement;
            merge_rgb_color_response(
                draw_rgb_color_control(
                    ui,
                    "線色",
                    &mut params.color_rgb,
                    RgbPickTarget::OutlineStrokeColor,
                    rgb_pick_active,
                ),
                &mut changed,
                &mut start_rgb_pick,
                &mut cancel_rgb_pick,
            );
            let width = ui.add(
                egui::Slider::new(&mut params.width_px, 0.0..=64.0)
                    .text("幅")
                    .suffix("px"),
            );
            changed |= width.changed();
            width.lab_hover_tip("マスク境界から作る線の太さです。0pxでは無効です。");
            let softness = ui.add(
                egui::Slider::new(&mut params.softness_px, 0.0..=16.0)
                    .text("ぼかし")
                    .suffix("px"),
            );
            changed |= softness.changed();
            softness.lab_hover_tip(
                "線の縁を柔らかくします。ステッカー風は低め、発光前の下地は高めが向きます。",
            );
            let opacity =
                ui.add(egui::Slider::new(&mut params.opacity, 0.0..=1.0).text("不透明度"));
            changed |= opacity.changed();
            opacity.lab_hover_tip("縁取り色を元画像へ重ねる強さです。");
        }
        LocalEffect::ColorOverlay(params) => {
            ui.label(egui::RichText::new("プリセット").color(Color32::from_gray(190)));
            ui.horizontal_wrapped(|ui| {
                if preset_button(ui, "夕焼け") {
                    *params = ColorOverlayParams {
                        shape: ColorOverlayShape::Linear,
                        blend_mode: ColorOverlayBlendMode::SoftLight,
                        start_rgb: [255, 132, 48],
                        end_rgb: [78, 124, 255],
                        angle_degrees: -24.0,
                        softness: 0.65,
                        opacity: 0.58,
                        ..Default::default()
                    };
                    changed = true;
                }
                if preset_button(ui, "暖色塗り") {
                    *params = ColorOverlayParams {
                        shape: ColorOverlayShape::Solid,
                        blend_mode: ColorOverlayBlendMode::SoftLight,
                        start_rgb: [255, 170, 92],
                        opacity: 0.36,
                        ..Default::default()
                    };
                    changed = true;
                }
                if preset_button(ui, "影色乗算") {
                    *params = ColorOverlayParams {
                        shape: ColorOverlayShape::Linear,
                        blend_mode: ColorOverlayBlendMode::Multiply,
                        start_rgb: [76, 84, 148],
                        end_rgb: [255, 180, 98],
                        angle_degrees: 34.0,
                        softness: 0.45,
                        opacity: 0.32,
                        ..Default::default()
                    };
                    changed = true;
                }
                if preset_button(ui, "光の中心") {
                    *params = ColorOverlayParams {
                        shape: ColorOverlayShape::Radial,
                        blend_mode: ColorOverlayBlendMode::Screen,
                        start_rgb: [255, 236, 186],
                        end_rgb: [255, 114, 44],
                        center: [0.50, 0.36],
                        radius: 0.72,
                        softness: 0.82,
                        opacity: 0.44,
                        ..Default::default()
                    };
                    changed = true;
                }
            });
            let before_shape = params.shape;
            lab_combo_box(
                ui,
                "color_overlay_shape",
                color_overlay_shape_label(params.shape),
                |ui| {
                    for shape in [
                        ColorOverlayShape::Solid,
                        ColorOverlayShape::Linear,
                        ColorOverlayShape::Radial,
                    ] {
                        ui.selectable_value(
                            &mut params.shape,
                            shape,
                            color_overlay_shape_label(shape),
                        );
                    }
                },
            );
            changed |= params.shape != before_shape;
            let before_blend = params.blend_mode;
            lab_combo_box(
                ui,
                "color_overlay_blend_mode",
                color_overlay_blend_mode_label(params.blend_mode),
                |ui| {
                    for mode in [
                        ColorOverlayBlendMode::Normal,
                        ColorOverlayBlendMode::Multiply,
                        ColorOverlayBlendMode::Screen,
                        ColorOverlayBlendMode::Overlay,
                        ColorOverlayBlendMode::SoftLight,
                        ColorOverlayBlendMode::Color,
                    ] {
                        ui.selectable_value(
                            &mut params.blend_mode,
                            mode,
                            color_overlay_blend_mode_label(mode),
                        );
                    }
                },
            );
            changed |= params.blend_mode != before_blend;
            ui.label(
                egui::RichText::new(
                    "画像の明るさではなく画面上の位置を基準に、単色またはグラデーションの色面を合成します。",
                )
                .size(10.0)
                .color(Color32::from_gray(170)),
            );
            let color_label = if params.shape == ColorOverlayShape::Solid {
                "塗り色"
            } else {
                "開始色"
            };
            merge_rgb_color_response(
                draw_rgb_color_control(
                    ui,
                    color_label,
                    &mut params.start_rgb,
                    RgbPickTarget::ColorOverlayStart,
                    rgb_pick_active,
                ),
                &mut changed,
                &mut start_rgb_pick,
                &mut cancel_rgb_pick,
            );
            if params.shape != ColorOverlayShape::Solid {
                merge_rgb_color_response(
                    draw_rgb_color_control(
                        ui,
                        "終了色",
                        &mut params.end_rgb,
                        RgbPickTarget::ColorOverlayEnd,
                        rgb_pick_active,
                    ),
                    &mut changed,
                    &mut start_rgb_pick,
                    &mut cancel_rgb_pick,
                );
            }
            let opacity =
                ui.add(egui::Slider::new(&mut params.opacity, 0.0..=1.0).text("不透明度"));
            changed |= opacity.changed();
            opacity.lab_hover_tip("色面を合成した結果へどれだけ近づけるかです。");
            if params.shape == ColorOverlayShape::Linear {
                let angle = ui.add(
                    egui::Slider::new(&mut params.angle_degrees, -180.0..=180.0)
                        .text("角度")
                        .suffix("°"),
                );
                if angle.changed() {
                    params.linear_points_enabled = false;
                    changed = true;
                }
                angle.lab_hover_tip(
                    "線形グラデーションの方向です。0°で左から右へ色が変わります。画像上をドラッグすると開始点と終了点も設定できます。",
                );
            }
            if params.shape == ColorOverlayShape::Radial {
                let center_x =
                    ui.add(egui::Slider::new(&mut params.center[0], 0.0..=1.0).text("中心X"));
                changed |= center_x.changed();
                center_x.lab_hover_tip("円形グラデーション中心の横位置です。");
                let center_y =
                    ui.add(egui::Slider::new(&mut params.center[1], 0.0..=1.0).text("中心Y"));
                changed |= center_y.changed();
                center_y.lab_hover_tip("円形グラデーション中心の縦位置です。");
                let radius = ui.add(egui::Slider::new(&mut params.radius, 0.02..=2.0).text("半径"));
                changed |= radius.changed();
                radius.lab_hover_tip(
                    "中心色から終了色へ変わる範囲です。画像上をドラッグすると中心と半径を設定できます。",
                );
            }
            if params.shape != ColorOverlayShape::Solid {
                let softness =
                    ui.add(egui::Slider::new(&mut params.softness, 0.0..=1.0).text("なめらかさ"));
                changed |= softness.changed();
                softness
                    .lab_hover_tip("グラデーションの変化を直線的にするか、なだらかにするかです。");
            }
        }
        LocalEffect::NeonGlow(params) => {
            ui.label(egui::RichText::new("プリセット").color(Color32::from_gray(190)));
            ui.horizontal_wrapped(|ui| {
                if preset_button(ui, "シアン管") {
                    *params = NeonGlowParams {
                        threshold: 0.72,
                        by_saturation: true,
                        inner_radius_px: 5.0,
                        outer_radius_px: 34.0,
                        strength: 0.95,
                        inner_amount: 0.95,
                        outer_amount: 0.85,
                        glow_saturation: 0.85,
                        tint_rgb: [0, 220, 255],
                        tint_strength: 0.28,
                        screen_blend: true,
                        ..Default::default()
                    };
                    changed = true;
                }
                if preset_button(ui, "マゼンタ") {
                    *params = NeonGlowParams {
                        threshold: 0.68,
                        by_saturation: true,
                        inner_radius_px: 4.0,
                        outer_radius_px: 26.0,
                        strength: 0.90,
                        inner_amount: 1.0,
                        outer_amount: 0.72,
                        glow_saturation: 0.95,
                        tint_rgb: [255, 58, 210],
                        tint_strength: 0.38,
                        screen_blend: true,
                        ..Default::default()
                    };
                    changed = true;
                }
                if preset_button(ui, "広いハロー") {
                    *params = NeonGlowParams {
                        threshold: 0.50,
                        by_saturation: true,
                        inner_radius_px: 7.0,
                        outer_radius_px: 64.0,
                        strength: 0.70,
                        inner_amount: 0.62,
                        outer_amount: 1.15,
                        glow_saturation: 0.55,
                        tint_strength: 0.0,
                        screen_blend: true,
                        ..Default::default()
                    };
                    changed = true;
                }
                if preset_button(ui, "色指定") {
                    *params = NeonGlowParams {
                        threshold: 0.36,
                        by_saturation: true,
                        inner_radius_px: 5.0,
                        outer_radius_px: 24.0,
                        strength: 0.85,
                        inner_amount: 0.85,
                        outer_amount: 0.75,
                        glow_saturation: 0.45,
                        source_color_enabled: true,
                        source_rgb: [0, 220, 255],
                        source_tolerance: 0.24,
                        source_feather: 0.12,
                        tint_rgb: [0, 220, 255],
                        tint_strength: 0.18,
                        screen_blend: true,
                        ..Default::default()
                    };
                    changed = true;
                }
            });
            ui.label(
                egui::RichText::new(
                    "輝度だけでなく高彩度の色も発光源として拾い、芯のにじみと広いハローを二段で重ねます。",
                )
                .size(10.0)
                .color(Color32::from_gray(170)),
            );
            let threshold =
                ui.add(egui::Slider::new(&mut params.threshold, 0.05..=0.999).text("発光しきい値"));
            changed |= threshold.changed();
            threshold.lab_hover_tip(
                "どの明るさ/鮮やかさから発光源として拾うかです。低いほど広く光ります。",
            );
            let by_saturation = ui.checkbox(&mut params.by_saturation, "鮮やかな色も拾う");
            changed |= by_saturation.changed();
            by_saturation.lab_hover_tip(
                "ONにすると、白くないシアンやマゼンタのネオン色も発光源になります。",
            );
            let source_color = ui.checkbox(&mut params.source_color_enabled, "発光色を指定する");
            changed |= source_color.changed();
            source_color
                .lab_hover_tip("ONにすると、指定色に近い線や面だけを発光源として拾います。");
            if params.source_color_enabled {
                merge_rgb_color_response(
                    draw_rgb_color_control(
                        ui,
                        "発光源の色",
                        &mut params.source_rgb,
                        RgbPickTarget::NeonGlowSource,
                        rgb_pick_active,
                    ),
                    &mut changed,
                    &mut start_rgb_pick,
                    &mut cancel_rgb_pick,
                );
                let tolerance = ui
                    .add(egui::Slider::new(&mut params.source_tolerance, 0.0..=1.0).text("色許容"));
                changed |= tolerance.changed();
                tolerance
                    .lab_hover_tip("発光源として拾う色の近さです。低いほど指定色だけに絞ります。");
                let feather = ui.add(
                    egui::Slider::new(&mut params.source_feather, 0.001..=1.0).text("色ぼかし"),
                );
                changed |= feather.changed();
                feather.lab_hover_tip("指定色の範囲境界をどれだけなだらかにするかです。");
            }
            let inner_radius = ui.add(
                egui::Slider::new(&mut params.inner_radius_px, 0.0..=96.0)
                    .text("芯の半径")
                    .suffix("px"),
            );
            changed |= inner_radius.changed();
            inner_radius.lab_hover_tip("光源の近くに出る強いにじみの半径です。");
            let outer_radius = ui.add(
                egui::Slider::new(&mut params.outer_radius_px, 0.0..=180.0)
                    .text("ハロー半径")
                    .suffix("px"),
            );
            changed |= outer_radius.changed();
            outer_radius.lab_hover_tip("周囲へ広く漂う外側の光の半径です。");
            let strength = ui.add(egui::Slider::new(&mut params.strength, 0.0..=2.0).text("強さ"));
            changed |= strength.changed();
            strength.lab_hover_tip("内側グローと外側ハローを元画像へ重ねる強さです。");
            let inner_amount =
                ui.add(egui::Slider::new(&mut params.inner_amount, 0.0..=2.0).text("芯の強さ"));
            changed |= inner_amount.changed();
            inner_amount.lab_hover_tip("光源近くの強いグローの量です。");
            let outer_amount =
                ui.add(egui::Slider::new(&mut params.outer_amount, 0.0..=2.0).text("ハロー量"));
            changed |= outer_amount.changed();
            outer_amount.lab_hover_tip("外側へ広がる柔らかいハローの量です。");
            let glow_saturation =
                ui.add(egui::Slider::new(&mut params.glow_saturation, -1.0..=2.0).text("光の彩度"));
            changed |= glow_saturation.changed();
            glow_saturation.lab_hover_tip("光輪の色をどれだけ鮮やかにするかです。");
            merge_rgb_color_response(
                draw_rgb_color_control(
                    ui,
                    "着色",
                    &mut params.tint_rgb,
                    RgbPickTarget::NeonGlowTint,
                    rgb_pick_active,
                ),
                &mut changed,
                &mut start_rgb_pick,
                &mut cancel_rgb_pick,
            );
            let tint_strength =
                ui.add(egui::Slider::new(&mut params.tint_strength, 0.0..=1.0).text("着色量"));
            changed |= tint_strength.changed();
            tint_strength.lab_hover_tip("元の発光色から、着色で指定した色へどれだけ寄せるかです。");
            let screen_blend = ui.checkbox(&mut params.screen_blend, "スクリーン合成");
            changed |= screen_blend.changed();
            screen_blend.lab_hover_tip("ONにすると、加算より白飛びを抑えながら発光感を出します。");
        }
        LocalEffect::DiffuseGlow(params) => {
            ui.label(egui::RichText::new("プリセット").color(Color32::from_gray(190)));
            ui.horizontal_wrapped(|ui| {
                if preset_button(ui, "夢幻") {
                    *params = DiffuseGlowParams {
                        threshold: 0.48,
                        radius_px: 28.0,
                        strength: 0.75,
                        white_mix: 0.55,
                        grain: 0.28,
                        seed: params.seed,
                    };
                    changed = true;
                }
                if preset_button(ui, "淡く") {
                    *params = DiffuseGlowParams {
                        threshold: 0.62,
                        radius_px: 18.0,
                        strength: 0.42,
                        white_mix: 0.35,
                        grain: 0.12,
                        seed: params.seed,
                    };
                    changed = true;
                }
                if preset_button(ui, "粒状") {
                    *params = DiffuseGlowParams {
                        threshold: 0.42,
                        radius_px: 22.0,
                        strength: 0.85,
                        white_mix: 0.45,
                        grain: 0.75,
                        seed: params.seed,
                    };
                    changed = true;
                }
            });
            ui.label(
                egui::RichText::new(
                    "明るい部分を白く拡散し、粒状ノイズで光のにじみにムラを作ります。Bloom より柔らかい写真効果向けです。",
                )
                .size(10.0)
                .color(Color32::from_gray(170)),
            );
            let threshold =
                ui.add(egui::Slider::new(&mut params.threshold, 0.0..=0.98).text("明部しきい値"));
            changed |= threshold.changed();
            threshold
                .lab_hover_tip("光彩として拾う明るさです。低いほど広い範囲へ白い拡散が乗ります。");
            let radius = ui.add(
                egui::Slider::new(&mut params.radius_px, 0.0..=120.0)
                    .text("拡散半径")
                    .suffix("px"),
            );
            changed |= radius.changed();
            radius.lab_hover_tip("抽出した明部をどれだけ広くにじませるかです。");
            let strength = ui.add(egui::Slider::new(&mut params.strength, 0.0..=2.0).text("強さ"));
            changed |= strength.changed();
            strength.lab_hover_tip("元画像へ拡散光彩を重ねる強さです。");
            let white_mix =
                ui.add(egui::Slider::new(&mut params.white_mix, 0.0..=1.0).text("白さ"));
            changed |= white_mix.changed();
            white_mix.lab_hover_tip("光彩をどれだけ白く漂わせるかです。");
            let grain = ui.add(egui::Slider::new(&mut params.grain, 0.0..=1.0).text("粒状感"));
            changed |= grain.changed();
            grain.lab_hover_tip("光彩と明部に加える粒状ノイズの量です。");
            let mut seed = params.seed as i32;
            let seed_response = ui.add(egui::Slider::new(&mut seed, 0..=9999).text("seed"));
            changed |= seed_response.changed();
            seed_response.lab_hover_tip("粒状ノイズのパターンを変えます。");
            params.seed = seed.max(0) as u32;
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
        LocalEffect::GodRays(params) => {
            ui.label(egui::RichText::new("プリセット").color(Color32::from_gray(190)));
            ui.horizontal_wrapped(|ui| {
                if preset_button(ui, "木漏れ日") {
                    *params = GodRaysParams {
                        center: [0.38, 0.06],
                        threshold: 0.74,
                        length_px: 150.0,
                        decay: 0.88,
                        strength: 0.95,
                        warm_tint: 0.28,
                    };
                    changed = true;
                }
                if preset_button(ui, "舞台光") {
                    *params = GodRaysParams {
                        center: [0.50, 0.00],
                        threshold: 0.68,
                        length_px: 220.0,
                        decay: 0.82,
                        strength: 1.25,
                        warm_tint: 0.12,
                    };
                    changed = true;
                }
                if preset_button(ui, "夕日") {
                    *params = GodRaysParams {
                        center: [0.12, 0.22],
                        threshold: 0.70,
                        length_px: 190.0,
                        decay: 0.90,
                        strength: 1.10,
                        warm_tint: 0.55,
                    };
                    changed = true;
                }
            });
            ui.label(
                egui::RichText::new(
                    "明るい部分を拾い、光源中心から外側へ伸びる放射状の光芒を作ります。",
                )
                .size(10.0)
                .color(Color32::from_gray(170)),
            );
            changed |= draw_effect_center_controls(
                ui,
                &mut params.center,
                "光が差し込む中心の横位置です。",
                "光が差し込む中心の縦位置です。",
                effect_position_handles_visible,
                &mut set_effect_position_handles_visible,
            );
            changed |= ui
                .add(
                    egui::Slider::new(&mut params.threshold, 0.0..=0.98)
                        .text("明部しきい値")
                        .fixed_decimals(3),
                )
                .changed();
            changed |= ui
                .add(egui::Slider::new(&mut params.length_px, 1.0..=360.0).text("光芒長"))
                .changed();
            changed |= ui
                .add(egui::Slider::new(&mut params.decay, 0.0..=1.0).text("減衰"))
                .changed();
            changed |= ui
                .add(egui::Slider::new(&mut params.strength, 0.0..=3.0).text("強さ"))
                .changed();
            changed |= ui
                .add(egui::Slider::new(&mut params.warm_tint, 0.0..=1.0).text("暖色"))
                .changed();
        }
        LocalEffect::LensFlare(params) => {
            ui.label(egui::RichText::new("プリセット").color(Color32::from_gray(190)));
            ui.horizontal_wrapped(|ui| {
                if preset_button(ui, "逆光") {
                    *params = LensFlareParams {
                        center: [0.78, 0.20],
                        radius_px: 120.0,
                        strength: 0.90,
                        core_strength: 1.0,
                        halo_strength: 0.85,
                        ghost_strength: 0.75,
                        streak_strength: 0.35,
                        warm_tint: 0.45,
                    };
                    changed = true;
                }
                if preset_button(ui, "シネマ") {
                    *params = LensFlareParams {
                        center: [0.12, 0.36],
                        radius_px: 150.0,
                        strength: 0.80,
                        core_strength: 0.75,
                        halo_strength: 0.45,
                        ghost_strength: 0.65,
                        streak_strength: 1.10,
                        warm_tint: 0.08,
                    };
                    changed = true;
                }
                if preset_button(ui, "柔らかい") {
                    *params = LensFlareParams {
                        center: [0.66, 0.18],
                        radius_px: 170.0,
                        strength: 0.55,
                        core_strength: 0.55,
                        halo_strength: 1.15,
                        ghost_strength: 0.35,
                        streak_strength: 0.10,
                        warm_tint: 0.30,
                    };
                    changed = true;
                }
            });
            ui.label(
                egui::RichText::new(
                    "指定した光源から、にじみ、薄いリング、レンズ内反射のゴーストを作ります。",
                )
                .size(10.0)
                .color(Color32::from_gray(170)),
            );
            changed |= draw_effect_center_controls(
                ui,
                &mut params.center,
                "フレア光源の横位置です。",
                "フレア光源の縦位置です。",
                effect_position_handles_visible,
                &mut set_effect_position_handles_visible,
            );
            changed |= ui
                .add(egui::Slider::new(&mut params.radius_px, 4.0..=420.0).text("範囲"))
                .changed();
            changed |= ui
                .add(egui::Slider::new(&mut params.strength, 0.0..=3.0).text("強さ"))
                .changed();
            changed |= ui
                .add(egui::Slider::new(&mut params.core_strength, 0.0..=2.0).text("コア"))
                .changed();
            changed |= ui
                .add(egui::Slider::new(&mut params.halo_strength, 0.0..=2.0).text("ハロー"))
                .changed();
            changed |= ui
                .add(egui::Slider::new(&mut params.ghost_strength, 0.0..=2.0).text("ゴースト"))
                .changed();
            changed |= ui
                .add(egui::Slider::new(&mut params.streak_strength, 0.0..=2.0).text("光条"))
                .changed();
            changed |= ui
                .add(egui::Slider::new(&mut params.warm_tint, 0.0..=1.0).text("暖色"))
                .changed();
        }
        LocalEffect::CloudFog(params) => {
            ui.label(egui::RichText::new("プリセット").color(Color32::from_gray(190)));
            ui.horizontal_wrapped(|ui| {
                if preset_button(ui, "薄霧") {
                    *params = CloudFogParams {
                        mode: CloudFogMode::Fog,
                        scale_px: 220.0,
                        detail: 0.35,
                        density: 0.42,
                        contrast: 0.16,
                        height_fade: 0.35,
                        opacity: 0.30,
                        color_rgb: [235, 244, 255],
                        seed: params.seed,
                    };
                    changed = true;
                }
                if preset_button(ui, "濃霧") {
                    *params = CloudFogParams {
                        mode: CloudFogMode::Fog,
                        scale_px: 150.0,
                        detail: 0.48,
                        density: 0.78,
                        contrast: 0.10,
                        height_fade: 0.08,
                        opacity: 0.58,
                        color_rgb: [232, 238, 246],
                        seed: params.seed,
                    };
                    changed = true;
                }
                if preset_button(ui, "雲") {
                    *params = CloudFogParams {
                        mode: CloudFogMode::Clouds,
                        scale_px: 96.0,
                        detail: 0.78,
                        density: 0.66,
                        contrast: 0.62,
                        height_fade: 0.0,
                        opacity: 0.72,
                        color_rgb: [255, 255, 255],
                        seed: params.seed,
                    };
                    changed = true;
                }
                if preset_button(ui, "夕霧") {
                    *params = CloudFogParams {
                        mode: CloudFogMode::Fog,
                        scale_px: 190.0,
                        detail: 0.42,
                        density: 0.55,
                        contrast: 0.22,
                        height_fade: 0.22,
                        opacity: 0.42,
                        color_rgb: [255, 220, 176],
                        seed: params.seed,
                    };
                    changed = true;
                }
            });
            ui.label(
                egui::RichText::new(
                    "手続き型のノイズで霧や雲を重ねます。マスクと組み合わせて遠景や背景に大気感を足せます。",
                )
                .size(10.0)
                .color(Color32::from_gray(170)),
            );
            ui.horizontal_wrapped(|ui| {
                let fog = params.mode == CloudFogMode::Fog;
                if ui.selectable_label(fog, "霧").clicked() && !fog {
                    params.mode = CloudFogMode::Fog;
                    changed = true;
                }
                let clouds = params.mode == CloudFogMode::Clouds;
                if ui.selectable_label(clouds, "雲").clicked() && !clouds {
                    params.mode = CloudFogMode::Clouds;
                    changed = true;
                }
            });
            let scale =
                ui.add(egui::Slider::new(&mut params.scale_px, 8.0..=640.0).text("スケール"));
            changed |= scale.changed();
            scale.lab_hover_tip("ノイズの大きさです。大きいほど広くなだらかな霧になります。");
            let detail = ui.add(egui::Slider::new(&mut params.detail, 0.0..=1.0).text("細部"));
            changed |= detail.changed();
            detail.lab_hover_tip("細かい揺らぎを足します。雲では高め、薄霧では低めが向きます。");
            let density = ui.add(egui::Slider::new(&mut params.density, 0.0..=1.0).text("密度"));
            changed |= density.changed();
            density.lab_hover_tip("霧や雲が画面を覆う量です。");
            let contrast =
                ui.add(egui::Slider::new(&mut params.contrast, 0.0..=1.0).text("コントラスト"));
            changed |= contrast.changed();
            contrast.lab_hover_tip("雲の濃淡差です。霧では低めにすると自然です。");
            let height_fade =
                ui.add(egui::Slider::new(&mut params.height_fade, -1.0..=1.0).text("上下フェード"));
            changed |= height_fade.changed();
            height_fade.lab_hover_tip("正の値で上側、負の値で下側に霧や雲を寄せます。");
            let opacity =
                ui.add(egui::Slider::new(&mut params.opacity, 0.0..=1.0).text("不透明度"));
            changed |= opacity.changed();
            opacity.lab_hover_tip("元画像から霧/雲の色へどれだけ近づけるかです。");
            merge_rgb_color_response(
                draw_rgb_color_control(
                    ui,
                    "色",
                    &mut params.color_rgb,
                    RgbPickTarget::CloudFogColor,
                    rgb_pick_active,
                ),
                &mut changed,
                &mut start_rgb_pick,
                &mut cancel_rgb_pick,
            );
            let mut seed = params.seed as i32;
            let seed_response = ui.add(egui::Slider::new(&mut seed, 0..=9999).text("seed"));
            changed |= seed_response.changed();
            seed_response.lab_hover_tip("霧や雲のパターンを変えます。");
            params.seed = seed.max(0) as u32;
        }
        LocalEffect::Spotlight(params) => {
            ui.label(egui::RichText::new("プリセット").color(Color32::from_gray(190)));
            ui.horizontal_wrapped(|ui| {
                if preset_button(ui, "主役ライト") {
                    *params = SpotlightParams {
                        center: [0.50, 0.42],
                        radius: 0.24,
                        feather: 0.36,
                        light_strength: 0.75,
                        shadow_strength: 0.38,
                        tint_rgb: [255, 238, 200],
                        tint_strength: 0.22,
                    };
                    changed = true;
                }
                if preset_button(ui, "舞台") {
                    *params = SpotlightParams {
                        center: [0.50, 0.24],
                        radius: 0.18,
                        feather: 0.30,
                        light_strength: 1.05,
                        shadow_strength: 0.62,
                        tint_rgb: [255, 244, 220],
                        tint_strength: 0.18,
                    };
                    changed = true;
                }
                if preset_button(ui, "夕光") {
                    *params = SpotlightParams {
                        center: [0.28, 0.32],
                        radius: 0.30,
                        feather: 0.42,
                        light_strength: 0.65,
                        shadow_strength: 0.30,
                        tint_rgb: [255, 190, 118],
                        tint_strength: 0.42,
                    };
                    changed = true;
                }
                if preset_button(ui, "暗転") {
                    *params = SpotlightParams {
                        center: [0.50, 0.50],
                        radius: 0.26,
                        feather: 0.28,
                        light_strength: 0.20,
                        shadow_strength: 0.78,
                        tint_rgb: [230, 240, 255],
                        tint_strength: 0.08,
                    };
                    changed = true;
                }
            });
            ui.label(
                egui::RichText::new(
                    "指定中心を照らし、周辺を落として視線誘導や舞台照明のような局所光を作ります。",
                )
                .size(10.0)
                .color(Color32::from_gray(170)),
            );
            changed |= draw_effect_center_controls(
                ui,
                &mut params.center,
                "ライト中心の横位置です。",
                "ライト中心の縦位置です。",
                effect_position_handles_visible,
                &mut set_effect_position_handles_visible,
            );
            let radius = ui.add(egui::Slider::new(&mut params.radius, 0.0..=1.0).text("半径"));
            changed |= radius.changed();
            radius.lab_hover_tip("明るい中心部の大きさです。");
            let feather =
                ui.add(egui::Slider::new(&mut params.feather, 0.001..=1.0).text("ぼかし"));
            changed |= feather.changed();
            feather.lab_hover_tip("中心から外側へのなだらかさです。");
            let light = ui
                .add(egui::Slider::new(&mut params.light_strength, -1.0..=2.0).text("中心明るさ"));
            changed |= light.changed();
            light.lab_hover_tip("正の値で中心を明るく、負の値で中心を暗くします。");
            let shadow =
                ui.add(egui::Slider::new(&mut params.shadow_strength, 0.0..=1.0).text("周辺影"));
            changed |= shadow.changed();
            shadow.lab_hover_tip("スポット外側を暗く落とす強さです。");
            let tint = ui.add(egui::Slider::new(&mut params.tint_strength, 0.0..=1.0).text("光色"));
            changed |= tint.changed();
            tint.lab_hover_tip("中心部へ指定色をどれだけ混ぜるかです。");
            merge_rgb_color_response(
                draw_rgb_color_control(
                    ui,
                    "光色",
                    &mut params.tint_rgb,
                    RgbPickTarget::SpotlightTint,
                    rgb_pick_active,
                ),
                &mut changed,
                &mut start_rgb_pick,
                &mut cancel_rgb_pick,
            );
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
        LocalEffect::Noise(params) => {
            ui.label(egui::RichText::new("プリセット").color(Color32::from_gray(190)));
            ui.horizontal_wrapped(|ui| {
                if preset_button(ui, "微量") {
                    *params = NoiseParams {
                        amount: 0.08,
                        distribution: NoiseDistribution::Uniform,
                        monochrome: true,
                        seed: params.seed,
                    };
                    changed = true;
                }
                if preset_button(ui, "ガウス") {
                    *params = NoiseParams {
                        amount: 0.22,
                        distribution: NoiseDistribution::Gaussian,
                        monochrome: true,
                        seed: params.seed,
                    };
                    changed = true;
                }
                if preset_button(ui, "カラー") {
                    *params = NoiseParams {
                        amount: 0.18,
                        distribution: NoiseDistribution::Uniform,
                        monochrome: false,
                        seed: params.seed,
                    };
                    changed = true;
                }
            });
            changed |= ui
                .add(egui::Slider::new(&mut params.amount, 0.0..=1.0).text("量"))
                .changed();
            let previous_distribution = params.distribution;
            ComboBox::from_label("分布")
                .selected_text(noise_distribution_label(params.distribution))
                .show_ui(ui, |ui| {
                    ui.selectable_value(
                        &mut params.distribution,
                        NoiseDistribution::Uniform,
                        "均一",
                    );
                    ui.selectable_value(
                        &mut params.distribution,
                        NoiseDistribution::Gaussian,
                        "ガウス",
                    );
                });
            changed |= params.distribution != previous_distribution;
            changed |= ui.checkbox(&mut params.monochrome, "単色ノイズ").changed();
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
        LocalEffect::ScreenTone(params) => {
            ui.label(egui::RichText::new("プリセット").color(Color32::from_gray(190)));
            ui.horizontal_wrapped(|ui| {
                if preset_button(ui, "網点") {
                    *params = ScreenToneParams {
                        mode: ScreenToneMode::Dots,
                        cell_px: 8.0,
                        angle_degrees: 45.0,
                        density: 0.60,
                        gradation: 0.60,
                        softness: 0.08,
                        strength: 0.75,
                    };
                    changed = true;
                }
                if preset_button(ui, "細線") {
                    *params = ScreenToneParams {
                        mode: ScreenToneMode::Lines,
                        cell_px: 6.0,
                        angle_degrees: -35.0,
                        density: 0.34,
                        gradation: 0.35,
                        softness: 0.03,
                        strength: 0.70,
                    };
                    changed = true;
                }
                if preset_button(ui, "カケアミ") {
                    *params = ScreenToneParams {
                        mode: ScreenToneMode::CrossHatch,
                        cell_px: 8.0,
                        angle_degrees: 30.0,
                        density: 0.55,
                        gradation: 0.45,
                        softness: 0.02,
                        strength: 0.80,
                    };
                    changed = true;
                }
                if preset_button(ui, "淡い背景") {
                    *params = ScreenToneParams {
                        mode: ScreenToneMode::Dots,
                        cell_px: 12.0,
                        angle_degrees: 45.0,
                        density: 0.26,
                        gradation: 0.0,
                        softness: 0.10,
                        strength: 0.55,
                    };
                    changed = true;
                }
            });
            ui.label(
                egui::RichText::new(
                    "漫画用のトーンです。階調追従を下げると均一なトーン、上げると元画像の明暗に沿ったトーンになります。",
                )
                .size(10.0)
                .color(Color32::from_gray(170)),
            );
            ui.horizontal_wrapped(|ui| {
                changed |= ui
                    .selectable_value(&mut params.mode, ScreenToneMode::Dots, "網点")
                    .changed();
                changed |= ui
                    .selectable_value(&mut params.mode, ScreenToneMode::Lines, "線")
                    .changed();
                changed |= ui
                    .selectable_value(&mut params.mode, ScreenToneMode::CrossHatch, "カケアミ")
                    .changed();
            });
            changed |= ui
                .add(egui::Slider::new(&mut params.cell_px, 2.0..=128.0).text("セル(px)"))
                .changed();
            changed |= ui
                .add(egui::Slider::new(&mut params.angle_degrees, -180.0..=180.0).text("角度"))
                .changed();
            changed |= ui
                .add(egui::Slider::new(&mut params.density, 0.0..=1.0).text("濃度"))
                .changed();
            changed |= ui
                .add(egui::Slider::new(&mut params.gradation, 0.0..=1.0).text("階調追従"))
                .changed();
            changed |= ui
                .add(egui::Slider::new(&mut params.softness, 0.0..=1.0).text("柔らかさ"))
                .changed();
            changed |= ui
                .add(egui::Slider::new(&mut params.strength, 0.0..=1.0).text("強度"))
                .changed();
        }
        LocalEffect::ColorHalftone(params) => {
            ui.label(egui::RichText::new("プリセット").color(Color32::from_gray(190)));
            ui.horizontal_wrapped(|ui| {
                if preset_button(ui, "ポップ") {
                    *params = ColorHalftoneParams {
                        cell_px: 8.0,
                        angle_offset_degrees: 0.0,
                        dot_gain: 0.10,
                        black_generation: 0.55,
                        softness: 0.03,
                        strength: 0.85,
                    };
                    changed = true;
                }
                if preset_button(ui, "粗い印刷") {
                    *params = ColorHalftoneParams {
                        cell_px: 16.0,
                        angle_offset_degrees: 0.0,
                        dot_gain: 0.06,
                        black_generation: 0.80,
                        softness: 0.0,
                        strength: 0.80,
                    };
                    changed = true;
                }
                if preset_button(ui, "淡いCMYK") {
                    *params = ColorHalftoneParams {
                        cell_px: 11.0,
                        angle_offset_degrees: 0.0,
                        dot_gain: -0.08,
                        black_generation: 0.45,
                        softness: 0.10,
                        strength: 0.60,
                    };
                    changed = true;
                }
            });
            ui.label(
                egui::RichText::new(
                    "CMYKの4版を角度違いのドットにします。ドット増減を上げるとインクが太り、印刷物らしい粗さが出ます。",
                )
                .size(10.0)
                .color(Color32::from_gray(170)),
            );
            changed |= ui
                .add(egui::Slider::new(&mut params.cell_px, 3.0..=160.0).text("セル(px)"))
                .changed();
            changed |= ui
                .add(
                    egui::Slider::new(&mut params.angle_offset_degrees, -180.0..=180.0)
                        .text("角度オフセット"),
                )
                .changed();
            changed |= ui
                .add(egui::Slider::new(&mut params.dot_gain, -0.5..=0.5).text("ドット増減"))
                .changed();
            changed |= ui
                .add(egui::Slider::new(&mut params.black_generation, 0.0..=1.0).text("黒版量"))
                .changed();
            changed |= ui
                .add(egui::Slider::new(&mut params.softness, 0.0..=1.0).text("柔らかさ"))
                .changed();
            changed |= ui
                .add(egui::Slider::new(&mut params.strength, 0.0..=1.0).text("強度"))
                .changed();
        }
        LocalEffect::Textureizer(params) => {
            ui.label(egui::RichText::new("プリセット").color(Color32::from_gray(190)));
            ui.horizontal_wrapped(|ui| {
                if preset_button(ui, "紙目") {
                    *params = TextureizerParams {
                        mode: TextureizerMode::Paper,
                        scale_px: 9.0,
                        depth: 0.55,
                        contrast: 1.05,
                        warmth: 0.22,
                        strength: 0.60,
                        seed: params.seed,
                    };
                    changed = true;
                }
                if preset_button(ui, "キャンバス") {
                    *params = TextureizerParams {
                        mode: TextureizerMode::Canvas,
                        scale_px: 7.0,
                        depth: 0.60,
                        contrast: 1.20,
                        warmth: 0.10,
                        strength: 0.65,
                        seed: params.seed,
                    };
                    changed = true;
                }
                if preset_button(ui, "リネン") {
                    *params = TextureizerParams {
                        mode: TextureizerMode::Linen,
                        scale_px: 8.0,
                        depth: 0.50,
                        contrast: 1.15,
                        warmth: 0.16,
                        strength: 0.58,
                        seed: params.seed,
                    };
                    changed = true;
                }
                if preset_button(ui, "冷たい紙目") {
                    *params = TextureizerParams {
                        mode: TextureizerMode::Paper,
                        scale_px: 12.0,
                        depth: 0.42,
                        contrast: 0.90,
                        warmth: -0.22,
                        strength: 0.50,
                        seed: params.seed,
                    };
                    changed = true;
                }
            });
            ui.label(
                egui::RichText::new(
                    "手続き型の紙目や織り目をソフトライトで重ねます。フィルム粒子より大きな面の質感向けです。",
                )
                .size(10.0)
                .color(Color32::from_gray(170)),
            );
            ui.horizontal_wrapped(|ui| {
                changed |= ui
                    .selectable_value(&mut params.mode, TextureizerMode::Paper, "紙目")
                    .changed();
                changed |= ui
                    .selectable_value(&mut params.mode, TextureizerMode::Canvas, "キャンバス")
                    .changed();
                changed |= ui
                    .selectable_value(&mut params.mode, TextureizerMode::Linen, "リネン")
                    .changed();
            });
            changed |= ui
                .add(egui::Slider::new(&mut params.scale_px, 2.0..=96.0).text("スケール(px)"))
                .changed();
            changed |= ui
                .add(egui::Slider::new(&mut params.depth, 0.0..=1.0).text("凹凸"))
                .changed();
            changed |= ui
                .add(egui::Slider::new(&mut params.contrast, 0.0..=2.0).text("コントラスト"))
                .changed();
            changed |= ui
                .add(egui::Slider::new(&mut params.warmth, -1.0..=1.0).text("紙色"))
                .changed();
            changed |= ui
                .add(egui::Slider::new(&mut params.strength, 0.0..=1.0).text("強度"))
                .changed();
            let mut seed = params.seed as i32;
            changed |= ui
                .add(egui::Slider::new(&mut seed, 0..=9999).text("seed"))
                .changed();
            params.seed = seed.max(0) as u32;
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
        LocalEffect::Despeckle(params) => {
            ui.label(egui::RichText::new("プリセット").color(Color32::from_gray(190)));
            ui.horizontal_wrapped(|ui| {
                if preset_button(ui, "点ゴミ") {
                    *params = DespeckleParams {
                        radius_px: 1.0,
                        threshold: 42.0,
                        strength: 1.0,
                    };
                    changed = true;
                }
                if preset_button(ui, "スキャン補修") {
                    *params = DespeckleParams {
                        radius_px: 2.0,
                        threshold: 34.0,
                        strength: 0.75,
                    };
                    changed = true;
                }
                if preset_button(ui, "控えめ") {
                    *params = DespeckleParams {
                        radius_px: 1.0,
                        threshold: 70.0,
                        strength: 0.55,
                    };
                    changed = true;
                }
            });
            ui.label(
                egui::RichText::new(
                    "周囲から大きく外れた孤立点だけを中央値へ寄せます。通常のメディアンより線や面を残しやすい点ゴミ除去です。",
                )
                .size(10.0)
                .color(Color32::from_gray(170)),
            );
            let radius = ui.add(egui::Slider::new(&mut params.radius_px, 1.0..=4.0).text("半径"));
            changed |= radius.changed();
            radius.lab_hover_tip(
                "周囲を調べる範囲です。1px は白点・黒点、2px 以上は小さなゴミ向けです。",
            );
            let threshold =
                ui.add(egui::Slider::new(&mut params.threshold, 1.0..=160.0).text("検出しきい値"));
            changed |= threshold.changed();
            threshold.lab_hover_tip(
                "中心画素が周囲の中央値からどれだけ外れたら補修するかです。小さいほど多く補修します。",
            );
            let strength = ui.add(egui::Slider::new(&mut params.strength, 0.0..=1.0).text("強さ"));
            changed |= strength.changed();
            strength.lab_hover_tip("元画像から補修後の色へどれだけ近づけるかです。");
        }
        LocalEffect::Median(params) => {
            ui.label(egui::RichText::new("プリセット").color(Color32::from_gray(190)));
            ui.horizontal_wrapped(|ui| {
                if preset_button(ui, "点ノイズ") {
                    *params = MedianParams {
                        radius_px: 1.0,
                        strength: 1.0,
                    };
                    changed = true;
                }
                if preset_button(ui, "弱く") {
                    *params = MedianParams {
                        radius_px: 1.0,
                        strength: 0.45,
                    };
                    changed = true;
                }
                if preset_button(ui, "強め") {
                    *params = MedianParams {
                        radius_px: 2.0,
                        strength: 0.85,
                    };
                    changed = true;
                }
            });
            ui.label(
                egui::RichText::new(
                    "周囲の中央値に置き換えることで、孤立した白点・黒点や細かいゴミを落とします。線や細部も丸まりやすいので小さめの半径から試してください。",
                )
                .size(10.0)
                .color(Color32::from_gray(170)),
            );
            let radius = ui.add(egui::Slider::new(&mut params.radius_px, 0.0..=8.0).text("半径"));
            changed |= radius.changed();
            radius.lab_hover_tip(
                "中央値を取る範囲です。1pxは点ノイズ除去向け、大きい値は細部も消えやすくなります。",
            );
            let strength = ui.add(egui::Slider::new(&mut params.strength, 0.0..=1.0).text("強さ"));
            changed |= strength.changed();
            strength.lab_hover_tip("元画像から中央値処理後の色へどれだけ近づけるかです。");
        }
    }
    EffectParamResponse {
        changed,
        load_cube_lut,
        start_selective_color_pick,
        cancel_selective_color_pick,
        start_rgb_pick,
        cancel_rgb_pick,
        set_effect_position_handles_visible,
    }
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
        MaskKind::Subject => LocalMask::Subject(SubjectMask::empty(width, height)),
        MaskKind::Segmentation => LocalMask::Segmentation(RegionMask::empty(width, height)),
    }
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

#[derive(Debug, Clone, Copy, PartialEq)]
struct SubjectMaskStats {
    foreground_percent: f32,
    soft_percent: f32,
}

fn subject_mask_stats(mask: &SubjectMask) -> SubjectMaskStats {
    if mask.alpha.is_empty() {
        return SubjectMaskStats {
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
    SubjectMaskStats {
        foreground_percent: foreground as f32 * 100.0 / total,
        soft_percent: soft as f32 * 100.0 / total,
    }
}

fn subject_mask_has_content(mask: &SubjectMask) -> bool {
    mask.alpha.iter().any(|&alpha| alpha > 0.02)
        || mask
            .source_alpha
            .as_ref()
            .is_some_and(|alpha| alpha.iter().any(|&value| value > 0.02))
}

fn subject_cutout_refined_alpha(
    mask: &RasterMask,
    threshold: f32,
    expand_px: i32,
    feather_px: usize,
) -> Vec<f32> {
    let mut alpha: Vec<f32> = mask
        .alpha
        .iter()
        .map(|&value| {
            if value.clamp(0.0, 1.0) >= threshold.clamp(0.0, 1.0) {
                1.0
            } else {
                0.0
            }
        })
        .collect();
    let steps = expand_px.unsigned_abs().min(16);
    for _ in 0..steps {
        alpha = if expand_px >= 0 {
            dilate_alpha(&alpha, mask.width, mask.height)
        } else {
            erode_alpha(&alpha, mask.width, mask.height)
        };
    }
    if feather_px > 0 {
        alpha = box_blur_alpha_local(&alpha, mask.width, mask.height, feather_px.min(16));
    }
    alpha
}

fn box_blur_alpha_local(src: &[f32], width: usize, height: usize, radius: usize) -> Vec<f32> {
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
        EffectKind::RgbToneCurve => LocalEffect::RgbToneCurve(RgbToneCurveParams::default()),
        EffectKind::ColorBalance => LocalEffect::ColorBalance(ColorBalanceParams::default()),
        EffectKind::ThreeWayColorGrading => {
            LocalEffect::ThreeWayColorGrading(ThreeWayColorGradingParams::default())
        }
        EffectKind::SelectiveColor => LocalEffect::SelectiveColor(SelectiveColorParams::default()),
        EffectKind::ChannelMixer => LocalEffect::ChannelMixer(ChannelMixerParams::default()),
        EffectKind::Clarity => LocalEffect::Clarity(ClarityParams::default()),
        EffectKind::Texture => LocalEffect::Texture(TextureParams::default()),
        EffectKind::HighPass => LocalEffect::HighPass(HighPassParams::default()),
        EffectKind::HighlightsShadows => {
            LocalEffect::HighlightsShadows(HighlightsShadowsParams::default())
        }
        EffectKind::Dehaze => LocalEffect::Dehaze(DehazeParams::default()),
        EffectKind::Blur => LocalEffect::Blur(BlurParams::default()),
        EffectKind::MotionBlur => LocalEffect::MotionBlur(MotionBlurParams::default()),
        EffectKind::Wind => LocalEffect::Wind(WindParams::default()),
        EffectKind::SpeedLines => LocalEffect::SpeedLines(SpeedLinesParams::default()),
        EffectKind::TiltShift => LocalEffect::TiltShift(TiltShiftParams::default()),
        EffectKind::LensBlur => LocalEffect::LensBlur(LensBlurParams::default()),
        EffectKind::RadialBlur => LocalEffect::RadialBlur(RadialBlurParams::default()),
        EffectKind::WaveDistortion => LocalEffect::WaveDistortion(WaveDistortionParams::default()),
        EffectKind::PinchSpherize => LocalEffect::PinchSpherize(PinchSpherizeParams::default()),
        EffectKind::Twirl => LocalEffect::Twirl(TwirlParams::default()),
        EffectKind::PolarCoordinates => {
            LocalEffect::PolarCoordinates(PolarCoordinatesParams::default())
        }
        EffectKind::GlassDisplacement => {
            LocalEffect::GlassDisplacement(GlassDisplacementParams::default())
        }
        EffectKind::LensCorrection => LocalEffect::LensCorrection(LensCorrectionParams::default()),
        EffectKind::LineExtract => LocalEffect::LineExtract(LineExtractParams::default()),
        EffectKind::ArtisticMedia => LocalEffect::ArtisticMedia(ArtisticMediaParams::default()),
        EffectKind::BrushStroke => LocalEffect::BrushStroke(BrushStrokeParams::default()),
        EffectKind::Cutout => LocalEffect::Cutout(CutoutParams::default()),
        EffectKind::Emboss => LocalEffect::Emboss(EmbossParams::default()),
        EffectKind::PixelStylize => LocalEffect::PixelStylize(PixelStylizeParams::default()),
        EffectKind::Solarize => LocalEffect::Solarize(SolarizeParams::default()),
        EffectKind::GlowingEdges => LocalEffect::GlowingEdges(GlowingEdgesParams::default()),
        EffectKind::OilPaint => LocalEffect::OilPaint(OilPaintParams::default()),
        EffectKind::SoftFocus => LocalEffect::SoftFocus(SoftFocusParams::default()),
        EffectKind::Mosaic => LocalEffect::Mosaic(MosaicParams::default()),
        EffectKind::Sharpen => LocalEffect::Sharpen(SharpenParams::default()),
        EffectKind::SmartSharpen => LocalEffect::SmartSharpen(SmartSharpenParams::default()),
        EffectKind::Hsl => LocalEffect::Hsl(HslParams::default()),
        EffectKind::ColorMixer => LocalEffect::ColorMixer(ColorMixerParams::default()),
        EffectKind::Look => LocalEffect::Look(LookParams::default()),
        EffectKind::CubeLut => LocalEffect::CubeLut(CubeLutParams::default()),
        EffectKind::Posterize => LocalEffect::Posterize(PosterizeParams::default()),
        EffectKind::Threshold => LocalEffect::Threshold(ThresholdParams::default()),
        EffectKind::Invert => LocalEffect::Invert(InvertParams::default()),
        EffectKind::Duotone => LocalEffect::Duotone(DuotoneParams::default()),
        EffectKind::Equalize => LocalEffect::Equalize(EqualizeParams::default()),
        EffectKind::GradientMap => LocalEffect::GradientMap(GradientMapParams::default()),
        EffectKind::ColorFill => LocalEffect::ColorFill(ColorFillParams::default()),
        EffectKind::OutlineStroke => LocalEffect::OutlineStroke(OutlineStrokeParams::default()),
        EffectKind::ColorOverlay => LocalEffect::ColorOverlay(ColorOverlayParams::default()),
        EffectKind::NeonGlow => LocalEffect::NeonGlow(NeonGlowParams::default()),
        EffectKind::DiffuseGlow => LocalEffect::DiffuseGlow(DiffuseGlowParams::default()),
        EffectKind::Bloom => LocalEffect::Bloom(BloomParams::default()),
        EffectKind::GodRays => LocalEffect::GodRays(GodRaysParams::default()),
        EffectKind::LensFlare => LocalEffect::LensFlare(LensFlareParams::default()),
        EffectKind::CloudFog => LocalEffect::CloudFog(CloudFogParams::default()),
        EffectKind::Spotlight => LocalEffect::Spotlight(SpotlightParams::default()),
        EffectKind::Vignette => LocalEffect::Vignette(VignetteParams::default()),
        EffectKind::FilmGrain => LocalEffect::FilmGrain(FilmGrainParams::default()),
        EffectKind::Noise => LocalEffect::Noise(NoiseParams::default()),
        EffectKind::ChromaticAberration => {
            LocalEffect::ChromaticAberration(ChromaticAberrationParams::default())
        }
        EffectKind::Halftone => LocalEffect::Halftone(HalftoneParams::default()),
        EffectKind::ScreenTone => LocalEffect::ScreenTone(ScreenToneParams::default()),
        EffectKind::ColorHalftone => LocalEffect::ColorHalftone(ColorHalftoneParams::default()),
        EffectKind::Textureizer => LocalEffect::Textureizer(TextureizerParams::default()),
        EffectKind::StarGlow => LocalEffect::StarGlow(StarGlowParams::default()),
        EffectKind::EdgeSmooth => LocalEffect::EdgeSmooth(EdgeSmoothParams::default()),
        EffectKind::Despeckle => LocalEffect::Despeckle(DespeckleParams::default()),
        EffectKind::Median => LocalEffect::Median(MedianParams::default()),
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
    scope: RegionSegmentationScope,
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
        if !region_seed_allowed(source, subject, scope, &boundary, start) {
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
                if !region_seed_allowed(source, subject, scope, &boundary, nidx) {
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
        .map(|idx| region_membership_allowed(source, subject, scope, idx))
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
    scope: RegionSegmentationScope,
    boundary: &[u8],
    idx: usize,
) -> bool {
    if boundary.get(idx).copied().unwrap_or(0) != 0 {
        return false;
    }
    region_membership_allowed(source, subject, scope, idx)
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
    scope: RegionSegmentationScope,
    idx: usize,
) -> bool {
    if source.pixels.get(idx * 4 + 3).copied().unwrap_or(255) < 8 {
        return false;
    }
    match scope {
        RegionSegmentationScope::Full => true,
        RegionSegmentationScope::Subject => subject
            .map(|mask| mask.alpha.get(idx).copied().unwrap_or(0.0) > 0.18)
            .unwrap_or(false),
        RegionSegmentationScope::Background => subject
            .map(|mask| mask.alpha.get(idx).copied().unwrap_or(0.0) <= 0.18)
            .unwrap_or(false),
    }
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

fn subject_model_available() -> bool {
    segmentation_model_path().is_file()
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

fn alpha_to_u8(alpha: &[f32]) -> Vec<u8> {
    alpha
        .iter()
        .map(|v| (v.clamp(0.0, 1.0) * 255.0).round() as u8)
        .collect()
}

fn stored_soft_mask_from_mask(mask: &SubjectMask) -> Result<StoredSoftMask, String> {
    let alpha_u8 = alpha_to_u8(&mask.alpha);
    let source_alpha_u8_deflate_b64 = mask
        .source_alpha
        .as_ref()
        .map(|alpha| deflate_b64(&alpha_to_u8(alpha)))
        .transpose()?;
    Ok(StoredSoftMask {
        width: mask.width,
        height: mask.height,
        alpha_u8_deflate_b64: deflate_b64(&alpha_u8)?,
        source_alpha_u8_deflate_b64,
        refinement: mask.refinement,
    })
}

fn soft_mask_from_stored(stored: &StoredSoftMask) -> Result<SubjectMask, String> {
    let len = stored.width.saturating_mul(stored.height);
    let bytes = inflate_b64(&stored.alpha_u8_deflate_b64)?;
    if bytes.len() < len {
        return Err("soft mask payload is shorter than expected".to_string());
    }
    let alpha: Vec<f32> = bytes[..len].iter().map(|&v| v as f32 / 255.0).collect();
    let source_alpha = if let Some(source) = &stored.source_alpha_u8_deflate_b64 {
        let source_bytes = inflate_b64(source)?;
        if source_bytes.len() < len {
            return Err("soft source mask payload is shorter than expected".to_string());
        }
        Some(
            source_bytes[..len]
                .iter()
                .map(|&v| v as f32 / 255.0)
                .collect(),
        )
    } else {
        Some(alpha.clone())
    };
    Ok(SubjectMask {
        width: stored.width,
        height: stored.height,
        alpha,
        source_alpha,
        refinement: stored.refinement,
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
        mask_before_effect: layer.mask_before_effect,
        mask_after_effect: layer.mask_after_effect,
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
        mask_before_effect: stored.mask_before_effect,
        mask_after_effect: stored.mask_after_effect,
        effect: stored.effect.clone(),
    })
}

fn color_image_from_rgba(image: &RgbaImageBuf) -> ColorImage {
    ColorImage::from_rgba_unmultiplied([image.width, image.height], &image.pixels)
}

fn build_mask_tile_image(
    mask: &[f32],
    layer: &LocalAdjustmentLayer,
    edit_target: Option<OverrideEditTarget>,
    colors: MaskPreviewColors,
    image_width: usize,
    tile_x: usize,
    tile_y: usize,
    tile_w: usize,
    tile_h: usize,
) -> ColorImage {
    let mut pixels = Vec::with_capacity(tile_w.saturating_mul(tile_h));
    let hide_full_base = matches!(layer.mask, LocalMask::Full);
    let show_full_base_while_editing = hide_full_base && edit_target.is_some();
    let show_full_result = hide_full_base
        && edit_target.is_none()
        && layer
            .manual_override
            .subtract
            .as_ref()
            .map(raster_vector_mask_has_content)
            .unwrap_or(false);
    for y in tile_y..tile_y + tile_h {
        let row = y * image_width;
        for x in tile_x..tile_x + tile_w {
            let idx = row + x;
            let editing_mask = edit_target
                .and_then(|target| match target {
                    OverrideEditTarget::Add => layer.manual_override.add.as_ref(),
                    OverrideEditTarget::Subtract => layer.manual_override.subtract.as_ref(),
                })
                .map(|manual| raster_vector_alpha_at(manual, idx, x, y) >= 0.5)
                .unwrap_or(false);
            if editing_mask {
                pixels.push(colors.edit(MASK_PREVIEW_EDIT_ALPHA));
            } else if show_full_base_while_editing || show_full_result {
                let alpha = (mask.get(idx).copied().unwrap_or(0.0).clamp(0.0, 1.0)
                    * MASK_PREVIEW_BASE_ALPHA)
                    .round() as u8;
                pixels.push(colors.base(alpha));
            } else if hide_full_base {
                pixels.push(Color32::TRANSPARENT);
            } else {
                let alpha = (mask.get(idx).copied().unwrap_or(0.0).clamp(0.0, 1.0)
                    * MASK_PREVIEW_BASE_ALPHA)
                    .round() as u8;
                pixels.push(colors.base(alpha));
            }
        }
    }
    ColorImage::new([tile_w, tile_h], pixels)
}

fn layer_for_mask_preview(
    layer: &LocalAdjustmentLayer,
    edit_target: Option<OverrideEditTarget>,
) -> LocalAdjustmentLayer {
    let mut preview = layer.clone();
    match edit_target {
        Some(OverrideEditTarget::Add) => {
            preview.manual_override = ManualMaskOverride::default();
        }
        Some(OverrideEditTarget::Subtract) => {
            preview.manual_override.subtract = None;
        }
        None => {}
    }
    preview
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

fn raster_vector_mask_has_content(mask: &RasterVectorMask) -> bool {
    mask.alpha.iter().any(|&alpha| alpha >= 0.5)
        || mask.shapes.iter().any(|shape| shape.op().is_add())
}

fn can_build_mask_tiles_from_layer(
    layer: &LocalAdjustmentLayer,
    image_width: usize,
    image_height: usize,
) -> bool {
    let mask_matches = match &layer.mask {
        LocalMask::Full => true,
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
    let manual_override_supported =
        matches!(layer.mask, LocalMask::Full) || layer.manual_override.is_empty();
    mask_matches
        && manual_override_supported
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
    colors: MaskPreviewColors,
) -> ColorImage {
    match &layer.mask {
        LocalMask::Full => {
            let show_result = layer
                .manual_override
                .subtract
                .as_ref()
                .map(raster_vector_mask_has_content)
                .unwrap_or(false);
            let mut pixels = Vec::with_capacity(tile_w.saturating_mul(tile_h));
            for y in tile_y..tile_y + tile_h {
                let row = y * image_width;
                for x in tile_x..tile_x + tile_w {
                    let idx = row + x;
                    if show_result {
                        let subtract_alpha = layer
                            .manual_override
                            .subtract
                            .as_ref()
                            .map(|manual| raster_vector_alpha_at(manual, idx, x, y))
                            .unwrap_or(0.0)
                            .clamp(0.0, 1.0);
                        let mut alpha = 1.0 - subtract_alpha;
                        if layer.mask_inverted {
                            alpha = 1.0 - alpha;
                        }
                        alpha = (alpha * layer.opacity.clamp(0.0, 1.0)).clamp(0.0, 1.0);
                        let alpha_u8 = (alpha * MASK_PREVIEW_BASE_ALPHA).round() as u8;
                        pixels.push(colors.base(alpha_u8));
                    } else {
                        pixels.push(Color32::TRANSPARENT);
                    }
                }
            }
            ColorImage::new([tile_w, tile_h], pixels)
        }
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
                    let alpha_u8 = (alpha * MASK_PREVIEW_BASE_ALPHA).round() as u8;
                    pixels.push(colors.base(alpha_u8));
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
                        colors,
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
    colors: MaskPreviewColors,
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
            return colors.boundary(235);
        }
        return colors.base(188);
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

    fn is_full(self, width: usize, height: usize) -> bool {
        let full = CropRect::full(width, height);
        let crop = self.sanitized(width, height);
        (crop.min_x - full.min_x).abs() < 0.5
            && (crop.min_y - full.min_y).abs() < 0.5
            && (crop.max_x - full.max_x).abs() < 0.5
            && (crop.max_y - full.max_y).abs() < 0.5
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

    fn fit_to_aspect_around_center(self, aspect_ratio: f32, width: usize, height: usize) -> Self {
        let ratio = aspect_ratio.max(0.01);
        let max_w = width.max(1) as f32;
        let max_h = height.max(1) as f32;
        let base = self.sanitized(width, height);
        let center = [
            (base.min_x + base.max_x) * 0.5,
            (base.min_y + base.max_y) * 0.5,
        ];
        let mut crop_w = base.width().min(max_w);
        let mut crop_h = base.height().min(max_h);
        if crop_w / crop_h > ratio {
            crop_w = crop_h * ratio;
        } else {
            crop_h = crop_w / ratio;
        }
        if crop_w > max_w {
            crop_w = max_w;
            crop_h = (crop_w / ratio).min(max_h);
        }
        if crop_h > max_h {
            crop_h = max_h;
            crop_w = (crop_h * ratio).min(max_w);
        }
        let min_x = (center[0] - crop_w * 0.5).clamp(0.0, max_w - crop_w);
        let min_y = (center[1] - crop_h * 0.5).clamp(0.0, max_h - crop_h);
        Self {
            min_x,
            min_y,
            max_x: min_x + crop_w,
            max_y: min_y + crop_h,
        }
        .sanitized(width, height)
    }

    fn dragged(
        self,
        handle: CropHandle,
        delta_x: f32,
        delta_y: f32,
        width: usize,
        height: usize,
        aspect_ratio: Option<f32>,
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
        let next = next.sanitized(width, height);
        if let Some(ratio) = aspect_ratio {
            next.fit_to_aspect_around_center(ratio, width, height)
        } else {
            next
        }
    }
}

fn crop_from_xywh_inputs(
    x: i32,
    y: i32,
    cw: i32,
    ch: i32,
    width: usize,
    height: usize,
    aspect_ratio: Option<f32>,
    prefer_height: bool,
) -> CropRect {
    let mut x = x.clamp(0, width.saturating_sub(1) as i32);
    let mut y = y.clamp(0, height.saturating_sub(1) as i32);
    let mut cw = cw.max(1).min(width.max(1) as i32);
    let mut ch = ch.max(1).min(height.max(1) as i32);
    if let Some(ratio) = aspect_ratio {
        let ratio = ratio.max(0.01);
        if prefer_height {
            cw = ((ch as f32 * ratio).round() as i32).max(1);
            if cw > width as i32 {
                cw = width as i32;
                ch = ((cw as f32 / ratio).round() as i32).max(1);
            }
        } else {
            ch = ((cw as f32 / ratio).round() as i32).max(1);
            if ch > height as i32 {
                ch = height as i32;
                cw = ((ch as f32 * ratio).round() as i32).max(1);
            }
        }
    }
    cw = cw.min(width.max(1) as i32);
    ch = ch.min(height.max(1) as i32);
    if x + cw > width as i32 {
        x = width as i32 - cw;
    }
    if y + ch > height as i32 {
        y = height as i32 - ch;
    }
    CropRect {
        min_x: x.max(0) as f32,
        min_y: y.max(0) as f32,
        max_x: (x + cw).max(1) as f32,
        max_y: (y + ch).max(1) as f32,
    }
    .sanitized(width, height)
}

fn crop_from_points(
    a: [f32; 2],
    b: [f32; 2],
    width: usize,
    height: usize,
    aspect_ratio: Option<f32>,
) -> CropRect {
    let min_x = a[0].min(b[0]);
    let min_y = a[1].min(b[1]);
    let max_x = a[0].max(b[0]);
    let max_y = a[1].max(b[1]);
    let crop = CropRect {
        min_x,
        min_y,
        max_x,
        max_y,
    }
    .sanitized(width, height);
    if let Some(ratio) = aspect_ratio {
        crop.fit_to_aspect_around_center(ratio, width, height)
    } else {
        crop
    }
}

fn clamp_pos_to_rect(pos: Pos2, rect: Rect) -> Pos2 {
    egui::pos2(
        pos.x.clamp(rect.left(), rect.right()),
        pos.y.clamp(rect.top(), rect.bottom()),
    )
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

/// Decide what a primary-button press at `press` should start.
///
/// Priority: a resize handle (corner/edge) wins first, then dragging the body of an
/// active crop moves it, otherwise the press begins a fresh create-drag anywhere in the
/// image. `handle_points` / `handle_bounds` / `handle_hit` mirror exactly what
/// `draw_crop_overlay` draws, so the hit areas can never drift from the rendered dots.
/// Returns `None` if the press is outside the image entirely.
fn crop_press_target(
    press: Pos2,
    image_rect: Rect,
    crop_screen: Rect,
    crop_active: bool,
    handle_points: &[(CropHandle, Pos2)],
    handle_bounds: Rect,
    handle_hit: f32,
) -> Option<CropPressTarget> {
    if !image_rect.contains(press) {
        return None;
    }
    for (handle, center) in handle_points {
        if *handle == CropHandle::Body {
            continue;
        }
        let hit = Rect::from_center_size(
            clamp_pos_to_rect(*center, handle_bounds),
            egui::vec2(handle_hit, handle_hit),
        );
        if hit.contains(press) {
            return Some(CropPressTarget::Resize(*handle));
        }
    }
    if crop_active && crop_screen.contains(press) {
        return Some(CropPressTarget::Move);
    }
    Some(CropPressTarget::Create)
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

fn load_recent_files() -> Vec<PathBuf> {
    let Ok(text) = std::fs::read_to_string(recent_files_path()) else {
        return Vec::new();
    };
    let Ok(history) = serde_json::from_str::<LabHistory>(&text) else {
        return Vec::new();
    };
    let mut recent_files = Vec::new();
    for path in history.recent_files.iter().rev() {
        push_recent_file(&mut recent_files, path);
    }
    recent_files
}

fn save_recent_files(recent_files: &[PathBuf]) -> Result<(), String> {
    let path = recent_files_path();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    let history = LabHistory {
        recent_files: recent_files.to_vec(),
    };
    let json = serde_json::to_string_pretty(&history).map_err(|e| e.to_string())?;
    std::fs::write(&path, json).map_err(|e| e.to_string())
}

fn push_recent_file(recent_files: &mut Vec<PathBuf>, path: &Path) {
    let path = normalize_recent_path(path);
    let key = recent_file_key(&path);
    recent_files.retain(|existing| recent_file_key(existing) != key);
    recent_files.insert(0, path);
    recent_files.truncate(FILE_HISTORY_LIMIT);
}

fn normalize_recent_path(path: &Path) -> PathBuf {
    if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()
            .unwrap_or_else(|_| PathBuf::from("."))
            .join(path)
    }
}

fn recent_file_key(path: &Path) -> String {
    path.to_string_lossy().to_lowercase()
}

fn history_menu_label(path: &Path) -> String {
    let file_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("画像");
    let parent_name = path
        .parent()
        .and_then(|parent| parent.file_name())
        .and_then(|name| name.to_str())
        .unwrap_or("");
    if parent_name.is_empty() {
        file_name.to_string()
    } else {
        format!("{file_name} ({parent_name})")
    }
}

fn recent_files_path() -> PathBuf {
    if let Some(appdata) = std::env::var_os("APPDATA") {
        return PathBuf::from(appdata)
            .join("mimageviewer")
            .join("local_adjust_lab")
            .join("recent_files.json");
    }
    std::env::current_dir()
        .unwrap_or_else(|_| PathBuf::from("."))
        .join("target")
        .join("local_adjust_lab_recent_files.json")
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

fn norm_to_screen_unclamped(rect: Rect, n: [f32; 2]) -> Pos2 {
    Pos2::new(
        rect.left() + n[0] * rect.width(),
        rect.top() + n[1] * rect.height(),
    )
}

fn offset_norm(base: [f32; 2], direction: [f32; 2], amount: f32) -> [f32; 2] {
    [
        base[0] + direction[0] * amount,
        base[1] + direction[1] * amount,
    ]
}

fn screen_to_norm(rect: Rect, p: Pos2) -> [f32; 2] {
    [
        ((p.x - rect.left()) / rect.width()).clamp(0.0, 1.0),
        ((p.y - rect.top()) / rect.height()).clamp(0.0, 1.0),
    ]
}

fn screen_px_per_source_px(rect: Rect, image_dims: (usize, usize)) -> f32 {
    let sx = rect.width() / image_dims.0.max(1) as f32;
    let sy = rect.height() / image_dims.1.max(1) as f32;
    (sx + sy) * 0.5
}

fn distance_to_farthest_rect_corner(center: Pos2, rect: Rect) -> f32 {
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

fn draw_effect_center_handle(
    ui: &mut egui::Ui,
    rect: Rect,
    id: egui::Id,
    center: &mut [f32; 2],
    label: &str,
    fill: Color32,
) -> (bool, bool, Pos2) {
    let center_screen = norm_to_screen(rect, *center);
    let (changed, used) = drag_norm_handle(ui, rect, id, center_screen, center, label);
    let center_screen = norm_to_screen(rect, *center);
    let painter = ui.painter();
    let guide = egui::Stroke::new(
        1.0,
        Color32::from_rgba_unmultiplied(fill.r(), fill.g(), fill.b(), 145),
    );
    let stroke = egui::Stroke::new(2.0, Color32::from_rgb(10, 30, 36));
    painter.circle_filled(center_screen, 7.0, fill);
    painter.circle_stroke(center_screen, 7.0, stroke);
    painter.line_segment(
        [
            Pos2::new(center_screen.x - 14.0, center_screen.y),
            Pos2::new(center_screen.x + 14.0, center_screen.y),
        ],
        guide,
    );
    painter.line_segment(
        [
            Pos2::new(center_screen.x, center_screen.y - 14.0),
            Pos2::new(center_screen.x, center_screen.y + 14.0),
        ],
        guide,
    );
    (changed, used, center_screen)
}

fn draw_effect_source_radius(
    painter: &egui::Painter,
    rect: Rect,
    center: Pos2,
    radius_px: f32,
    source_px_scale: f32,
    color: Color32,
) {
    let radius = if radius_px > 0.0 {
        radius_px * source_px_scale
    } else {
        distance_to_farthest_rect_corner(center, rect)
    };
    if radius > 1.5 {
        painter.circle_stroke(center, radius, egui::Stroke::new(1.0, color));
    }
}

fn mask_preview_active(adjust_panel_active: bool, show_mask: bool, alt_down: bool) -> bool {
    adjust_panel_active && (show_mask != alt_down)
}

#[derive(Clone, Copy)]
struct GradientHandleVisuals {
    stroke: egui::Stroke,
    soft_stroke: egui::Stroke,
    start_fill: Color32,
    end_fill: Color32,
    center_fill: Color32,
    handle_stroke: egui::Stroke,
}

fn mask_gradient_visuals() -> GradientHandleVisuals {
    GradientHandleVisuals {
        stroke: egui::Stroke::new(2.0, Color32::from_rgb(255, 220, 80)),
        soft_stroke: egui::Stroke::new(1.0, Color32::from_rgba_unmultiplied(255, 220, 80, 100)),
        start_fill: Color32::from_rgb(255, 250, 210),
        end_fill: Color32::from_rgb(255, 190, 110),
        center_fill: Color32::from_rgb(255, 250, 210),
        handle_stroke: egui::Stroke::new(2.0, Color32::from_rgb(40, 30, 10)),
    }
}

fn effect_gradient_visuals() -> GradientHandleVisuals {
    GradientHandleVisuals {
        stroke: egui::Stroke::new(2.0, Color32::from_rgb(120, 220, 255)),
        soft_stroke: egui::Stroke::new(1.0, Color32::from_rgba_unmultiplied(120, 220, 255, 110)),
        start_fill: Color32::from_rgb(215, 250, 255),
        end_fill: Color32::from_rgb(120, 220, 255),
        center_fill: Color32::from_rgb(215, 250, 255),
        handle_stroke: egui::Stroke::new(2.0, Color32::from_rgb(10, 30, 36)),
    }
}

fn draw_linear_gradient_handles(
    ui: &mut egui::Ui,
    rect: Rect,
    id: egui::Id,
    start: &mut [f32; 2],
    end: &mut [f32; 2],
    visuals: GradientHandleVisuals,
) -> (bool, bool) {
    let painter = ui.painter().clone();
    let start_screen = norm_to_screen(rect, *start);
    let end_screen = norm_to_screen(rect, *end);
    painter.line_segment([start_screen, end_screen], visuals.stroke);
    let (start_changed, start_used) = drag_norm_handle(
        ui,
        rect,
        id.with("linear_start"),
        start_screen,
        start,
        "開始",
    );
    let (end_changed, end_used) =
        drag_norm_handle(ui, rect, id.with("linear_end"), end_screen, end, "終了");
    let start_screen = norm_to_screen(rect, *start);
    let end_screen = norm_to_screen(rect, *end);
    painter.circle_filled(start_screen, 6.0, visuals.start_fill);
    painter.circle_stroke(start_screen, 6.0, visuals.handle_stroke);
    painter.circle_filled(end_screen, 6.0, visuals.end_fill);
    painter.circle_stroke(end_screen, 6.0, visuals.handle_stroke);
    (start_changed || end_changed, start_used || end_used)
}

fn draw_radial_circle_gradient_handles(
    ui: &mut egui::Ui,
    rect: Rect,
    id: egui::Id,
    center: &mut [f32; 2],
    radius: &mut f32,
    visuals: GradientHandleVisuals,
) -> (bool, bool) {
    let painter = ui.painter().clone();
    let center_screen = norm_to_screen(rect, *center);
    let radius_x = radius.max(0.001) * rect.width();
    let radius_handle = Pos2::new(center_screen.x + radius_x, center_screen.y);
    let (center_changed, center_used) = drag_norm_handle(
        ui,
        rect,
        id.with("radial_center"),
        center_screen,
        center,
        "中心",
    );
    let radius_resp = ui
        .interact(
            Rect::from_center_size(radius_handle, egui::vec2(28.0, 28.0)),
            id.with("radial_radius"),
            Sense::drag(),
        )
        .lab_hover_tip("半径");
    let mut radius_changed = false;
    if radius_resp.dragged()
        && let Some(pos) = radius_resp.interact_pointer_pos()
    {
        let n = screen_to_norm(rect, pos);
        let dx = n[0] - center[0];
        let dy = n[1] - center[1];
        *radius = (dx * dx + dy * dy).sqrt().clamp(0.02, 2.0);
        radius_changed = true;
    }
    let center_screen = norm_to_screen(rect, *center);
    let radius_x = radius.max(0.001) * rect.width();
    let radius_y = radius.max(0.001) * rect.height();
    let radius_handle = Pos2::new(center_screen.x + radius_x, center_screen.y);
    draw_ellipse_stroke(&painter, center_screen, radius_x, radius_y, visuals.stroke);
    painter.line_segment([center_screen, radius_handle], visuals.soft_stroke);
    painter.circle_filled(center_screen, 6.0, visuals.center_fill);
    painter.circle_stroke(center_screen, 6.0, visuals.handle_stroke);
    painter.circle_filled(radius_handle, 6.0, visuals.end_fill);
    painter.circle_stroke(radius_handle, 6.0, visuals.handle_stroke);
    (
        center_changed || radius_changed,
        center_used || radius_resp.hovered() || radius_resp.dragged(),
    )
}

fn linear_points_from_angle(angle_degrees: f32) -> ([f32; 2], [f32; 2]) {
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

fn angle_from_linear_points(start: [f32; 2], end: [f32; 2]) -> Option<f32> {
    let dx = end[0] - start[0];
    let dy = end[1] - start[1];
    if dx * dx + dy * dy <= 0.000001 {
        None
    } else {
        Some(dy.atan2(dx).to_degrees())
    }
}

#[derive(Clone, Copy)]
struct ColorGradientGeometry {
    shape: ColorOverlayShape,
    angle_degrees: f32,
    linear_points_enabled: bool,
    linear_start: [f32; 2],
    linear_end: [f32; 2],
    center: [f32; 2],
    radius: f32,
}

fn color_fill_gradient_geometry(params: &ColorFillParams) -> ColorGradientGeometry {
    ColorGradientGeometry {
        shape: params.shape,
        angle_degrees: params.angle_degrees,
        linear_points_enabled: params.linear_points_enabled,
        linear_start: params.linear_start,
        linear_end: params.linear_end,
        center: params.center,
        radius: params.radius,
    }
}

fn apply_color_fill_gradient_geometry(
    params: &mut ColorFillParams,
    geometry: ColorGradientGeometry,
) {
    params.angle_degrees = geometry.angle_degrees;
    params.linear_points_enabled = geometry.linear_points_enabled;
    params.linear_start = geometry.linear_start;
    params.linear_end = geometry.linear_end;
    params.center = geometry.center;
    params.radius = geometry.radius;
}

fn color_overlay_gradient_geometry(params: &ColorOverlayParams) -> ColorGradientGeometry {
    ColorGradientGeometry {
        shape: params.shape,
        angle_degrees: params.angle_degrees,
        linear_points_enabled: params.linear_points_enabled,
        linear_start: params.linear_start,
        linear_end: params.linear_end,
        center: params.center,
        radius: params.radius,
    }
}

fn apply_color_overlay_gradient_geometry(
    params: &mut ColorOverlayParams,
    geometry: ColorGradientGeometry,
) {
    params.angle_degrees = geometry.angle_degrees;
    params.linear_points_enabled = geometry.linear_points_enabled;
    params.linear_start = geometry.linear_start;
    params.linear_end = geometry.linear_end;
    params.center = geometry.center;
    params.radius = geometry.radius;
}

fn color_gradient_linear_points(geometry: ColorGradientGeometry) -> ([f32; 2], [f32; 2]) {
    if geometry.linear_points_enabled {
        (geometry.linear_start, geometry.linear_end)
    } else {
        linear_points_from_angle(geometry.angle_degrees)
    }
}

fn set_color_gradient_linear_points(
    geometry: &mut ColorGradientGeometry,
    start: [f32; 2],
    end: [f32; 2],
) {
    geometry.linear_points_enabled = true;
    geometry.linear_start = start;
    geometry.linear_end = end;
    if let Some(angle) = angle_from_linear_points(start, end) {
        geometry.angle_degrees = angle;
    }
}

fn drag_color_gradient_geometry(
    geometry: &mut ColorGradientGeometry,
    n: [f32; 2],
    started: bool,
) -> bool {
    match geometry.shape {
        ColorOverlayShape::Unselected => false,
        ColorOverlayShape::Solid => false,
        ColorOverlayShape::Linear => {
            if started || !geometry.linear_points_enabled {
                set_color_gradient_linear_points(geometry, n, n);
            } else {
                set_color_gradient_linear_points(geometry, geometry.linear_start, n);
            }
            true
        }
        ColorOverlayShape::Radial => {
            if started {
                geometry.center = n;
                geometry.radius = 0.02;
            } else {
                let dx = n[0] - geometry.center[0];
                let dy = n[1] - geometry.center[1];
                geometry.radius = (dx * dx + dy * dy).sqrt().clamp(0.02, 2.0);
            }
            true
        }
    }
}

fn reset_color_gradient_geometry(geometry: &mut ColorGradientGeometry) -> bool {
    match geometry.shape {
        ColorOverlayShape::Unselected => false,
        ColorOverlayShape::Solid => false,
        ColorOverlayShape::Linear => {
            geometry.linear_points_enabled = false;
            let (start, end) = linear_points_from_angle(geometry.angle_degrees);
            geometry.linear_start = start;
            geometry.linear_end = end;
            true
        }
        ColorOverlayShape::Radial => {
            geometry.center = [0.5, 0.5];
            geometry.radius = 0.85;
            true
        }
    }
}

fn draw_color_gradient_geometry_handles(
    ui: &mut egui::Ui,
    rect: Rect,
    layer_idx: usize,
    id_label: &'static str,
    geometry: &mut ColorGradientGeometry,
    visuals: GradientHandleVisuals,
) -> (bool, bool) {
    match geometry.shape {
        ColorOverlayShape::Unselected => (false, false),
        ColorOverlayShape::Solid => (false, false),
        ColorOverlayShape::Linear => {
            let (mut start, mut end) = color_gradient_linear_points(*geometry);
            let (changed, used) = draw_linear_gradient_handles(
                ui,
                rect,
                ui.id()
                    .with(("effect_linear_gradient", id_label, layer_idx)),
                &mut start,
                &mut end,
                visuals,
            );
            if changed {
                set_color_gradient_linear_points(geometry, start, end);
            }
            (changed, used)
        }
        ColorOverlayShape::Radial => draw_radial_circle_gradient_handles(
            ui,
            rect,
            ui.id()
                .with(("effect_radial_gradient", id_label, layer_idx)),
            &mut geometry.center,
            &mut geometry.radius,
            visuals,
        ),
    }
}

fn hue_degrees_from_rgb(rgb: [u8; 3]) -> f32 {
    let r = rgb[0] as f32 / 255.0;
    let g = rgb[1] as f32 / 255.0;
    let b = rgb[2] as f32 / 255.0;
    let max = r.max(g).max(b);
    let min = r.min(g).min(b);
    let delta = max - min;
    if delta <= f32::EPSILON {
        return 0.0;
    }
    let hue = if (max - r).abs() <= f32::EPSILON {
        ((g - b) / delta).rem_euclid(6.0)
    } else if (max - g).abs() <= f32::EPSILON {
        (b - r) / delta + 2.0
    } else {
        (r - g) / delta + 4.0
    };
    (hue * 60.0).rem_euclid(360.0)
}

fn hsl_swatch_color(hue_degrees: f32, saturation: f32, lightness: f32) -> Color32 {
    let h = hue_degrees.rem_euclid(360.0) / 360.0;
    let s = saturation.clamp(0.0, 1.0);
    let l = lightness.clamp(0.0, 1.0);
    if s <= f32::EPSILON {
        let v = (l * 255.0).round() as u8;
        return Color32::from_rgb(v, v, v);
    }
    let q = if l < 0.5 {
        l * (1.0 + s)
    } else {
        l + s - l * s
    };
    let p = 2.0 * l - q;
    let r = hue_channel(p, q, h + 1.0 / 3.0);
    let g = hue_channel(p, q, h);
    let b = hue_channel(p, q, h - 1.0 / 3.0);
    Color32::from_rgb(
        (r * 255.0).round() as u8,
        (g * 255.0).round() as u8,
        (b * 255.0).round() as u8,
    )
}

fn hue_channel(p: f32, q: f32, t: f32) -> f32 {
    let t = t.rem_euclid(1.0);
    if t < 1.0 / 6.0 {
        p + (q - p) * 6.0 * t
    } else if t < 0.5 {
        q
    } else if t < 2.0 / 3.0 {
        p + (q - p) * (2.0 / 3.0 - t) * 6.0
    } else {
        p
    }
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
        .lab_hover_tip(label);
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
    fn alt_inverts_mask_preview_visibility() {
        assert!(mask_preview_active(true, true, false));
        assert!(!mask_preview_active(true, true, true));
        assert!(!mask_preview_active(true, false, false));
        assert!(mask_preview_active(true, false, true));
        assert!(!mask_preview_active(false, true, false));
        assert!(!mask_preview_active(false, false, true));
    }

    #[test]
    fn add_layer_dialog_mask_order_starts_with_full() {
        let ordered: Vec<MaskKind> = MASK_GROUPS
            .iter()
            .flat_map(|group| group.kinds.iter().copied())
            .collect();
        assert_eq!(
            ordered,
            vec![
                MaskKind::Full,
                MaskKind::Raster,
                MaskKind::LinearGradient,
                MaskKind::RadialGradient,
                MaskKind::LumaRange,
                MaskKind::ColorRange,
                MaskKind::Subject,
                MaskKind::Segmentation,
            ]
        );
    }

    #[test]
    fn effect_picker_groups_are_subdivided_and_cover_every_effect() {
        let titles: Vec<&str> = EFFECT_GROUPS.iter().map(|group| group.title).collect();
        assert_eq!(
            titles,
            vec![
                "基本",
                "色調補正",
                "色変換・ルック",
                "ぼかし・フォーカス",
                "シャープ・ディテール",
                "変形・歪み",
                "表現・絵画調",
                "隠蔽・加工",
                "光・雰囲気",
            ]
        );

        let grouped: Vec<EffectKind> = EFFECT_GROUPS
            .iter()
            .flat_map(|group| group.kinds.iter().copied())
            .collect();
        let expected = vec![
            EffectKind::None,
            EffectKind::ColorFill,
            EffectKind::OutlineStroke,
            EffectKind::Tone,
            EffectKind::ToneCurve,
            EffectKind::RgbToneCurve,
            EffectKind::ColorBalance,
            EffectKind::ThreeWayColorGrading,
            EffectKind::SelectiveColor,
            EffectKind::ChannelMixer,
            EffectKind::Clarity,
            EffectKind::Texture,
            EffectKind::HighPass,
            EffectKind::HighlightsShadows,
            EffectKind::Dehaze,
            EffectKind::Blur,
            EffectKind::MotionBlur,
            EffectKind::Wind,
            EffectKind::SpeedLines,
            EffectKind::TiltShift,
            EffectKind::LensBlur,
            EffectKind::RadialBlur,
            EffectKind::WaveDistortion,
            EffectKind::PinchSpherize,
            EffectKind::Twirl,
            EffectKind::PolarCoordinates,
            EffectKind::GlassDisplacement,
            EffectKind::LensCorrection,
            EffectKind::LineExtract,
            EffectKind::ArtisticMedia,
            EffectKind::BrushStroke,
            EffectKind::Cutout,
            EffectKind::Emboss,
            EffectKind::PixelStylize,
            EffectKind::Solarize,
            EffectKind::GlowingEdges,
            EffectKind::OilPaint,
            EffectKind::SoftFocus,
            EffectKind::Mosaic,
            EffectKind::Sharpen,
            EffectKind::SmartSharpen,
            EffectKind::Hsl,
            EffectKind::ColorMixer,
            EffectKind::Look,
            EffectKind::CubeLut,
            EffectKind::Posterize,
            EffectKind::Threshold,
            EffectKind::Invert,
            EffectKind::Duotone,
            EffectKind::Equalize,
            EffectKind::GradientMap,
            EffectKind::NeonGlow,
            EffectKind::DiffuseGlow,
            EffectKind::Bloom,
            EffectKind::GodRays,
            EffectKind::LensFlare,
            EffectKind::CloudFog,
            EffectKind::Spotlight,
            EffectKind::Vignette,
            EffectKind::FilmGrain,
            EffectKind::Noise,
            EffectKind::ChromaticAberration,
            EffectKind::Halftone,
            EffectKind::ScreenTone,
            EffectKind::ColorHalftone,
            EffectKind::Textureizer,
            EffectKind::ColorOverlay,
            EffectKind::StarGlow,
            EffectKind::EdgeSmooth,
            EffectKind::Despeckle,
            EffectKind::Median,
        ];

        assert_eq!(grouped.len(), expected.len());
        for kind in expected {
            let count = grouped.iter().filter(|&&item| item == kind).count();
            assert_eq!(count, 1, "effect group count for {}", kind.label());
        }
    }

    #[test]
    fn image_space_center_effects_have_position_handles() {
        let mut ripple = WaveDistortionParams::default();
        ripple.mode = WaveDistortionMode::Ripple;

        let handled = [
            LocalEffect::RadialBlur(RadialBlurParams::default()),
            LocalEffect::WaveDistortion(ripple),
            LocalEffect::PinchSpherize(PinchSpherizeParams::default()),
            LocalEffect::Twirl(TwirlParams::default()),
            LocalEffect::PolarCoordinates(PolarCoordinatesParams::default()),
            LocalEffect::LensCorrection(LensCorrectionParams::default()),
            LocalEffect::GodRays(GodRaysParams::default()),
            LocalEffect::LensFlare(LensFlareParams::default()),
            LocalEffect::SpeedLines(SpeedLinesParams::default()),
            LocalEffect::Spotlight(SpotlightParams::default()),
        ];
        for effect in handled {
            assert!(
                effect_has_position_handles(&effect),
                "expected position handles for {}",
                effect_summary(&effect)
            );
        }

        let mut horizontal_wave = WaveDistortionParams::default();
        horizontal_wave.mode = WaveDistortionMode::Horizontal;

        let separate_or_non_position = [
            LocalEffect::WaveDistortion(horizontal_wave),
            LocalEffect::ColorFill(ColorFillParams {
                shape: ColorOverlayShape::Radial,
                ..Default::default()
            }),
            LocalEffect::ColorOverlay(ColorOverlayParams {
                shape: ColorOverlayShape::Radial,
                ..Default::default()
            }),
            LocalEffect::TiltShift(TiltShiftParams::default()),
            LocalEffect::Tone(ToneParams::default()),
        ];
        for effect in separate_or_non_position {
            assert!(
                !effect_has_position_handles(&effect),
                "unexpected position handles for {}",
                effect_summary(&effect)
            );
        }
    }

    #[test]
    fn recent_files_are_deduped_and_limited_to_twenty() {
        let mut recent_files = Vec::new();
        for idx in 0..25 {
            push_recent_file(
                &mut recent_files,
                &PathBuf::from(format!(r"C:\images\sample_{idx}.png")),
            );
        }

        assert_eq!(recent_files.len(), FILE_HISTORY_LIMIT);
        assert_eq!(recent_files[0], PathBuf::from(r"C:\images\sample_24.png"));
        assert_eq!(
            recent_files[FILE_HISTORY_LIMIT - 1],
            PathBuf::from(r"C:\images\sample_5.png")
        );

        push_recent_file(
            &mut recent_files,
            &PathBuf::from(r"C:\images\sample_10.png"),
        );
        assert_eq!(recent_files[0], PathBuf::from(r"C:\images\sample_10.png"));
        assert_eq!(recent_files.len(), FILE_HISTORY_LIMIT);
        assert_eq!(
            recent_files
                .iter()
                .filter(|path| *path == &PathBuf::from(r"C:\images\sample_10.png"))
                .count(),
            1
        );
    }

    #[test]
    fn selective_color_eyedropper_hue_matches_primary_colors() {
        assert!((hue_degrees_from_rgb([255, 0, 0]) - 0.0).abs() < 0.1);
        assert!((hue_degrees_from_rgb([0, 255, 0]) - 120.0).abs() < 0.1);
        assert!((hue_degrees_from_rgb([0, 0, 255]) - 240.0).abs() < 0.1);
    }

    #[test]
    fn subject_cutout_refinement_binarizes_soft_alpha() {
        let mask = RasterMask {
            width: 4,
            height: 1,
            alpha: vec![0.20, 0.49, 0.52, 0.90],
        };
        let refined = subject_cutout_refined_alpha(&mask, 0.5, 0, 0);
        assert_eq!(refined, vec![0.0, 0.0, 1.0, 1.0]);
        let stats = subject_mask_stats(&SubjectMask::from_raster(RasterMask {
            width: 4,
            height: 1,
            alpha: refined,
        }));
        assert_eq!(stats.foreground_percent, 50.0);
        assert_eq!(stats.soft_percent, 0.0);
    }

    #[test]
    fn subject_cutout_refinement_smooths_only_boundary_band() {
        let mask = RasterMask {
            width: 5,
            height: 1,
            alpha: vec![0.0, 1.0, 1.0, 1.0, 0.0],
        };
        let refined = subject_cutout_refined_alpha(&mask, 0.5, 0, 1);
        assert!((refined[0] - 0.5).abs() < 0.001);
        assert!((refined[1] - (2.0 / 3.0)).abs() < 0.001);
        assert!((refined[2] - 1.0).abs() < 0.001);
        assert!((refined[3] - (2.0 / 3.0)).abs() < 0.001);
        assert!((refined[4] - 0.5).abs() < 0.001);
    }

    #[test]
    fn subject_cutout_refinement_can_expand_or_shrink_binary_mask() {
        let mask = RasterMask {
            width: 5,
            height: 1,
            alpha: vec![0.0, 0.0, 1.0, 0.0, 0.0],
        };
        let expanded = subject_cutout_refined_alpha(&mask, 0.5, 1, 0);
        assert_eq!(expanded, vec![0.0, 1.0, 1.0, 1.0, 0.0]);

        let mask = RasterMask {
            width: 5,
            height: 1,
            alpha: vec![0.0, 1.0, 1.0, 1.0, 0.0],
        };
        let shrunk = subject_cutout_refined_alpha(&mask, 0.5, -1, 0);
        assert_eq!(shrunk, vec![0.0, 0.0, 1.0, 0.0, 0.0]);
    }

    #[test]
    fn subject_cutout_refinement_can_regenerate_from_cached_source() {
        let source = RasterMask {
            width: 4,
            height: 1,
            alpha: vec![0.20, 0.45, 0.55, 0.90],
        };
        let mut subject = SubjectMask::from_raster(source);
        subject.alpha = subject_cutout_refined_alpha(&subject.source_raster_mask(), 0.60, 0, 0);
        assert_eq!(subject.alpha, vec![0.0, 0.0, 0.0, 1.0]);

        subject.alpha = subject_cutout_refined_alpha(&subject.source_raster_mask(), 0.40, 0, 0);
        assert_eq!(subject.alpha, vec![0.0, 1.0, 1.0, 1.0]);
    }

    #[test]
    fn rgb_eyedropper_assigns_effect_color_targets() {
        let mut fill = LocalEffect::ColorFill(ColorFillParams::default());
        assert!(set_rgb_pick_target(
            &mut fill,
            RgbPickTarget::ColorFillStart,
            [10, 20, 30],
        ));
        assert!(set_rgb_pick_target(
            &mut fill,
            RgbPickTarget::ColorFillMiddle,
            [40, 50, 60],
        ));
        assert!(set_rgb_pick_target(
            &mut fill,
            RgbPickTarget::ColorFillEnd,
            [70, 80, 90],
        ));
        let LocalEffect::ColorFill(fill_params) = fill else {
            panic!("expected color fill effect");
        };
        assert_eq!(fill_params.start_rgb, [10, 20, 30]);
        assert_eq!(fill_params.middle_rgb, [40, 50, 60]);
        assert_eq!(fill_params.end_rgb, [70, 80, 90]);

        let mut overlay = LocalEffect::ColorOverlay(ColorOverlayParams::default());
        assert!(set_rgb_pick_target(
            &mut overlay,
            RgbPickTarget::ColorOverlayStart,
            [11, 22, 33],
        ));
        assert!(set_rgb_pick_target(
            &mut overlay,
            RgbPickTarget::ColorOverlayEnd,
            [44, 55, 66],
        ));
        let LocalEffect::ColorOverlay(overlay_params) = overlay else {
            panic!("expected color overlay effect");
        };
        assert_eq!(overlay_params.start_rgb, [11, 22, 33]);
        assert_eq!(overlay_params.end_rgb, [44, 55, 66]);

        let mut neon = LocalEffect::NeonGlow(NeonGlowParams {
            source_color_enabled: false,
            ..Default::default()
        });
        assert!(set_rgb_pick_target(
            &mut neon,
            RgbPickTarget::NeonGlowSource,
            [0, 210, 255],
        ));
        assert!(set_rgb_pick_target(
            &mut neon,
            RgbPickTarget::NeonGlowTint,
            [255, 80, 180],
        ));
        let LocalEffect::NeonGlow(neon_params) = neon else {
            panic!("expected neon glow effect");
        };
        assert_eq!(neon_params.source_rgb, [0, 210, 255]);
        assert!(neon_params.source_color_enabled);
        assert_eq!(neon_params.tint_rgb, [255, 80, 180]);

        let mut speed_lines = LocalEffect::SpeedLines(SpeedLinesParams::default());
        assert!(set_rgb_pick_target(
            &mut speed_lines,
            RgbPickTarget::SpeedLinesColor,
            [12, 34, 56],
        ));
        let LocalEffect::SpeedLines(speed_lines_params) = speed_lines else {
            panic!("expected speed lines effect");
        };
        assert_eq!(speed_lines_params.color_rgb, [12, 34, 56]);

        let mut cloud_fog = LocalEffect::CloudFog(CloudFogParams::default());
        assert!(set_rgb_pick_target(
            &mut cloud_fog,
            RgbPickTarget::CloudFogColor,
            [90, 120, 180],
        ));
        let LocalEffect::CloudFog(cloud_fog_params) = cloud_fog else {
            panic!("expected cloud fog effect");
        };
        assert_eq!(cloud_fog_params.color_rgb, [90, 120, 180]);

        let mut spotlight = LocalEffect::Spotlight(SpotlightParams::default());
        assert!(set_rgb_pick_target(
            &mut spotlight,
            RgbPickTarget::SpotlightTint,
            [255, 210, 120],
        ));
        let LocalEffect::Spotlight(spotlight_params) = spotlight else {
            panic!("expected spotlight effect");
        };
        assert_eq!(spotlight_params.tint_rgb, [255, 210, 120]);

        let mut outline = LocalEffect::OutlineStroke(OutlineStrokeParams::default());
        assert!(set_rgb_pick_target(
            &mut outline,
            RgbPickTarget::OutlineStrokeColor,
            [5, 6, 7],
        ));
        let LocalEffect::OutlineStroke(outline_params) = outline else {
            panic!("expected outline stroke effect");
        };
        assert_eq!(outline_params.color_rgb, [5, 6, 7]);

        let mut tone = LocalEffect::Tone(ToneParams::default());
        assert!(!set_rgb_pick_target(
            &mut tone,
            RgbPickTarget::NeonGlowSource,
            [1, 2, 3],
        ));
    }

    #[test]
    fn sample_cube_luts_parse() {
        for (name, text) in [
            ("identity", include_str!("../sample_luts/identity.cube")),
            (
                "warm_sunset",
                include_str!("../sample_luts/warm_sunset.cube"),
            ),
            (
                "cool_moonlight",
                include_str!("../sample_luts/cool_moonlight.cube"),
            ),
            ("soft_film", include_str!("../sample_luts/soft_film.cube")),
            ("vivid_pop", include_str!("../sample_luts/vivid_pop.cube")),
        ] {
            let lut = parse_cube_lut(text, name).unwrap();
            assert_eq!(lut.size, 2);
            assert_eq!(lut.table.len(), 8);
        }
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
    fn stored_subject_mask_roundtrips_source_and_refinement() {
        let mask = SubjectMask {
            width: 3,
            height: 1,
            alpha: vec![0.0, 1.0, 1.0],
            source_alpha: Some(vec![0.20, 0.55, 0.90]),
            refinement: SubjectMaskRefinement {
                enabled: true,
                threshold: 0.58,
                expand_px: -1,
                feather_px: 2,
            },
        };

        let stored = stored_soft_mask_from_mask(&mask).unwrap();
        let restored = soft_mask_from_stored(&stored).unwrap();

        assert_eq!(restored.width, 3);
        assert_eq!(restored.height, 1);
        assert_eq!(restored.alpha, mask.alpha);
        let source = restored.source_alpha.as_ref().unwrap();
        for (actual, expected) in source.iter().zip(mask.source_alpha.as_ref().unwrap()) {
            assert!((actual - expected).abs() <= 1.0 / 255.0);
        }
        assert_eq!(restored.refinement, mask.refinement);
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
        layer.mask_before_effect = true;
        layer.mask_after_effect = false;

        let stored = stored_layer_from_local(&layer).unwrap();
        let restored = local_layer_from_stored(&stored).unwrap();

        assert_eq!(restored.name, "soft");
        assert!(matches!(restored.mask, LocalMask::RadialGradient(_)));
        assert!(restored.manual_override.subtract.is_some());
        assert!(matches!(restored.effect, LocalEffect::SoftFocus(_)));
        assert!(restored.mask_inverted);
        assert_eq!(restored.mask_feather_px, 3.0);
        assert!(restored.mask_before_effect);
        assert!(!restored.mask_after_effect);
    }

    #[test]
    fn full_mask_preview_without_subtract_hides_base() {
        let layer = LocalAdjustmentLayer::new(
            "mask",
            LocalMask::Full,
            LocalEffect::Tone(ToneParams::default()),
        );

        let image = build_mask_tile_image(
            &[1.0, 1.0, 1.0],
            &layer,
            None,
            MaskColorPreset::PinkCyan.colors(),
            3,
            0,
            0,
            3,
            1,
        );

        assert_eq!(image.pixels[0], Color32::TRANSPARENT);
        assert_eq!(image.pixels[1], Color32::TRANSPARENT);
        assert_eq!(image.pixels[2], Color32::TRANSPARENT);
    }

    #[test]
    fn full_mask_preview_shows_result_after_subtract_override() {
        let mut layer = LocalAdjustmentLayer::new(
            "mask",
            LocalMask::Full,
            LocalEffect::Tone(ToneParams::default()),
        );
        layer.manual_override.subtract = Some(RasterVectorMask {
            width: 3,
            height: 1,
            alpha: vec![0.0, 1.0, 0.0],
            shapes: Vec::new(),
        });

        let image = build_mask_tile_image(
            &[1.0, 0.0, 0.5],
            &layer,
            None,
            MaskColorPreset::PinkCyan.colors(),
            3,
            0,
            0,
            3,
            1,
        );

        assert_eq!(
            image.pixels[0],
            Color32::from_rgba_unmultiplied(255, 48, 84, 155)
        );
        assert_eq!(image.pixels[1], Color32::TRANSPARENT);
        assert_eq!(
            image.pixels[2],
            Color32::from_rgba_unmultiplied(255, 48, 84, 78)
        );
    }

    #[test]
    fn full_mask_preview_subtract_panel_shows_base_and_active_subtract_mask() {
        let mut layer = LocalAdjustmentLayer::new(
            "mask",
            LocalMask::Full,
            LocalEffect::Tone(ToneParams::default()),
        );
        layer.manual_override.subtract = Some(RasterVectorMask {
            width: 3,
            height: 1,
            alpha: vec![0.0, 1.0, 0.0],
            shapes: Vec::new(),
        });

        let image = build_mask_tile_image(
            &[1.0, 0.8, 0.5],
            &layer,
            Some(OverrideEditTarget::Subtract),
            MaskColorPreset::PinkCyan.colors(),
            3,
            0,
            0,
            3,
            1,
        );

        assert_eq!(
            image.pixels[0],
            Color32::from_rgba_unmultiplied(255, 48, 84, 155)
        );
        assert_eq!(
            image.pixels[1],
            Color32::from_rgba_unmultiplied(64, 190, 255, 225)
        );
        assert_eq!(
            image.pixels[2],
            Color32::from_rgba_unmultiplied(255, 48, 84, 78)
        );
    }

    #[test]
    fn mask_preview_color_preset_changes_base_and_edit_colors() {
        let mut layer = LocalAdjustmentLayer::new(
            "mask",
            LocalMask::LumaRange(RangeMask::default()),
            LocalEffect::Tone(ToneParams::default()),
        );
        layer.manual_override.add = Some(RasterVectorMask {
            width: 2,
            height: 1,
            alpha: vec![1.0, 0.0],
            shapes: Vec::new(),
        });

        let image = build_mask_tile_image(
            &[0.5, 0.75],
            &layer,
            Some(OverrideEditTarget::Add),
            MaskColorPreset::CyanOrange.colors(),
            2,
            0,
            0,
            2,
            1,
        );

        assert_eq!(
            image.pixels[0],
            Color32::from_rgba_unmultiplied(255, 150, 40, 225)
        );
        assert_eq!(
            image.pixels[1],
            Color32::from_rgba_unmultiplied(0, 205, 255, 116)
        );
    }

    #[test]
    fn raster_vector_edit_controls_are_visible_for_override_masks() {
        assert!(raster_vector_edit_controls_visible(
            Some(MaskKind::Raster),
            None
        ));
        assert!(raster_vector_edit_controls_visible(
            Some(MaskKind::Subject),
            Some(OverrideEditTarget::Add)
        ));
        assert!(raster_vector_edit_controls_visible(
            Some(MaskKind::Segmentation),
            Some(OverrideEditTarget::Subtract)
        ));
        assert!(!raster_vector_edit_controls_visible(
            Some(MaskKind::Subject),
            None
        ));
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
    fn crop_resize_handle_shrinks_without_resetting_to_full() {
        let full = CropRect::full(100, 80);
        let crop = full.dragged(CropHandle::East, -20.0, 0.0, 100, 80, None);

        assert!(!crop.is_full(100, 80));
        assert_eq!(crop.min_x, 0.0);
        assert_eq!(crop.max_x, 80.0);
        assert_eq!(crop.max_y, 80.0);
    }

    #[test]
    fn crop_xywh_inputs_can_lock_aspect_ratio() {
        let crop = crop_from_xywh_inputs(0, 0, 160, 100, 200, 200, Some(16.0 / 9.0), false);

        assert_eq!(crop.min_x, 0.0);
        assert_eq!(crop.min_y, 0.0);
        assert_eq!(crop.max_x, 160.0);
        assert_eq!(crop.max_y, 90.0);
    }

    #[test]
    fn crop_from_points_creates_non_full_rect_from_full_state() {
        let crop = crop_from_points([10.0, 20.0], [70.0, 90.0], 100, 100, None);

        assert!(!crop.is_full(100, 100));
        assert_eq!(crop.min_x, 10.0);
        assert_eq!(crop.min_y, 20.0);
        assert_eq!(crop.max_x, 70.0);
        assert_eq!(crop.max_y, 90.0);
    }

    // image displayed 1:1 on screen at the origin, so screen coords == image coords.
    const TEST_IMG_RECT: Rect = Rect {
        min: Pos2 { x: 0.0, y: 0.0 },
        max: Pos2 { x: 200.0, y: 160.0 },
    };
    const TEST_HANDLE_HIT: f32 = 32.0;

    fn test_press_target(
        press: Pos2,
        crop: CropRect,
        crop_active: bool,
    ) -> Option<CropPressTarget> {
        let crop_screen = crop.to_screen_rect(TEST_IMG_RECT, 200, 160);
        let handle_bounds = TEST_IMG_RECT.shrink(14.0);
        let handle_points = crop_handle_points(crop_screen);
        crop_press_target(
            press,
            TEST_IMG_RECT,
            crop_screen,
            crop_active,
            &handle_points,
            handle_bounds,
            TEST_HANDLE_HIT,
        )
    }

    #[test]
    fn crop_press_target_starts_create_in_full_image_interior() {
        // From the full (no-crop) state, pressing inside the image must begin a create
        // drag, not silently fall through (the original symptom: drag did nothing).
        let target = test_press_target(Pos2::new(80.0, 60.0), CropRect::full(200, 160), false);
        assert_eq!(target, Some(CropPressTarget::Create));
    }

    #[test]
    fn crop_press_target_moves_active_crop_body() {
        let crop = CropRect {
            min_x: 40.0,
            min_y: 30.0,
            max_x: 160.0,
            max_y: 130.0,
        };
        let target = test_press_target(Pos2::new(100.0, 80.0), crop, true);
        assert_eq!(target, Some(CropPressTarget::Move));
    }

    #[test]
    fn crop_press_target_resizes_on_corner_handle() {
        let crop = CropRect {
            min_x: 40.0,
            min_y: 30.0,
            max_x: 160.0,
            max_y: 130.0,
        };
        // Press right on the south-east corner dot.
        let target = test_press_target(Pos2::new(160.0, 130.0), crop, true);
        assert_eq!(target, Some(CropPressTarget::Resize(CropHandle::SouthEast)));
    }

    #[test]
    fn crop_press_target_creates_outside_active_crop() {
        // Pressing in the darkened area outside an existing crop starts a fresh crop.
        let crop = CropRect {
            min_x: 40.0,
            min_y: 30.0,
            max_x: 160.0,
            max_y: 130.0,
        };
        let target = test_press_target(Pos2::new(10.0, 10.0), crop, true);
        assert_eq!(target, Some(CropPressTarget::Create));
    }

    #[test]
    fn crop_press_target_ignores_press_outside_image() {
        let target = test_press_target(Pos2::new(-5.0, -5.0), CropRect::full(200, 160), false);
        assert_eq!(target, None);
    }

    #[test]
    fn crop_create_drag_tracks_full_pointer_travel() {
        // Simulate the create gesture the way draw_crop_overlay does: the press latches a
        // start point, then every frame rebuilds the rect from (start, current). The rect
        // must follow the pointer the whole way instead of freezing after one frame (the
        // bug where crop_is_active flipped mid-drag and abandoned the gesture).
        let start = [30.0, 20.0];
        let trace = [[60.0, 50.0], [90.0, 80.0], [130.0, 110.0]];
        let mut last = CropRect::full(200, 160);
        for current in trace {
            last = crop_from_points(start, current, 200, 160, None);
        }
        assert!(!last.is_full(200, 160));
        assert_eq!(last.min_x, 30.0);
        assert_eq!(last.min_y, 20.0);
        assert_eq!(last.max_x, 130.0);
        assert_eq!(last.max_y, 110.0);
    }

    #[test]
    fn crop_resize_accumulates_total_delta_not_single_frame() {
        // draw_crop_overlay applies the cumulative drag delta (total_drag_delta) to the
        // base captured at press, not the per-frame drag_delta. With three -10px frames
        // the West-bound East handle must end 30px in, not 10px (the per-frame-on-fixed-
        // base bug would have stuck it near the start).
        let base = CropRect::full(200, 160);
        let total_delta_x = -30.0; // 3 frames * -10px, summed
        let next = base.dragged(CropHandle::East, total_delta_x, 0.0, 200, 160, None);
        assert_eq!(next.max_x, 170.0);
        assert!(!next.is_full(200, 160));
    }

    fn gesture_input(
        primary_pressed: bool,
        primary_down: bool,
        press_target: Option<CropPressTarget>,
        press_image: Option<Pos2>,
        current_image: Option<Pos2>,
        create_moved_enough: bool,
        base_at_press: CropRect,
        total_delta_image: (f32, f32),
    ) -> CropGestureInput {
        CropGestureInput {
            primary_pressed,
            primary_down,
            press_target,
            press_image,
            current_image,
            create_moved_enough,
            base_at_press,
            resize_aspect: None,
            create_aspect: None,
            total_delta_image,
            img_w: 200,
            img_h: 160,
        }
    }

    #[test]
    fn crop_gesture_create_survives_active_flip_and_tracks_pointer() {
        // The original bug: the first drag frame turned the rect non-full, which on the
        // next frame would abandon the create gesture. The reducer must keep it latched.
        let full = CropRect::full(200, 160);

        // Frame 0: press in the interior begins a create at (30,20); no travel yet.
        let (g0, rect0) = crop_gesture_step(
            CropGesture::Idle,
            &gesture_input(
                true,
                true,
                Some(CropPressTarget::Create),
                Some(Pos2::new(30.0, 20.0)),
                Some(Pos2::new(30.0, 20.0)),
                false,
                full,
                (0.0, 0.0),
            ),
        );
        assert!(matches!(g0, CropGesture::Create(_)));
        assert!(
            rect0.is_none(),
            "a press with no travel must not size the crop"
        );

        // Frame 1: dragged out to (130,110) — crop is now non-full (would have flipped
        // crop_active in the old code). Gesture stays a create and tracks the pointer.
        let (g1, rect1) = crop_gesture_step(
            g0,
            &gesture_input(
                false,
                true,
                None,
                None,
                Some(Pos2::new(130.0, 110.0)),
                true,
                full,
                (0.0, 0.0),
            ),
        );
        assert!(matches!(g1, CropGesture::Create(_)));
        let rect1 = rect1.expect("create should size the crop once moved");
        assert!(!rect1.is_full(200, 160));
        assert_eq!((rect1.min_x, rect1.min_y), (30.0, 20.0));
        assert_eq!((rect1.max_x, rect1.max_y), (130.0, 110.0));

        // Frame 2: release ends the gesture and leaves the rect untouched.
        let (g2, rect2) = crop_gesture_step(
            g1,
            &gesture_input(false, false, None, None, None, true, full, (0.0, 0.0)),
        );
        assert!(matches!(g2, CropGesture::Idle));
        assert!(rect2.is_none());
    }

    #[test]
    fn crop_gesture_click_without_drag_is_a_noop() {
        // Press + release with no travel must not create a 1px crop or disturb the rect.
        let full = CropRect::full(200, 160);
        let (g0, rect0) = crop_gesture_step(
            CropGesture::Idle,
            &gesture_input(
                true,
                true,
                Some(CropPressTarget::Create),
                Some(Pos2::new(80.0, 60.0)),
                Some(Pos2::new(80.0, 60.0)),
                false,
                full,
                (0.0, 0.0),
            ),
        );
        assert!(matches!(g0, CropGesture::Create(_)));
        assert!(rect0.is_none());
        let (g1, rect1) = crop_gesture_step(
            g0,
            &gesture_input(false, false, None, None, None, false, full, (0.0, 0.0)),
        );
        assert!(matches!(g1, CropGesture::Idle));
        assert!(rect1.is_none());
    }

    #[test]
    fn crop_gesture_resize_applies_cumulative_delta() {
        let full = CropRect::full(200, 160);
        // Press the east handle, then continue with a cumulative -30px delta.
        let (g0, _) = crop_gesture_step(
            CropGesture::Idle,
            &gesture_input(
                true,
                true,
                Some(CropPressTarget::Resize(CropHandle::East)),
                Some(Pos2::new(200.0, 80.0)),
                Some(Pos2::new(200.0, 80.0)),
                false,
                full,
                (0.0, 0.0),
            ),
        );
        assert!(matches!(g0, CropGesture::Resize(_)));
        let (_, rect) = crop_gesture_step(
            g0,
            &gesture_input(
                false,
                true,
                None,
                None,
                Some(Pos2::new(170.0, 80.0)),
                false,
                full,
                (-30.0, 0.0),
            ),
        );
        let rect = rect.expect("resize should update the crop");
        assert_eq!(rect.max_x, 170.0);
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
        let mask = build_region_segmentation(
            &source,
            None,
            RegionSegmentationScope::Full,
            8.0,
            1,
            255,
            255,
            0,
        )
        .unwrap();
        assert_eq!(mask.label_count(), 2);
        assert_ne!(mask.labels[0], mask.labels[width - 1]);
        assert!(mask.selected.iter().skip(1).all(|&selected| !selected));
    }

    #[test]
    fn region_segmentation_background_scope_excludes_subject_pixels() {
        let source = RgbaImageBuf::new(
            4,
            1,
            vec![
                220, 40, 40, 255, 220, 40, 40, 255, 40, 80, 230, 255, 40, 80, 230, 255,
            ],
        )
        .unwrap();
        let subject = RasterMask {
            width: 4,
            height: 1,
            alpha: vec![1.0, 1.0, 0.0, 0.0],
        };
        let mask = build_region_segmentation(
            &source,
            Some(&subject),
            RegionSegmentationScope::Background,
            8.0,
            1,
            255,
            255,
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
