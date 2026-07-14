//! comic_lab — a standalone egui/eframe playground for the speech-bubble /
//! text-annotation feature, built on `comic-core`.
//!
//! This is a prototype (like `local_adjust_lab`), NOT integration into the main
//! app. It loads a flat image, lets you add/edit text + bubble annotations in
//! image-pixel space (left panel = object list, right panel = details), and
//! renders them by baking the comic-core overlay (`comic_core::bake_overlay`)
//! at image resolution whenever the scene changes — the same pixels the export
//! would produce (WYSIWYG). The live egui pass only draws selection / tail
//! handles, not the text (egui text positioning could not match ab_glyph).
//!
//! Persistence: Save/Load write/read the object list as JSON to a
//! `<image>.comic.json` sidecar (analogous to local_adjust_lab's `.miv`).

use std::collections::{HashMap, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::mpsc;

use comic_core::{
    AnnotationKind, AnnotationObject, BubbleObject, BubbleShape, FillMode, FontSet, FrameStyle,
    IndicatorKind, InlineDir, Insets, LoadedFont, MarkupRule, MessageWindowObject, NamePlate,
    NamePlateMode, Orientation, PortraitSide, PortraitSlot, Rgba, RgbaOverlay, ShadowStyle,
    SizeMode, StampImages, StampObject, StampSource, StrokeStyle, Tail, TailKind, TextAlign,
    TextBlock, VAnchor, WindowPosition, bake_overlay, bake_overlay_with_stamps, bubble_geometry,
    effective_bubble_shape, effective_window_half_extents, layout_text, markup_rules_angle,
    markup_rules_brackets, markup_rules_white, nearest_base_t, resolve_tail_base, resolve_tail_tip,
    shape_renders_tail, tessellate_bubble,
};
use eframe::egui::{self, Color32, ColorImage, Pos2, Rect, Sense, TextureHandle, TextureOptions};
use serde::{Deserialize, Serialize};

mod stamp;
use stamp::{
    EMOJI_CATALOG, EmojiCategory, downscale_overlay, emoji_assets_available, load_recent_stamps,
    load_stamp_image, push_recent_stamp, save_recent_stamps, stamp_label, stamp_source_key,
};

const LEFT_W: f32 = 240.0;
const RIGHT_W: f32 = 308.0;
const ZOOM_MIN: f32 = 0.05;
const ZOOM_MAX: f32 = 16.0;
const HANDLE_R: f32 = 7.0;
const FILE_HISTORY_LIMIT: usize = 16;
const UNDO_CAP: usize = 100;
/// Max rasterized stamp edge (px) when pre-compositing a sticker texture — mirrors
/// comic-core's `draw_stamp` cap so a corrupt size can't OOM the upload.
const STAMP_MAX_PX: usize = 8192;

/// Font-picker card size (shared by the grid layout and the card renderer so the
/// column count, row height and per-card geometry stay in sync). The height
/// reserves a clear name band below the preview strip so the font name is never
/// clipped or drawn over the light sample.
const FONT_CARD_W: f32 = 220.0;
const FONT_CARD_H: f32 = 70.0;

/// Cache key for an outlined stamp's pre-composited sticker texture. Keyed on the
/// halo's *effective* params — color + integer dilation radius `rad` (the rounded,
/// capped width that actually shapes the halo) + display size + flips — NOT the
/// raw `width_px`. Two widths that round to the same `rad` yield an identical halo
/// and correctly share one texture; widths straddling a `.5` boundary get distinct
/// keys (and distinct padding). Identical duplicates collapse to one key.
fn sticker_key(
    source: &StampSource,
    color: Rgba,
    rad: i32,
    tw: usize,
    th: usize,
    flip_h: bool,
    flip_v: bool,
) -> String {
    format!(
        "{}|{:02x}{:02x}{:02x}{:02x}r{}|{}x{}|{}{}",
        stamp_source_key(source),
        color.r,
        color.g,
        color.b,
        color.a,
        rad,
        tw,
        th,
        flip_h as u8,
        flip_v as u8,
    )
}

fn main() -> eframe::Result<()> {
    let initial_path = std::env::args_os().nth(1).map(PathBuf::from);
    let mut options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("comic_lab")
            .with_inner_size([1360.0, 880.0])
            .with_drag_and_drop(true),
        vsync: false,
        ..Default::default()
    };
    options.wgpu_options.present_mode = eframe::wgpu::PresentMode::AutoNoVsync;
    eframe::run_native(
        "comic_lab",
        options,
        Box::new(move |cc| Ok(Box::new(ComicLab::new(cc, initial_path)))),
    )
}

/// Windows font catalog: (display label, path). Each is loaded at startup if the
/// file exists, and offered in the per-object font picker. "フォント追加..."
/// lets the user load any other TTF/OTF/TTC at runtime.
const FONT_CATALOG: &[(&str, &str)] = &[
    ("游ゴシック Medium", r"C:\Windows\Fonts\YuGothM.ttc"),
    ("游ゴシック", r"C:\Windows\Fonts\YuGothR.ttc"),
    ("メイリオ", r"C:\Windows\Fonts\meiryo.ttc"),
    ("MS ゴシック", r"C:\Windows\Fonts\msgothic.ttc"),
    ("游明朝", r"C:\Windows\Fonts\yumin.ttf"),
    ("MS 明朝", r"C:\Windows\Fonts\msmincho.ttc"),
    ("BIZ UDゴシック", r"C:\Windows\Fonts\BIZ-UDGothicR.ttc"),
    ("BIZ UD明朝", r"C:\Windows\Fonts\BIZ-UDMinchoM.ttc"),
    (
        "UDデジタル教科書体",
        r"C:\Windows\Fonts\UDDigiKyokashoN-R.ttc",
    ),
];

/// Optional lab-only bundled font directory. Drop OFL-compatible TTF/OTF/TTC
/// files here to make them available in the font picker and SFX presets without
/// installing them into Windows.
fn bundled_font_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("assets")
        .join("fonts")
}

fn enumerate_bundled_fonts() -> Vec<(String, PathBuf)> {
    let Ok(entries) = std::fs::read_dir(bundled_font_dir()) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_file() {
            continue;
        }
        let ext_ok = path
            .extension()
            .and_then(|e| e.to_str())
            .map(|e| {
                let e = e.to_ascii_lowercase();
                e == "ttf" || e == "ttc" || e == "otf"
            })
            .unwrap_or(false);
        if !ext_ok {
            continue;
        }
        let Some(stem) = path.file_stem().and_then(|s| s.to_str()) else {
            continue;
        };
        let label = stem.replace(['_', '-'], " ");
        out.push((label, path));
    }
    out.sort_by(|a, b| a.0.to_lowercase().cmp(&b.0.to_lowercase()));
    out
}

/// Enumerate installed Windows fonts from the registry. Reads both the
/// machine-wide (`HKLM`) and per-user (`HKCU`) `…\Fonts` keys; each value name
/// looks like `Yu Gothic Medium (TrueType)` and the data is a bare filename
/// (resolved against `C:\Windows\Fonts\`) or an absolute path (HKCU user fonts
/// live in `%LOCALAPPDATA%\Microsoft\Windows\Fonts`).
///
/// Returns a de-duplicated, name-sorted `Vec<(display_name, path)>` keeping only
/// `.ttf/.ttc/.otf`. When a value name carries multiple aliases joined by ` & `,
/// the first alias is used.
#[cfg(windows)]
fn enumerate_system_fonts() -> Vec<(String, PathBuf)> {
    use winreg::RegKey;
    use winreg::enums::{HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE};

    const FONTS_SUBKEY: &str = r"SOFTWARE\Microsoft\Windows NT\CurrentVersion\Fonts";
    let windows_fonts = PathBuf::from(r"C:\Windows\Fonts");

    let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
    let mut out: Vec<(String, PathBuf)> = Vec::new();

    for hive in [HKEY_LOCAL_MACHINE, HKEY_CURRENT_USER] {
        let Ok(key) = RegKey::predef(hive).open_subkey(FONTS_SUBKEY) else {
            continue;
        };
        for value in key.enum_values().flatten() {
            let (raw_name, _) = value;
            let data: String = match key.get_value(&raw_name) {
                Ok(d) => d,
                Err(_) => continue,
            };
            let name = clean_font_name(&raw_name);
            if name.is_empty() {
                continue;
            }
            // Resolve a bare filename against C:\Windows\Fonts; keep absolute as-is.
            let path = {
                let p = PathBuf::from(&data);
                if p.is_absolute() {
                    p
                } else {
                    windows_fonts.join(&data)
                }
            };
            let ext_ok = path
                .extension()
                .and_then(|e| e.to_str())
                .map(|e| {
                    let e = e.to_ascii_lowercase();
                    e == "ttf" || e == "ttc" || e == "otf"
                })
                .unwrap_or(false);
            if !ext_ok {
                continue;
            }
            let dedup_key = name.to_lowercase();
            if seen.insert(dedup_key) {
                out.push((name, path));
            }
        }
    }
    out.sort_by(|a, b| a.0.to_lowercase().cmp(&b.0.to_lowercase()));
    out
}

#[cfg(not(windows))]
fn enumerate_system_fonts() -> Vec<(String, PathBuf)> {
    Vec::new()
}

/// Strip the trailing ` (TrueType)` / `(OpenType)` / `(TrueType-Reserved…)`
/// suffix from a registry font value name, and keep only the first alias when
/// several are joined by ` & `.
fn clean_font_name(raw: &str) -> String {
    // Drop a trailing parenthesized type tag, e.g. " (TrueType)".
    let without_tag = match raw.rfind('(') {
        Some(idx) => raw[..idx].trim_end(),
        None => raw.trim_end(),
    };
    // Keep the first alias if multiple are joined with " & ".
    let first = without_tag.split(" & ").next().unwrap_or(without_tag);
    first.trim().to_string()
}

fn font_lookup_key(s: &str) -> String {
    s.chars()
        .filter(|c| !c.is_whitespace() && !matches!(c, '-' | '_' | '.'))
        .flat_map(char::to_lowercase)
        .collect()
}

fn font_name_matches_candidate(font_name: &str, candidate: &str) -> bool {
    let font_name = font_lookup_key(font_name);
    let candidate = font_lookup_key(candidate);
    !candidate.is_empty() && font_name.contains(&candidate)
}

/// Install the JP font bytes into egui so live text shows Japanese. Kept
/// self-contained (we don't pull in the main crate's ui_fonts).
fn configure_egui_fonts(ctx: &egui::Context, bytes: &[u8]) {
    let mut fonts = egui::FontDefinitions::default();
    fonts.font_data.insert(
        "comic_lab_jp".to_owned(),
        Arc::new(egui::FontData::from_owned(bytes.to_vec())),
    );
    for family in [egui::FontFamily::Proportional, egui::FontFamily::Monospace] {
        let fam = fonts.families.entry(family).or_default();
        fam.insert(0, "comic_lab_jp".to_owned());
    }
    ctx.set_fonts(fonts);
}

/// Dark visuals matching `local_adjust_lab` (panel 18/18/20, faint 32/32/34,
/// selection 45/96/140, white override text). Copied from that lab, minus its
/// custom text-family bits (comic_lab installs its own JP font separately).
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

/// Pin the dark theme (incl. ComboBox popups, which egui 0.33 can otherwise
/// resolve against the OS light style).
fn apply_dark_theme(ctx: &egui::Context) {
    ctx.set_theme(egui::ThemePreference::Dark);
    ctx.style_mut(|style| {
        style.visuals = lab_dark_visuals();
    });
}

struct LoadedImage {
    path: PathBuf,
    width: usize,
    height: usize,
}

/// Sidecar JSON document for the object list.
#[derive(serde::Serialize, serde::Deserialize)]
struct SidecarDoc {
    schema_version: u32,
    objects: Vec<AnnotationObject>,
}

/// View transform: image-pixel space <-> screen space.
#[derive(Clone, Copy)]
struct View {
    zoom: f32,
    /// Screen position of image pixel (0,0).
    offset: egui::Vec2,
}

impl View {
    fn img_to_screen(&self, p: (f32, f32)) -> Pos2 {
        Pos2::new(p.0 * self.zoom, p.1 * self.zoom) + self.offset
    }
    fn screen_to_img(&self, p: Pos2) -> (f32, f32) {
        let v = p - self.offset;
        (v.x / self.zoom, v.y / self.zoom)
    }
}

/// What handle (if any) the user is currently dragging.
#[derive(Clone, Copy, PartialEq)]
enum DragKind {
    None,
    Move,
    TailTip,
    TailBase,
    /// Resizing via a bubble corner handle (index 0..3: TL,TR,BR,BL).
    Corner(usize),
    /// Rotating via the rotation handle above the bubble.
    Rotate,
    Pan,
}

/// Script coverage classification of a font (from glyph coverage probing).
#[derive(Clone, Copy, PartialEq, Eq)]
enum FontScript {
    /// Has kana/kanji glyphs (can render Japanese).
    Japanese,
    /// Has Latin 'A' but not Japanese.
    Latin,
    /// Neither (symbol / other-script fonts).
    Other,
}

/// Category filter for the font-sample dialog.
#[derive(Clone, Copy, PartialEq, Eq)]
enum FontCategory {
    Japanese,
    Latin,
    All,
}

/// Detail-tab selection in the right properties panel. Global (one per app,
/// not per-object), mirroring 補正レイヤー's panel-section accent scheme.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PropTab {
    Serifu,
    Body,
    Tail,
    Deco,
}

impl PropTab {
    /// Category accent color (matches the left color-bar on the tab content).
    fn color(self) -> Color32 {
        match self {
            PropTab::Serifu => Color32::from_rgb(90, 170, 255), // 青
            PropTab::Body => Color32::from_rgb(95, 208, 140),   // 緑
            PropTab::Tail => Color32::from_rgb(255, 160, 60),   // 橙
            PropTab::Deco => Color32::from_rgb(255, 210, 75),   // 金
        }
    }

    fn label(self) -> &'static str {
        match self {
            PropTab::Serifu => "セリフ",
            PropTab::Body => "本体",
            PropTab::Tail => "しっぽ",
            PropTab::Deco => "飾り",
        }
    }
}

struct ComicLab {
    image: Option<LoadedImage>,
    source_texture: Option<TextureHandle>,
    baked_texture: Option<TextureHandle>,
    baked_dirty: bool,
    /// egui time (s) of the last bake, for drag throttling.
    last_bake_time: f64,
    /// Wall-clock duration (s) of the last bake. During a drag the re-bake
    /// interval adapts to this so a heavy scene (large/rotated stamps on a big
    /// image) doesn't saturate the UI thread with back-to-back full-res bakes.
    last_bake_dur: f64,
    /// Breakdown (ms) of the last bake: CPU composite vs GPU texture upload, for
    /// the on-canvas perf HUD (find what's slow).
    last_composite_ms: f64,
    last_upload_ms: f64,
    /// Show the perf HUD (bake timings) on the canvas. Toggle with F1.
    show_perf_hud: bool,

    fonts: FontSet,
    /// (key, display label) for the font picker. The key IS the display name for
    /// system fonts (so the picker filter and the FontSet share one identity).
    available_fonts: Vec<(String, String)>,
    /// Name → file path for every enumerated/known font (lazy-loaded on demand).
    font_paths: HashMap<String, PathBuf>,
    default_font_key: String,
    font_loaded: bool,
    status: String,

    /// Text-style presets (system `sys:*` first, then user `user:*`).
    text_presets: Vec<TextStylePreset>,
    /// Shape-style presets (system `sys:*` first, then user `user:*`).
    shape_presets: Vec<ShapeStylePreset>,
    /// Window-style presets (system `sys:*` first, then user `user:*`).
    window_presets: Vec<WindowStylePreset>,
    /// In-progress name in the 文字プリセット 登録 field.
    text_preset_name_input: String,
    /// In-progress name in the 形状プリセット 登録 field.
    shape_preset_name_input: String,
    /// In-progress name in the ウィンドウプリセット 登録 field.
    window_preset_name_input: String,

    objects: Vec<AnnotationObject>,
    next_id: u64,
    selected: Option<u64>,
    /// Tail preserved while "しっぽを表示" is off, so toggling doesn't move it.
    tail_stash: HashMap<u64, Tail>,
    /// Decoration layers preserved while "飾りを使う" is off, so toggling
    /// off→on restores the previous layers instead of starting empty.
    deco_stash: HashMap<u64, Vec<comic_core::DecorationLayer>>,
    recent_files: Vec<PathBuf>,

    // Undo/redo of the object list. `undo_baseline` is the last committed state;
    // edits coalesce into one entry that commits when the interaction settles.
    undo_stack: Vec<Vec<AnnotationObject>>,
    redo_stack: Vec<Vec<AnnotationObject>>,
    undo_baseline: Vec<AnnotationObject>,

    view: View,
    view_initialized: bool,
    drag: DragKind,
    /// Drag anchor in image space (for Move) or screen (for Pan).
    drag_img_anchor: (f32, f32),
    drag_pivot_anchor: (f32, f32),

    /// Whether the "吹き出し追加" preset-picker modal is open.
    show_add_dialog: bool,
    /// Whether the "ウィンドウ追加" preset-picker modal is open.
    show_add_window_dialog: bool,
    /// Whether the "オノマトペ追加" preset-picker modal is open.
    show_onomatopoeia_dialog: bool,

    /// Whether the font-sample picker modal is open + which object it targets.
    show_font_dialog: bool,
    font_dialog_target: Option<u64>,
    /// Filter text inside the font-sample dialog.
    font_dialog_filter: String,
    /// Category filter inside the font-sample dialog (default: Japanese).
    font_dialog_category: FontCategory,
    /// The sample string rendered in every font card.
    font_dialog_sample: String,
    /// Cache of rendered font-sample textures, keyed by (font key, sample text).
    /// Cleared when the sample text changes.
    font_sample_cache: HashMap<(String, String), TextureHandle>,
    /// Cache of rendered onomatopoeia preset thumbnails, keyed by
    /// (preset label + resolved font key). These thumbnails are baked through
    /// comic-core so the picker preview uses the actual OFL font face.
    onomatopoeia_thumb_cache: HashMap<String, TextureHandle>,
    /// Script classification per font key (filled in by a background thread so
    /// the UI never blocks parsing hundreds of fonts). Unknown = not yet done.
    font_script: HashMap<String, FontScript>,
    /// Receiver for background font classifications; None once draining is done.
    font_script_rx: Option<mpsc::Receiver<(String, FontScript)>>,

    /// Which detail tab (セリフ / 本体 / しっぽ / 飾り) is shown in the right
    /// panel. Global across objects (補正レイヤー convention).
    prop_tab: PropTab,

    /// True while a Japanese IME conversion is in progress. Used by
    /// `consume_ime_enter` to drop the Enter that confirms a conversion (so it
    /// doesn't leak into the multiline TextEdit as a newline).
    ime_composing: bool,

    // ----- Stamp (image sticker) state -----
    /// Decoded stamp pixels keyed by object id, handed to the baker. Rebuilt when
    /// stamp objects/sources change (see `ensure_stamp_images`).
    stamp_images: StampImages,
    /// Decode cache keyed by stamp source key (so the same emoji/file decodes
    /// once and is reused across objects). `None` = decode failed (don't retry).
    stamp_source_cache: HashMap<String, Option<Arc<RgbaOverlay>>>,
    /// Stamp source key -> GPU texture (raw image, no halo), uploaded once.
    /// Non-outlined stamps are drawn as GPU-transformed quads from this texture
    /// (scale/rotate/flip/opacity are ~free on the GPU) and EXCLUDED from the CPU
    /// bake entirely, so stamps never re-rasterize on the CPU — the bake stays
    /// cheap regardless of stamp count/size/rotation.
    stamp_textures: HashMap<String, TextureHandle>,
    /// Outlined ("sticker") stamps: a pre-composited image+halo texture, baked
    /// ONCE via `comic_core::composite_stamp_sticker` and reused as a GPU quad —
    /// so N duplicates of one outlined stamp cost one halo dilation, not N every
    /// bake. Keyed by source+outline(color/width)+display size+flips, so identical
    /// duplicates share one texture; resizing one re-bakes only that stamp. FIFO-
    /// capped (`sticker_order`) to bound GPU memory across a session of resizes.
    sticker_textures: HashMap<String, TextureHandle>,
    /// Insertion order of `sticker_textures` keys for FIFO eviction.
    sticker_order: VecDeque<String>,
    /// Stamp ids excluded from the last completed bake (= the GPU-quad stamps),
    /// sorted. When this set changes (stamp added/removed/outline toggled/z moved)
    /// the CPU bake must re-run.
    baked_excluded_set: Vec<u64>,
    /// Recently used stamps (MRU, persisted) for quick re-insert.
    recent_stamps: Vec<StampSource>,
    /// Whether the stamp picker dialog is open.
    show_stamp_dialog: bool,
    /// Search box text inside the stamp picker.
    stamp_dialog_filter: String,
    /// Active category tab in the stamp picker.
    stamp_dialog_category: EmojiCategory,
    /// Picker thumbnail textures keyed by source key (lazy, visible-only).
    stamp_thumb_cache: HashMap<String, TextureHandle>,
    /// When `Some(id)`, the picker replaces that stamp's source instead of
    /// inserting a new object ("別のスタンプに変更").
    stamp_dialog_replace_target: Option<u64>,
}

impl ComicLab {
    fn new(cc: &eframe::CreationContext<'_>, initial_path: Option<PathBuf>) -> Self {
        // Build the merged font catalog: enumerated system fonts (registry) +
        // the built-in FONT_CATALOG (so common JP fonts are always present even
        // if the registry read fails). Paths are recorded but NOT parsed here —
        // we lazy-load on demand (`ensure_font_loaded`) to keep startup fast.
        let mut font_paths: HashMap<String, PathBuf> = HashMap::new();
        let mut available_fonts: Vec<(String, String)> = Vec::new();

        // Lab-bundled fonts first: this lets OFL fonts in
        // tools/comic_lab/assets/fonts override same-named system entries for
        // onomatopoeia presets while keeping the normal default-font preference
        // below anchored to FONT_CATALOG.
        for (name, path) in enumerate_bundled_fonts() {
            if !font_paths.contains_key(&name) {
                font_paths.insert(name.clone(), path);
                available_fonts.push((name.clone(), name));
            }
        }
        // FONT_CATALOG next so its (often nicer) JP labels seed default_font_key.
        for (label, path) in FONT_CATALOG {
            let p = PathBuf::from(path);
            if !p.exists() {
                continue;
            }
            if !font_paths.contains_key(*label) {
                font_paths.insert((*label).to_string(), p);
                available_fonts.push(((*label).to_string(), (*label).to_string()));
            }
        }
        for (name, path) in enumerate_system_fonts() {
            if !font_paths.contains_key(&name) {
                font_paths.insert(name.clone(), path);
                available_fonts.push((name.clone(), name));
            }
        }
        available_fonts.sort_by(|a, b| a.1.to_lowercase().cmp(&b.1.to_lowercase()));

        // Classify each font's script coverage on a background thread (parsing
        // hundreds of fonts would freeze the UI). Results stream back via mpsc
        // and the right-panel list / dialog filter refine as they arrive.
        let font_script_rx = {
            let paths: Vec<(String, PathBuf)> = font_paths
                .iter()
                .map(|(k, p)| (k.clone(), p.clone()))
                .collect();
            let (tx, rx) = mpsc::channel();
            std::thread::spawn(move || {
                for (key, path) in paths {
                    let script = classify_font_file(&path);
                    if tx.send((key, script)).is_err() {
                        break; // receiver dropped (app closed)
                    }
                }
            });
            Some(rx)
        };

        // Default font key: prefer a built-in JP font that actually has a path.
        let default_font_key = FONT_CATALOG
            .iter()
            .map(|(label, _)| (*label).to_string())
            .find(|k| font_paths.contains_key(k))
            .or_else(|| available_fonts.first().map(|(k, _)| k.clone()))
            .unwrap_or_default();

        // Eagerly load just the default font (1 file) so live egui text + the
        // first object render Japanese immediately.
        let mut fonts = FontSet::new();
        let mut first_bytes: Option<Vec<u8>> = None;
        if let Some(path) = font_paths.get(&default_font_key) {
            if let Ok(bytes) = std::fs::read(path) {
                if LoadedFont::from_bytes(default_font_key.clone(), bytes.clone())
                    .map(|f| fonts.insert(f))
                    .is_ok()
                {
                    first_bytes = Some(bytes);
                }
            }
        }
        if let Some(bytes) = &first_bytes {
            configure_egui_fonts(&cc.egui_ctx, bytes);
        }
        // Match local_adjust_lab's dark look (after font setup so the JP font
        // family survives; apply_dark_theme only overrides visuals colors).
        apply_dark_theme(&cc.egui_ctx);
        let font_loaded = !fonts.is_empty();
        let status = if font_loaded {
            format!("フォント {} 種類を検出しました", available_fonts.len())
        } else {
            "Windows 日本語フォントが見つかりません (テキストは空になります)".to_string()
        };

        let user_presets = load_user_presets();
        let mut text_presets = system_text_presets(&default_font_key);
        text_presets.extend(user_presets.text);
        let mut shape_presets = system_shape_presets();
        shape_presets.extend(user_presets.shape);
        let mut window_presets = system_window_presets(&default_font_key);
        window_presets.extend(user_presets.window);

        let recent_files = load_recent_files();
        let mut app = ComicLab {
            image: None,
            source_texture: None,
            baked_texture: None,
            baked_dirty: true,
            last_bake_time: 0.0,
            last_bake_dur: 0.0,
            last_composite_ms: 0.0,
            last_upload_ms: 0.0,
            show_perf_hud: std::env::var_os("COMIC_LAB_PERF").is_some(),
            fonts,
            available_fonts,
            font_paths,
            default_font_key,
            font_loaded,
            status,
            text_presets,
            shape_presets,
            window_presets,
            text_preset_name_input: String::new(),
            shape_preset_name_input: String::new(),
            window_preset_name_input: String::new(),
            objects: Vec::new(),
            next_id: 1,
            selected: None,
            tail_stash: HashMap::new(),
            deco_stash: HashMap::new(),
            recent_files,
            undo_stack: Vec::new(),
            redo_stack: Vec::new(),
            undo_baseline: Vec::new(),
            view: View {
                zoom: 1.0,
                offset: egui::Vec2::ZERO,
            },
            view_initialized: false,
            drag: DragKind::None,
            drag_img_anchor: (0.0, 0.0),
            drag_pivot_anchor: (0.0, 0.0),
            show_add_dialog: false,
            show_add_window_dialog: false,
            show_onomatopoeia_dialog: false,
            show_font_dialog: false,
            font_dialog_target: None,
            font_dialog_filter: String::new(),
            font_dialog_category: FontCategory::Japanese,
            font_dialog_sample: String::new(),
            font_sample_cache: HashMap::new(),
            onomatopoeia_thumb_cache: HashMap::new(),
            font_script: HashMap::new(),
            font_script_rx,
            prop_tab: PropTab::Serifu,
            ime_composing: false,
            stamp_images: StampImages::new(),
            stamp_source_cache: HashMap::new(),
            stamp_textures: HashMap::new(),
            sticker_textures: HashMap::new(),
            sticker_order: VecDeque::new(),
            baked_excluded_set: Vec::new(),
            recent_stamps: load_recent_stamps(),
            show_stamp_dialog: false,
            stamp_dialog_filter: String::new(),
            stamp_dialog_category: EmojiCategory::Smileys,
            stamp_thumb_cache: HashMap::new(),
            stamp_dialog_replace_target: None,
        };
        if let Some(path) = initial_path {
            app.open_image(&cc.egui_ctx, &path);
        }
        app
    }

    fn open_image(&mut self, ctx: &egui::Context, path: &Path) {
        match image::open(path) {
            Ok(dyn_img) => {
                let rgba = dyn_img.to_rgba8();
                let (w, h) = rgba.dimensions();
                let color =
                    ColorImage::from_rgba_unmultiplied([w as usize, h as usize], rgba.as_raw());
                self.source_texture =
                    Some(ctx.load_texture("comic_source", color, TextureOptions::LINEAR));
                self.image = Some(LoadedImage {
                    path: path.to_path_buf(),
                    width: w as usize,
                    height: h as usize,
                });
                self.objects.clear();
                self.tail_stash.clear();
                self.deco_stash.clear();
                self.selected = None;
                self.next_id = 1;
                self.baked_texture = None;
                self.baked_dirty = true;
                self.view_initialized = false;
                self.status = format!("Loaded {} ({}x{})", path.display(), w, h);
                self.remember_recent_file(path);
                // Auto-load sidecar if present.
                self.load_sidecar(path);
                self.reset_history();
            }
            Err(e) => self.status = format!("Failed to open image: {e}"),
        }
    }

    fn remember_recent_file(&mut self, path: &Path) {
        push_recent_file(&mut self.recent_files, path);
        if let Err(e) = save_recent_files(&self.recent_files) {
            self.status = format!("履歴の保存に失敗: {e}");
        }
    }

    fn handle_dropped_files(&mut self, ctx: &egui::Context) {
        let dropped: Vec<PathBuf> = ctx.input(|i| {
            i.raw
                .dropped_files
                .iter()
                .filter_map(|f| f.path.clone())
                .collect()
        });
        if let Some(path) = dropped.first() {
            self.open_image(ctx, path);
        }
    }

    fn draw_menu_bar(&mut self, ctx: &egui::Context) {
        let recent_files = self.recent_files.clone();
        let current_path = self.image.as_ref().map(|img| img.path.clone());
        let has_image = current_path.is_some();
        let mut path_to_load: Option<PathBuf> = None;
        let mut do_save = false;
        let mut do_reload = false;

        egui::TopBottomPanel::top("comic_lab_menubar").show(ctx, |ui| {
            egui::MenuBar::new().ui(ui, |ui| {
                ui.menu_button("読み込み", |ui| {
                    if ui.button("画像を開く...").clicked() {
                        let mut dialog = rfd::FileDialog::new()
                            .add_filter("画像", &["png", "jpg", "jpeg", "bmp", "webp"])
                            .set_title("画像を選択");
                        if let Some(parent) = current_path.as_ref().and_then(|p| p.parent()) {
                            dialog = dialog.set_directory(parent);
                        }
                        path_to_load = dialog.pick_file();
                        ui.close();
                    }
                    ui.separator();
                    ui.label(egui::RichText::new("最近開いた画像").weak());
                    if recent_files.is_empty() {
                        ui.add_enabled(false, egui::Button::new("履歴はありません"));
                    } else {
                        for path in &recent_files {
                            let is_current = current_path
                                .as_ref()
                                .map(|c| recent_file_key(c) == recent_file_key(path))
                                .unwrap_or(false);
                            let button =
                                egui::Button::new(history_menu_label(path)).fill(if is_current {
                                    Color32::from_rgb(36, 112, 150)
                                } else {
                                    Color32::from_rgba_unmultiplied(70, 70, 70, 190)
                                });
                            if ui
                                .add_sized(egui::vec2(420.0, 24.0), button)
                                .on_hover_text(path.display().to_string())
                                .clicked()
                            {
                                path_to_load = Some(path.clone());
                                ui.close();
                            }
                        }
                    }
                });
                ui.menu_button("サイドカー", |ui| {
                    if ui
                        .add_enabled(has_image, egui::Button::new("保存 (.comic.json)"))
                        .clicked()
                    {
                        do_save = true;
                        ui.close();
                    }
                    if ui
                        .add_enabled(has_image, egui::Button::new("再読み込み"))
                        .clicked()
                    {
                        do_reload = true;
                        ui.close();
                    }
                });
            });
        });

        if do_save {
            self.save_sidecar();
        }
        if do_reload {
            if let Some(img) = &self.image {
                let p = img.path.clone();
                self.load_sidecar(&p);
            }
        }
        if let Some(path) = path_to_load {
            self.open_image(ctx, &path);
        }
    }

    fn sidecar_path(&self) -> Option<PathBuf> {
        let img = self.image.as_ref()?;
        let name = img
            .path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("image");
        Some(img.path.with_file_name(format!("{name}.comic.json")))
    }

    fn save_sidecar(&mut self) {
        let Some(path) = self.sidecar_path() else {
            self.status = "No image loaded.".to_string();
            return;
        };
        let doc = SidecarDoc {
            schema_version: 1,
            objects: self.objects.clone(),
        };
        match serde_json::to_string_pretty(&doc) {
            Ok(json) => match std::fs::write(&path, json) {
                Ok(()) => self.status = format!("Saved {}", path.display()),
                Err(e) => self.status = format!("Save failed: {e}"),
            },
            Err(e) => self.status = format!("Serialize failed: {e}"),
        }
    }

    fn load_sidecar(&mut self, image_path: &Path) {
        let name = image_path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("image");
        let path = image_path.with_file_name(format!("{name}.comic.json"));
        let Ok(text) = std::fs::read_to_string(&path) else {
            return;
        };
        match serde_json::from_str::<SidecarDoc>(&text) {
            Ok(doc) => {
                self.next_id = doc.objects.iter().map(|o| o.id).max().unwrap_or(0) + 1;
                self.objects = doc.objects;
                self.normalize_z();
                self.tail_stash.clear();
                self.selected = self.objects.first().map(|o| o.id);
                self.reset_history();
                self.baked_dirty = true;
                self.status = format!("Loaded sidecar {}", path.display());
            }
            Err(e) => self.status = format!("Sidecar load failed: {e}"),
        }
    }

    fn add_text(&mut self) {
        let tb = TextBlock {
            text: "テキスト".to_string(),
            font_key: self.default_font_key.clone(),
            size_px: 48.0,
            color: Rgba::WHITE,
            orientation: Orientation::Vertical,
            markup_enabled: true,
            outline: Some(StrokeStyle {
                color: Rgba::BLACK,
                width_px: 3.0,
            }),
            ..TextBlock::default()
        };
        let center = self.default_new_pivot();
        let (w, h) = self.text_layout_size(&tb);
        let pivot = (center.0 - w * 0.5, center.1 - h * 0.5);
        let id = self.next_id;
        self.next_id += 1;
        let mut obj = AnnotationObject::new_text(id, pivot, tb);
        obj.z = self.objects.len() as i32;
        self.objects.push(obj);
        self.normalize_z();
        self.selected = Some(id);
        self.baked_dirty = true;
    }

    fn add_onomatopoeia_preset(&mut self, preset: OnomatopoeiaPreset) {
        let font_key = self.resolve_onomatopoeia_font(preset.font_candidate());
        self.ensure_font_loaded(&font_key);
        let tb = preset.build_text(&font_key);
        let center = self.default_new_pivot();
        let (w, h) = self.text_layout_size(&tb);
        let pivot = (center.0 - w * 0.5, center.1 - h * 0.5);
        let id = self.next_id;
        self.next_id += 1;
        let mut obj = AnnotationObject::new_text(id, pivot, tb);
        obj.rotation_rad = preset.rotation_rad();
        obj.z = self.objects.len() as i32;
        self.objects.push(obj);
        self.normalize_z();
        self.selected = Some(id);
        self.baked_dirty = true;
        self.status = format!("オノマトペ「{}」を追加しました ({font_key})", preset.text());
    }

    fn resolve_onomatopoeia_font(&self, candidate: &str) -> String {
        if let Some((key, _)) = self.available_fonts.iter().find(|(key, label)| {
            font_name_matches_candidate(key, candidate)
                || font_name_matches_candidate(label, candidate)
        }) {
            return key.clone();
        }
        self.default_font_key.clone()
    }

    /// Insert a new bubble built from `preset` at the default pivot.
    fn add_bubble_preset(&mut self, preset: BubblePreset) {
        let pivot = self.default_new_pivot();
        let bubble = preset.build_bubble(pivot, &self.default_font_key);
        let id = self.next_id;
        self.next_id += 1;
        let mut obj = AnnotationObject::new_bubble(id, pivot, bubble);
        obj.z = self.objects.len() as i32;
        self.objects.push(obj);
        self.normalize_z();
        self.selected = Some(id);
        self.baked_dirty = true;
    }

    /// Insert a new message window (default DQ-ish bottom full-width panel).
    fn add_message_window(&mut self) {
        let mut win = MessageWindowObject::default();
        win.text.text = "メッセージ".to_string();
        win.text.font_key = self.default_font_key.clone();
        win.name_plate.name.font_key = self.default_font_key.clone();
        win.name_plate.name.text = "名前".to_string();
        let pivot = self.default_new_pivot();
        let id = self.next_id;
        self.next_id += 1;
        let mut obj = AnnotationObject::new_message_window(id, pivot, win);
        obj.z = self.objects.len() as i32;
        self.objects.push(obj);
        self.normalize_z();
        self.selected = Some(id);
        self.apply_window_placement(id);
        self.baked_dirty = true;
    }

    /// Insert a window, then apply window-style preset `idx` (used by the add
    /// dialog's preset picker).
    fn add_message_window_with_preset(&mut self, idx: usize) {
        self.add_message_window();
        self.apply_window_preset_by_index(idx);
    }

    /// Decode (caching) a stamp source and return its pixel aspect (w/h). 1.0 if
    /// the image can't be decoded.
    fn stamp_source_aspect(&mut self, source: &StampSource) -> f32 {
        let key = stamp_source_key(source);
        let entry = self
            .stamp_source_cache
            .entry(key)
            .or_insert_with(|| load_stamp_image(source).map(Arc::new));
        match entry {
            Some(o) if o.w > 0 && o.h > 0 => o.w as f32 / o.h as f32,
            _ => 1.0,
        }
    }

    /// Insert a new image stamp at the default pivot, sized ~192px on its long
    /// edge with the source's aspect ratio. Records it in the recent list.
    fn add_stamp(&mut self, source: StampSource) {
        let pivot = self.default_new_pivot();
        let aspect = self.stamp_source_aspect(&source).max(1e-3);
        let long = 96.0_f32; // half-extent of the long edge (~192px on canvas)
        let (half_w, half_h) = if aspect >= 1.0 {
            (long, long / aspect)
        } else {
            (long * aspect, long)
        };
        let stamp = StampObject {
            source: source.clone(),
            half_w,
            half_h: half_h.max(8.0),
            ..StampObject::default()
        };
        let id = self.next_id;
        self.next_id += 1;
        let mut obj = AnnotationObject::new_stamp(id, pivot, stamp);
        obj.z = self.objects.len() as i32;
        self.objects.push(obj);
        self.normalize_z();
        self.selected = Some(id);
        push_recent_stamp(&mut self.recent_stamps, &source);
        save_recent_stamps(&self.recent_stamps);
        self.baked_dirty = true;
    }

    /// Replace the source of an existing stamp (keeps geometry, re-fits aspect).
    fn replace_stamp_source(&mut self, id: u64, source: StampSource) {
        let aspect = self.stamp_source_aspect(&source).max(1e-3);
        if let Some(obj) = self.objects.iter_mut().find(|o| o.id == id) {
            if let AnnotationKind::Stamp(s) = &mut obj.kind {
                // Keep the long-edge size, re-fit the short edge to the new aspect.
                let long = s.half_w.max(s.half_h);
                if aspect >= 1.0 {
                    s.half_w = long;
                    s.half_h = (long / aspect).max(8.0);
                } else {
                    s.half_h = long;
                    s.half_w = (long * aspect).max(8.0);
                }
                s.source = source.clone();
            }
        }
        push_recent_stamp(&mut self.recent_stamps, &source);
        save_recent_stamps(&self.recent_stamps);
        self.baked_dirty = true;
    }

    /// Apply a chosen stamp source from the picker: replace the dialog's target
    /// stamp, or insert a new one.
    fn choose_stamp_source(&mut self, source: StampSource) {
        match self.stamp_dialog_replace_target.take() {
            Some(id) => self.replace_stamp_source(id, source),
            None => self.add_stamp(source),
        }
        self.show_stamp_dialog = false;
    }

    /// Ensure a small picker thumbnail texture exists for `source`.
    fn ensure_stamp_thumb(&mut self, ctx: &egui::Context, source: &StampSource) {
        let key = stamp_source_key(source);
        if self.stamp_thumb_cache.contains_key(&key) {
            return;
        }
        let full = self
            .stamp_source_cache
            .entry(key.clone())
            .or_insert_with(|| load_stamp_image(source).map(Arc::new))
            .clone();
        let Some(full) = full else {
            return;
        };
        let thumb = downscale_overlay(&full, 44);
        let color = ColorImage::from_rgba_unmultiplied([thumb.w, thumb.h], &thumb.pixels);
        let tex = ctx.load_texture(format!("stamp_thumb_{key}"), color, TextureOptions::LINEAR);
        self.stamp_thumb_cache.insert(key, tex);
    }

    /// Right-panel properties for a selected stamp (size / opacity / flips /
    /// sticker outline / replace source).
    fn draw_stamp_properties(&mut self, ui: &mut egui::Ui, sel: u64) {
        ui.label("種類: スタンプ");
        ui.separator();
        if let Some(AnnotationKind::Stamp(s)) =
            self.objects.iter().find(|o| o.id == sel).map(|o| &o.kind)
        {
            ui.label(format!("画像: {}", stamp_label(&s.source)));
        }
        if ui.button("別のスタンプに変更…").clicked() {
            self.stamp_dialog_replace_target = Some(sel);
            self.show_stamp_dialog = true;
        }
        ui.separator();

        let mut dirty = false;
        if let Some(obj) = self.objects.iter_mut().find(|o| o.id == sel) {
            if let AnnotationKind::Stamp(s) = &mut obj.kind {
                let aspect = if s.half_h > 1e-3 {
                    s.half_w / s.half_h
                } else {
                    1.0
                };
                let mut long = s.half_w.max(s.half_h) * 2.0;
                ui.horizontal(|ui| {
                    ui.label("大きさ");
                    if ui
                        .add(egui::Slider::new(&mut long, 16.0..=1600.0).suffix("px"))
                        .changed()
                    {
                        let half_long = (long * 0.5).max(8.0);
                        if aspect >= 1.0 {
                            s.half_w = half_long;
                            s.half_h = (half_long / aspect).max(8.0);
                        } else {
                            s.half_h = half_long;
                            s.half_w = (half_long * aspect).max(8.0);
                        }
                        dirty = true;
                    }
                });
                ui.horizontal(|ui| {
                    ui.label("不透明度");
                    if ui
                        .add(egui::Slider::new(&mut s.opacity, 0.0..=1.0))
                        .changed()
                    {
                        dirty = true;
                    }
                });
                ui.horizontal(|ui| {
                    if ui.checkbox(&mut s.flip_h, "左右反転").changed() {
                        dirty = true;
                    }
                    if ui.checkbox(&mut s.flip_v, "上下反転").changed() {
                        dirty = true;
                    }
                });
                let mut has_outline = s.outline.is_some();
                if ui
                    .checkbox(&mut has_outline, "縁取り (ステッカー風)")
                    .changed()
                {
                    s.outline = if has_outline {
                        Some(StrokeStyle {
                            color: Rgba::WHITE,
                            width_px: 6.0,
                        })
                    } else {
                        None
                    };
                    dirty = true;
                }
                if let Some(o) = &mut s.outline {
                    ui.horizontal(|ui| {
                        ui.label("色");
                        let mut c = [o.color.r, o.color.g, o.color.b];
                        if ui.color_edit_button_srgb(&mut c).changed() {
                            o.color = Rgba::new(c[0], c[1], c[2], 255);
                            dirty = true;
                        }
                        ui.label("太さ");
                        if ui
                            .add(egui::Slider::new(&mut o.width_px, 0.0..=40.0))
                            .changed()
                        {
                            dirty = true;
                        }
                    });
                }
            }
        }
        if dirty {
            self.baked_dirty = true;
        }
    }

    /// The stamp picker dialog: category tabs + search + recent row + emoji grid,
    /// plus "画像ファイルから追加" for user images. Click inserts (or replaces, when
    /// opened via "別のスタンプに変更").
    fn draw_stamp_dialog(&mut self, ctx: &egui::Context) {
        if !self.show_stamp_dialog {
            return;
        }
        let assets = emoji_assets_available();
        let filter = self.stamp_dialog_filter.to_lowercase();
        let cat = self.stamp_dialog_category;
        // Visible entries: when searching, ignore the category tab.
        let visible: Vec<(&'static str, &'static str)> = EMOJI_CATALOG
            .iter()
            .filter(|e| !e.name.is_empty())
            .filter(|e| {
                if filter.is_empty() {
                    e.category == cat
                } else {
                    e.name.to_lowercase().contains(&filter) || e.key.contains(&filter)
                }
            })
            .map(|e| (e.key, e.name))
            .collect();

        // Pre-build thumbnails (needs &mut self) before the read-only closure.
        if assets {
            for (key, _) in &visible {
                self.ensure_stamp_thumb(ctx, &StampSource::Emoji((*key).to_string()));
            }
        }
        let recents = self.recent_stamps.clone();
        for s in &recents {
            self.ensure_stamp_thumb(ctx, s);
        }
        let thumbs = self.stamp_thumb_cache.clone();
        let replacing = self.stamp_dialog_replace_target.is_some();

        let mut open = true;
        let mut chosen: Option<StampSource> = None;
        let mut pick_file = false;
        let mut filter_local = self.stamp_dialog_filter.clone();
        let mut cat_local = cat;

        let title = if replacing {
            "スタンプを変更"
        } else {
            "スタンプを追加"
        };
        egui::Window::new(title)
            .open(&mut open)
            .collapsible(false)
            .resizable(true)
            .default_size([560.0, 460.0])
            .default_pos(ctx.content_rect().min + egui::vec2(60.0, 40.0))
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    if ui.button("画像ファイルから追加…").clicked() {
                        pick_file = true;
                    }
                    ui.separator();
                    ui.label("検索");
                    ui.add(
                        egui::TextEdit::singleline(&mut filter_local)
                            .hint_text("名前 / コード")
                            .desired_width(160.0),
                    );
                });

                // Recent row.
                if !recents.is_empty() {
                    ui.add_space(4.0);
                    ui.label(egui::RichText::new("最近使った").weak());
                    ui.horizontal_wrapped(|ui| {
                        for s in &recents {
                            let key = stamp_source_key(s);
                            let resp = if let Some(tex) = thumbs.get(&key) {
                                ui.add(
                                    egui::Button::image(egui::load::SizedTexture::new(
                                        tex.id(),
                                        egui::vec2(34.0, 34.0),
                                    ))
                                    .corner_radius(4.0),
                                )
                            } else {
                                ui.button(stamp_label(s))
                            };
                            if resp.on_hover_text(stamp_label(s)).clicked() {
                                chosen = Some(s.clone());
                            }
                        }
                    });
                    ui.separator();
                }

                // Category tabs (hidden while searching).
                if filter_local.is_empty() {
                    ui.horizontal_wrapped(|ui| {
                        for &c in EmojiCategory::all() {
                            ui.selectable_value(&mut cat_local, c, c.label());
                        }
                    });
                    ui.add_space(2.0);
                }

                if !assets {
                    ui.colored_label(
                        Color32::from_rgb(220, 180, 90),
                        "絵文字アセット未配置: scripts/setup-twemoji.sh で取得 (画像ファイルからは追加できます)",
                    );
                }

                // Emoji grid.
                egui::ScrollArea::vertical()
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        ui.horizontal_wrapped(|ui| {
                            for (key, name) in &visible {
                                let src_key = format!("e:{key}");
                                let resp = if let Some(tex) = thumbs.get(&src_key) {
                                    ui.add(
                                        egui::Button::image(egui::load::SizedTexture::new(
                                            tex.id(),
                                            egui::vec2(40.0, 40.0),
                                        ))
                                        .corner_radius(4.0),
                                    )
                                } else {
                                    // No asset (or decode failed): a compact text chip.
                                    ui.add_sized(
                                        [44.0, 44.0],
                                        egui::Button::new(
                                            egui::RichText::new(*key).size(9.0).weak(),
                                        ),
                                    )
                                };
                                if resp.on_hover_text(*name).clicked() {
                                    chosen = Some(StampSource::Emoji((*key).to_string()));
                                }
                            }
                        });
                    });
            });

        // Write back the edited filter / category.
        self.stamp_dialog_filter = filter_local;
        self.stamp_dialog_category = cat_local;

        if pick_file {
            if let Some(path) = rfd::FileDialog::new()
                .add_filter("画像", &["png", "jpg", "jpeg", "webp", "gif", "bmp"])
                .pick_file()
            {
                chosen = Some(StampSource::File(path));
            }
        }
        if let Some(src) = chosen {
            self.choose_stamp_source(src);
        }
        if !open {
            self.show_stamp_dialog = false;
            self.stamp_dialog_replace_target = None;
        }
    }

    /// Resolve a window's `position` + `size_mode` against the image into its
    /// concrete pivot (center) + `half_w` (for FullWidth). Called on creation and
    /// whenever the position / size / margin controls change. No-op without an
    /// image or for `Free` placement / non-windows.
    fn apply_window_placement(&mut self, id: u64) {
        let Some((iw, ih)) = self
            .image
            .as_ref()
            .map(|i| (i.width as f32, i.height as f32))
        else {
            return;
        };
        let Some(idx) = self.objects.iter().position(|o| o.id == id) else {
            return;
        };
        let (hh, pos, size_mode, margin) = match &self.objects[idx].kind {
            AnnotationKind::MessageWindow(w) => {
                let (_hw, hh) = effective_window_half_extents(w, &self.fonts);
                (hh, w.position, w.size_mode, w.margin_px)
            }
            _ => return,
        };
        // Free placement is fully manual — never reposition or re-width it (a
        // dragged window keeps exactly where/what the user left it).
        if matches!(pos, WindowPosition::Free) {
            return;
        }
        let mut px = self.objects[idx].pivot.0;
        let mut py = self.objects[idx].pivot.1;
        if matches!(size_mode, SizeMode::FullWidth) {
            let new_hw = (iw * 0.5 - margin).max(40.0);
            px = iw * 0.5;
            if let AnnotationKind::MessageWindow(w) = &mut self.objects[idx].kind {
                w.half_w = new_hw;
            }
        }
        match pos {
            WindowPosition::Top => py = margin + hh,
            WindowPosition::Middle | WindowPosition::Center => py = ih * 0.5,
            WindowPosition::Bottom => py = ih - margin - hh,
            WindowPosition::Free => {}
        }
        self.objects[idx].pivot = (px, py);
    }

    fn default_new_pivot(&self) -> (f32, f32) {
        match &self.image {
            Some(img) => (img.width as f32 * 0.5, img.height as f32 * 0.5),
            None => (300.0, 300.0),
        }
    }

    /// Reset undo/redo to the current object state (called on image / sidecar load).
    fn reset_history(&mut self) {
        self.undo_stack.clear();
        self.redo_stack.clear();
        self.undo_baseline = self.objects.clone();
    }

    /// Commit a coalesced undo entry when the object state differs from the
    /// last committed baseline. Called once edits settle (no drag / text focus).
    fn commit_pending(&mut self) {
        if self.objects != self.undo_baseline {
            self.undo_stack.push(self.undo_baseline.clone());
            if self.undo_stack.len() > UNDO_CAP {
                self.undo_stack.remove(0);
            }
            self.redo_stack.clear();
            self.undo_baseline = self.objects.clone();
        }
    }

    fn do_undo(&mut self) {
        self.commit_pending();
        if let Some(prev) = self.undo_stack.pop() {
            self.redo_stack
                .push(std::mem::replace(&mut self.objects, prev));
            self.undo_baseline = self.objects.clone();
            self.after_history_change();
        }
    }

    fn do_redo(&mut self) {
        if let Some(next) = self.redo_stack.pop() {
            self.undo_stack
                .push(std::mem::replace(&mut self.objects, next));
            self.undo_baseline = self.objects.clone();
            self.after_history_change();
        }
    }

    fn after_history_change(&mut self) {
        if let Some(sel) = self.selected {
            if !self.objects.iter().any(|o| o.id == sel) {
                self.selected = None;
            }
        }
        // Drop stashed (hidden) tails for objects that no longer exist, so the
        // stash can't desync after an undo/redo or a delete.
        self.tail_stash
            .retain(|sid, _| self.objects.iter().any(|o| o.id == *sid));
        self.baked_dirty = true;
    }

    /// Re-pack `z` into a dense, unique 0..n sequence following the current
    /// (z, id) order. Keeping `z` unique makes the object list, the bake order,
    /// and hit-testing agree (Codex P2: tie handling was inconsistent).
    fn normalize_z(&mut self) {
        let mut idx: Vec<usize> = (0..self.objects.len()).collect();
        idx.sort_by(|&a, &b| {
            self.objects[a]
                .z
                .cmp(&self.objects[b].z)
                .then(self.objects[a].id.cmp(&self.objects[b].id))
        });
        for (z, &i) in idx.iter().enumerate() {
            self.objects[i].z = z as i32;
        }
    }

    fn selected_obj_mut(&mut self) -> Option<&mut AnnotationObject> {
        let sel = self.selected?;
        self.objects.iter_mut().find(|o| o.id == sel)
    }

    fn text_layout_size(&self, t: &TextBlock) -> (f32, f32) {
        self.fonts
            .get(&t.font_key)
            .map(|font| {
                let layout = layout_text(t, font);
                (layout.bounds.0.max(8.0), layout.bounds.1.max(8.0))
            })
            .unwrap_or((100.0, 40.0))
    }

    fn text_rotation_center(&self, obj: &AnnotationObject, t: &TextBlock) -> (f32, f32) {
        let (w, h) = self.text_layout_size(t);
        (obj.pivot.0 + w * 0.5, obj.pivot.1 + h * 0.5)
    }

    fn rotation_center(&self, obj: &AnnotationObject) -> (f32, f32) {
        match &obj.kind {
            AnnotationKind::Text(t) => self.text_rotation_center(obj, t),
            _ => obj.pivot,
        }
    }

    /// Approximate image-space bounds of an object (for hit-testing / select).
    fn object_bounds(&self, obj: &AnnotationObject) -> Rect {
        match &obj.kind {
            AnnotationKind::Bubble(b) => {
                // Bounds over the actual baked geometry (effective/auto-sized
                // outline incl. the spliced spike tail, thought circles, and the
                // tail tip) so clicking the tail / spikes selects the bubble.
                // Expanded by the stroke half-width + a little slack.
                let eff = effective_bubble_shape(b, &self.fonts);
                // Tailless shapes (line fields / 意識 / なし) render no tail, so a
                // stale tail must not splice into the hit region or inflate bounds.
                let tail = b.tail.as_ref().filter(|_| shape_renders_tail(&eff));
                let geo = bubble_geometry(&eff, obj.pivot, tail);
                let mut min = (f32::INFINITY, f32::INFINITY);
                let mut max = (f32::NEG_INFINITY, f32::NEG_INFINITY);
                let pivot = obj.pivot;
                let rot = obj.rotation_rad;
                // Plain min/max accumulator; callers rotate points before adding,
                // so the AABB encloses the baked (rotated) bubble (hit-test ≈
                // visible pixels).
                let mut acc = |x: f32, y: f32| {
                    min.0 = min.0.min(x);
                    min.1 = min.1.min(y);
                    max.0 = max.0.max(x);
                    max.1 = max.1.max(y);
                };
                for &(x, y) in &geo.outline {
                    let (px, py) = rotate_about((x, y), pivot, rot);
                    acc(px, py);
                }
                // A circle is rotation-invariant: rotate its center, expand by r.
                for &(cx, cy, r) in &geo.thought {
                    let (rcx, rcy) = rotate_about((cx, cy), pivot, rot);
                    acc(rcx - r, rcy - r);
                    acc(rcx + r, rcy + r);
                }
                if let Some(t) = tail {
                    // Drawn tip (kept outside the auto-sized outline), not the raw
                    // stored tip, so the AABB matches the visible spike.
                    let drawn_tip = resolve_tail_tip(&eff, pivot, t);
                    let (px, py) = rotate_about(drawn_tip, pivot, rot);
                    acc(px, py);
                }
                if min.0 > max.0 {
                    // Degenerate fallback (no geometry): a small box at the pivot.
                    return Rect::from_center_size(
                        Pos2::new(obj.pivot.0, obj.pivot.1),
                        egui::vec2(20.0, 20.0),
                    );
                }
                let m = b.outline.width_px.max(0.0) * 0.5 + 2.0;
                Rect::from_min_max(
                    Pos2::new(min.0 - m, min.1 - m),
                    Pos2::new(max.0 + m, max.1 + m),
                )
            }
            AnnotationKind::Text(t) => {
                let (w, h) = self.text_layout_size(t);
                let center = (obj.pivot.0 + w * 0.5, obj.pivot.1 + h * 0.5);
                let mut min = (f32::INFINITY, f32::INFINITY);
                let mut max = (f32::NEG_INFINITY, f32::NEG_INFINITY);
                for &(lx, ly) in &[
                    (-w * 0.5, -h * 0.5),
                    (w * 0.5, -h * 0.5),
                    (w * 0.5, h * 0.5),
                    (-w * 0.5, h * 0.5),
                ] {
                    let p = rotate_about((center.0 + lx, center.1 + ly), center, obj.rotation_rad);
                    min.0 = min.0.min(p.0);
                    min.1 = min.1.min(p.1);
                    max.0 = max.0.max(p.0);
                    max.1 = max.1.max(p.1);
                }
                let m = t.outline.map(|s| s.width_px).unwrap_or(0.0).max(0.0) + 2.0;
                Rect::from_min_max(
                    Pos2::new(min.0 - m, min.1 - m),
                    Pos2::new(max.0 + m, max.1 + m),
                )
            }
            AnnotationKind::MessageWindow(w) => {
                // Rotated panel rect (pivot = center), expanded by the stroke
                // half-width — same accumulator pattern as the bubble arm.
                let (hw, hh) = effective_window_half_extents(w, &self.fonts);
                let pivot = obj.pivot;
                let rot = obj.rotation_rad;
                let mut min = (f32::INFINITY, f32::INFINITY);
                let mut max = (f32::NEG_INFINITY, f32::NEG_INFINITY);
                for &(lx, ly) in &[(-hw, -hh), (hw, -hh), (hw, hh), (-hw, hh)] {
                    let (px, py) = rotate_about((pivot.0 + lx, pivot.1 + ly), pivot, rot);
                    min.0 = min.0.min(px);
                    min.1 = min.1.min(py);
                    max.0 = max.0.max(px);
                    max.1 = max.1.max(py);
                }
                let m = w.outline.width_px.max(0.0) * 0.5 + 2.0;
                Rect::from_min_max(
                    Pos2::new(min.0 - m, min.1 - m),
                    Pos2::new(max.0 + m, max.1 + m),
                )
            }
            AnnotationKind::Stamp(s) => {
                // Rotated rect (pivot = center) expanded by the sticker outline.
                let (hw, hh) = (s.half_w, s.half_h);
                let pivot = obj.pivot;
                let rot = obj.rotation_rad;
                let mut min = (f32::INFINITY, f32::INFINITY);
                let mut max = (f32::NEG_INFINITY, f32::NEG_INFINITY);
                for &(lx, ly) in &[(-hw, -hh), (hw, -hh), (hw, hh), (-hw, hh)] {
                    let (px, py) = rotate_about((pivot.0 + lx, pivot.1 + ly), pivot, rot);
                    min.0 = min.0.min(px);
                    min.1 = min.1.min(py);
                    max.0 = max.0.max(px);
                    max.1 = max.1.max(py);
                }
                let m = s.outline.map(|o| o.width_px).unwrap_or(0.0).max(0.0) + 2.0;
                Rect::from_min_max(
                    Pos2::new(min.0 - m, min.1 - m),
                    Pos2::new(max.0 + m, max.1 + m),
                )
            }
        }
    }

    /// Lazily parse + register the font named `key` if it isn't already in the
    /// FontSet but we know its path. Parse failures are ignored (the FontSet
    /// `get` fallback then resolves to the default font). Keeps startup fast by
    /// not parsing hundreds of fonts up front.
    fn ensure_font_loaded(&mut self, key: &str) {
        if key.is_empty() {
            return;
        }
        // `FontSet::get` falls back to the default font for unknown keys, so
        // confirm the resolved key actually matches before treating it as loaded.
        if self.fonts.get(key).map(|f| f.key == key).unwrap_or(false) {
            return;
        }
        let Some(path) = self.font_paths.get(key).cloned() else {
            return;
        };
        let Ok(bytes) = std::fs::read(&path) else {
            return;
        };
        if let Ok(font) = LoadedFont::from_bytes(key.to_string(), bytes) {
            self.fonts.insert(font);
        }
    }

    /// Ensure every font referenced by any object is loaded before a bake.
    fn ensure_object_fonts_loaded(&mut self) {
        let mut keys: Vec<String> = Vec::new();
        for obj in &self.objects {
            let Some(tb) = obj.text_block() else {
                continue;
            };
            let k = &tb.font_key;
            if !k.is_empty() && !keys.iter().any(|e| e == k) {
                keys.push(k.clone());
            }
        }
        for k in keys {
            self.ensure_font_loaded(&k);
        }
    }

    fn rebake(&mut self, ctx: &egui::Context, exclude: &[u64]) {
        if self.image.is_none() {
            return;
        }
        // Defensively make sure every object's font is parsed before baking
        // (covers sidecar loads / undo restoring an object whose font wasn't
        // touched in the picker this session). Done before borrowing `image`.
        self.ensure_object_fonts_loaded();
        self.ensure_stamp_images();
        let (iw, ih) = {
            let img = self.image.as_ref().unwrap();
            (img.width, img.height)
        };
        // Hide the GPU-quad stamps from the bake (they're drawn as GPU quads).
        // They're all enabled by construction, so restore them to enabled after.
        let mut restore: Vec<usize> = Vec::new();
        for &id in exclude {
            if let Some(idx) = self.objects.iter().position(|o| o.id == id) {
                if self.objects[idx].enabled {
                    self.objects[idx].enabled = false;
                    restore.push(idx);
                }
            }
        }
        let t_c = std::time::Instant::now();
        let overlay =
            bake_overlay_with_stamps(&self.objects, iw, ih, &self.fonts, &self.stamp_images);
        self.last_composite_ms = t_c.elapsed().as_secs_f64() * 1000.0;
        for idx in restore {
            self.objects[idx].enabled = true;
        }
        let t_u = std::time::Instant::now();
        let color = ColorImage::from_rgba_unmultiplied([overlay.w, overlay.h], &overlay.pixels);
        self.baked_texture = Some(ctx.load_texture("comic_baked", color, TextureOptions::LINEAR));
        self.last_upload_ms = t_u.elapsed().as_secs_f64() * 1000.0;
        self.baked_dirty = false;
        self.baked_excluded_set = exclude.to_vec();
    }

    /// Ids of stamps drawn as GPU quads (= excluded from the CPU bake), in
    /// ascending z (draw bottom-to-top). To stay z-correct, only the maximal
    /// TOP-z run of enabled, decodable stamps qualifies: those sit above
    /// everything left in the CPU bake, so drawing them on top preserves z.
    /// Both plain and outlined ("sticker") stamps qualify — the outlined ones use a
    /// pre-composited image+halo texture (`ensure_sticker_texture`) so the heavy
    /// halo dilation runs once per unique sticker, not every bake. A stamp placed
    /// BELOW a non-stamp object (or an undecodable stamp) ends the run and stays in
    /// the CPU bake at its true z. Ensures textures.
    fn gpu_stamp_ids(&mut self, ctx: &egui::Context) -> Vec<u64> {
        let mut order: Vec<usize> = (0..self.objects.len())
            .filter(|&i| self.objects[i].enabled)
            .collect();
        order.sort_by_key(|&i| self.objects[i].z);
        let mut ids = Vec::new();
        for &i in order.iter().rev() {
            let (id, stamp) = {
                let o = &self.objects[i];
                match &o.kind {
                    AnnotationKind::Stamp(s) => (o.id, s.clone()),
                    // Any non-stamp object ends the top run — stamps below it must
                    // stay in the CPU bake to keep correct z.
                    _ => break,
                }
            };
            let ok = if stamp.outline.is_some_and(|o| o.width_px > 0.0) {
                self.ensure_sticker_texture(ctx, &stamp).is_some()
            } else {
                self.ensure_stamp_texture(ctx, &stamp.source).is_some()
            };
            if ok {
                ids.push(id);
            } else {
                // Decode failed → this stamp (placeholder) and everything below it
                // stay in the CPU bake.
                break;
            }
        }
        ids.reverse(); // ascending z
        ids
    }

    /// Upload a stamp source as a GPU texture (once, cached by source key) for the
    /// live drag preview. Reuses the decode cache. `None` if the source can't be
    /// decoded (the bake's placeholder shows when the drag ends).
    fn ensure_stamp_texture(
        &mut self,
        ctx: &egui::Context,
        source: &StampSource,
    ) -> Option<TextureHandle> {
        let key = stamp_source_key(source);
        if let Some(t) = self.stamp_textures.get(&key) {
            return Some(t.clone());
        }
        let img = self
            .stamp_source_cache
            .entry(key.clone())
            .or_insert_with(|| load_stamp_image(source).map(Arc::new))
            .clone()?;
        let color = ColorImage::from_rgba_unmultiplied([img.w, img.h], &img.pixels);
        let tex = ctx.load_texture(format!("stamp_{key}"), color, TextureOptions::LINEAR);
        self.stamp_textures.insert(key, tex.clone());
        Some(tex)
    }

    /// Build (or fetch) the pre-composited image+halo GPU texture for an outlined
    /// ("sticker") stamp, so it draws as one quad like a plain stamp instead of
    /// re-rasterizing the halo in the CPU bake. Cached by source+outline+display
    /// size+flips: identical duplicates share one texture (the heavy dilation runs
    /// ONCE), so N copies of an outlined stamp no longer make the bake O(N).
    ///
    /// Returns `(texture, half_w, half_h)` — the on-canvas half-extents of the quad
    /// to draw, derived from the *capped* raster size so they match the CPU bake
    /// even for a corrupt/huge sidecar (the bake centers the capped tw×th bitmap).
    /// `None` if the source can't be decoded or the stamp has no positive-width
    /// outline.
    fn ensure_sticker_texture(
        &mut self,
        ctx: &egui::Context,
        stamp: &StampObject,
    ) -> Option<(TextureHandle, f32, f32)> {
        let outline = stamp.outline.filter(|o| o.width_px > 0.0)?;
        let tw = ((stamp.half_w * 2.0).round().max(1.0) as usize).min(STAMP_MAX_PX);
        let th = ((stamp.half_h * 2.0).round().max(1.0) as usize).min(STAMP_MAX_PX);
        // Effective dilation radius — must match comic-core's `composite_stamp_sticker`.
        let rad = (outline.width_px.round().max(0.0) as i32).min(256).max(0);
        // Quad half-extents from the capped bitmap (+ halo padding), so the quad
        // maps the texture 1:1 and matches the CPU bake's centered footprint.
        let hw = tw as f32 * 0.5 + rad as f32;
        let hh = th as f32 * 0.5 + rad as f32;
        let key = sticker_key(
            &stamp.source,
            outline.color,
            rad,
            tw,
            th,
            stamp.flip_h,
            stamp.flip_v,
        );
        if let Some(t) = self.sticker_textures.get(&key) {
            return Some((t.clone(), hw, hh));
        }
        // Reuse the shared decode cache (same key as the plain image path).
        let img = self
            .stamp_source_cache
            .entry(stamp_source_key(&stamp.source))
            .or_insert_with(|| load_stamp_image(&stamp.source).map(Arc::new))
            .clone()?;
        let (sticker, _) = comic_core::composite_stamp_sticker(
            &img,
            stamp.flip_h,
            stamp.flip_v,
            tw,
            th,
            Some(outline),
        );
        let color = ColorImage::from_rgba_unmultiplied([sticker.w, sticker.h], &sticker.pixels);
        let tex = ctx.load_texture(format!("sticker_{key}"), color, TextureOptions::LINEAR);
        // FIFO-evict the oldest entry to bound GPU memory — a long session of
        // resizing outlined stamps would otherwise keep a texture per distinct size.
        // Capped well above any single frame's working set (the top-z run can't
        // exceed the object count) so eviction never drops an in-use texture.
        const STICKER_CACHE_CAP: usize = 256;
        while self.sticker_textures.len() >= STICKER_CACHE_CAP {
            match self.sticker_order.pop_front() {
                Some(old) => {
                    self.sticker_textures.remove(&old);
                }
                None => break,
            }
        }
        self.sticker_textures.insert(key.clone(), tex.clone());
        self.sticker_order.push_back(key);
        Some((tex, hw, hh))
    }

    /// Draw stamp `id` as a GPU-transformed textured quad (scale/rotate/flip/
    /// opacity), centered on its pivot. Used for ALL GPU stamps (excluded from the
    /// CPU bake), so a stamp never re-rasterizes on the CPU and its position is
    /// identical whether idle or being dragged. Outlined stamps draw their
    /// pre-composited image+halo "sticker" texture (flips baked in, quad grown by
    /// the halo `rad`); plain stamps draw the raw image texture with UV-swap flips.
    fn draw_stamp_preview(&mut self, ctx: &egui::Context, painter: &egui::Painter, id: u64) {
        let (pivot, rot, opacity, stamp) = {
            let Some(obj) = self.objects.iter().find(|o| o.id == id) else {
                return;
            };
            let AnnotationKind::Stamp(s) = &obj.kind else {
                return;
            };
            (
                obj.pivot,
                obj.rotation_rad,
                s.opacity.clamp(0.0, 1.0),
                s.clone(),
            )
        };
        let (tex, hw, hh, (u0, u1), (v0, v1)) = if stamp.outline.is_some_and(|o| o.width_px > 0.0) {
            // Sticker texture already contains the halo + baked flips → straight UVs.
            // `ensure_sticker_texture` returns the quad half-extents (from the capped
            // raster + halo padding) so the quad maps the texture 1:1.
            let Some((tex, hw, hh)) = self.ensure_sticker_texture(ctx, &stamp) else {
                return;
            };
            (tex, hw, hh, (0.0f32, 1.0f32), (0.0f32, 1.0f32))
        } else {
            let Some(tex) = self.ensure_stamp_texture(ctx, &stamp.source) else {
                return;
            };
            let u = if stamp.flip_h {
                (1.0f32, 0.0f32)
            } else {
                (0.0f32, 1.0f32)
            };
            let v = if stamp.flip_v {
                (1.0f32, 0.0f32)
            } else {
                (0.0f32, 1.0f32)
            };
            (tex, stamp.half_w, stamp.half_h, u, v)
        };
        let corner = |lx: f32, ly: f32| {
            self.view
                .img_to_screen(rotate_about((pivot.0 + lx, pivot.1 + ly), pivot, rot))
        };
        let (tl, tr, br, bl) = (
            corner(-hw, -hh),
            corner(hw, -hh),
            corner(hw, hh),
            corner(-hw, hh),
        );
        let tint = Color32::from_white_alpha((opacity * 255.0).round().clamp(0.0, 255.0) as u8);
        let mut mesh = egui::Mesh::with_texture(tex.id());
        mesh.vertices.push(egui::epaint::Vertex {
            pos: tl,
            uv: egui::pos2(u0, v0),
            color: tint,
        });
        mesh.vertices.push(egui::epaint::Vertex {
            pos: tr,
            uv: egui::pos2(u1, v0),
            color: tint,
        });
        mesh.vertices.push(egui::epaint::Vertex {
            pos: br,
            uv: egui::pos2(u1, v1),
            color: tint,
        });
        mesh.vertices.push(egui::epaint::Vertex {
            pos: bl,
            uv: egui::pos2(u0, v1),
            color: tint,
        });
        mesh.indices.extend_from_slice(&[0, 1, 2, 0, 2, 3]);
        painter.add(egui::Shape::mesh(mesh));
    }

    /// Resolve every Stamp object's source into a decoded RGBA image for baking.
    /// Decodes are cached by source key so the same emoji/file decodes once;
    /// `stamp_images` (keyed by object id) is rebuilt to match the live objects.
    fn ensure_stamp_images(&mut self) {
        self.stamp_images.clear();
        // Collect (object id, source) for stamp objects to avoid borrow overlap.
        let wanted: Vec<(u64, StampSource)> = self
            .objects
            .iter()
            .filter_map(|o| match &o.kind {
                AnnotationKind::Stamp(s) => Some((o.id, s.source.clone())),
                _ => None,
            })
            .collect();
        for (id, source) in wanted {
            let key = stamp_source_key(&source);
            let entry = self
                .stamp_source_cache
                .entry(key)
                .or_insert_with(|| load_stamp_image(&source).map(Arc::new));
            if let Some(img) = entry {
                // Cheap Arc clone (shares the cached decode; no pixel copy).
                self.stamp_images.insert(id, img.clone());
            }
        }
    }

    /// Load a font file at runtime and assign it to `target`.
    fn add_font_file(&mut self, path: &Path, target: u64) {
        let Ok(bytes) = std::fs::read(path) else {
            self.status = format!("フォント読み込み失敗: {}", path.display());
            return;
        };
        let label = path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("font")
            .to_string();
        match LoadedFont::from_bytes(label.clone(), bytes) {
            Ok(font) => {
                // Classify so a Latin-only added font is filtered correctly in
                // the JP-only right-panel list (covers is cheap on a loaded face).
                let script = if font.covers('あ') && font.covers('日') {
                    FontScript::Japanese
                } else if font.covers('A') {
                    FontScript::Latin
                } else {
                    FontScript::Other
                };
                self.font_script.insert(label.clone(), script);
                self.fonts.insert(font);
                self.font_paths
                    .entry(label.clone())
                    .or_insert_with(|| path.to_path_buf());
                if !self.available_fonts.iter().any(|(k, _)| k == &label) {
                    self.available_fonts.push((label.clone(), label.clone()));
                    self.available_fonts
                        .sort_by(|a, b| a.1.to_lowercase().cmp(&b.1.to_lowercase()));
                }
                if let Some(tb) = self
                    .objects
                    .iter_mut()
                    .find(|o| o.id == target)
                    .and_then(|o| o.text_block_mut())
                {
                    tb.font_key = label.clone();
                    // Choosing a font is an individual edit → break the link.
                    tb.preset_link = None;
                }
                self.font_sample_cache.clear();
                self.onomatopoeia_thumb_cache.clear();
                self.baked_dirty = true;
                self.status = format!("フォント追加: {label}");
            }
            Err(e) => self.status = format!("フォント解析失敗: {e}"),
        }
    }

    // ----- Font sample dialog -------------------------------------------

    /// Open the font-sample picker for object `sel`. The sample defaults to the
    /// object's current text (so you preview your own words), falling back to a
    /// mixed JP/Latin/number string. Clears the texture cache (sample changed).
    fn open_font_dialog(&mut self, sel: u64) {
        let sample = self
            .objects
            .iter()
            .find(|o| o.id == sel)
            .and_then(|o| o.text_block().map(|tb| tb.text.clone()))
            .unwrap_or_default();
        // First line only, capped to a sane length (a very long sample would
        // make a huge texture per font card).
        let sample: String = sample
            .lines()
            .next()
            .unwrap_or("")
            .trim()
            .chars()
            .take(24)
            .collect();
        self.font_dialog_sample = if sample.is_empty() {
            "あア亜Ag 12!?".to_string()
        } else {
            sample
        };
        self.font_dialog_target = Some(sel);
        self.font_dialog_filter.clear();
        self.font_sample_cache.clear();
        self.show_font_dialog = true;
    }

    /// Render a one-line sample of `name` (must be loaded) as an RGBA image:
    /// black text on transparent. Returns None if the font isn't resolvable.
    fn render_font_sample(&self, name: &str, sample: &str, px: f32) -> Option<ColorImage> {
        let font = self.fonts.get(name)?;
        // `FontSet::get` silently falls back to the default font for unknown
        // keys; skip if the resolved face isn't actually `name` (don't show the
        // default font under another name's label).
        if font.key != name {
            return None;
        }
        // Hard cap the sample length so one card can never allocate a huge
        // texture (the dialog field is user-editable).
        let sample: String = sample.chars().take(40).collect();
        let block = TextBlock {
            text: sample,
            font_key: name.to_string(),
            size_px: px,
            color: Rgba::BLACK,
            orientation: Orientation::Horizontal,
            ..TextBlock::default()
        };
        let layout = layout_text(&block, font);
        let pad = 6.0f32;
        // Clamp the canvas so a pathological glyph/advance can't blow up memory.
        let w = ((layout.bounds.0 + pad * 2.0).ceil() as usize).clamp(1, 2000);
        let h = ((layout.bounds.1 + pad * 2.0).ceil() as usize).clamp(1, 400);
        let obj = AnnotationObject::new_text(0, (pad, pad), block);
        let overlay = bake_overlay(&[obj], w, h, &self.fonts);
        Some(ColorImage::from_rgba_unmultiplied(
            [overlay.w, overlay.h],
            &overlay.pixels,
        ))
    }

    /// Render an onomatopoeia preset thumbnail through comic-core using the
    /// resolved font face. Rotation is intentionally omitted in the picker so
    /// the glyph shape stays easy to compare; inserted objects still receive
    /// the preset's rotation.
    fn render_onomatopoeia_preview(
        &self,
        preset: OnomatopoeiaPreset,
        font_key: &str,
    ) -> Option<ColorImage> {
        let font = self.fonts.get(font_key)?;
        if font.key != font_key {
            return None;
        }
        let mut block = preset.build_text(font_key);
        block.size_px = block.size_px.clamp(50.0, 92.0);
        let layout = layout_text(&block, font);
        let outline_pad = block.outline.map(|s| s.width_px).unwrap_or(0.0);
        let pad = (outline_pad + 12.0).ceil();
        let w = ((layout.bounds.0 + pad * 2.0).ceil() as usize).clamp(1, 1600);
        let h = ((layout.bounds.1 + pad * 2.0).ceil() as usize).clamp(1, 1200);
        let obj = AnnotationObject::new_text(0, (pad, pad), block);
        let overlay = bake_overlay(&[obj], w, h, &self.fonts);
        Some(ColorImage::from_rgba_unmultiplied(
            [overlay.w, overlay.h],
            &overlay.pixels,
        ))
    }

    /// Return the cached sample texture for `name` without building it.
    fn font_sample_cached(&self, name: &str) -> Option<TextureHandle> {
        self.font_sample_cache
            .get(&(name.to_string(), self.font_dialog_sample.clone()))
            .cloned()
    }

    /// Lazily build + cache the sample texture for `name` (loads the font if
    /// needed). Returns a clone of the cached handle, or None if unavailable.
    fn font_sample_texture(&mut self, ctx: &egui::Context, name: &str) -> Option<TextureHandle> {
        let key = (name.to_string(), self.font_dialog_sample.clone());
        if let Some(tex) = self.font_sample_cache.get(&key) {
            return Some(tex.clone());
        }
        self.ensure_font_loaded(name);
        let sample = self.font_dialog_sample.clone();
        let img = self.render_font_sample(name, &sample, 30.0)?;
        let tex = ctx.load_texture(format!("font_sample_{name}"), img, TextureOptions::LINEAR);
        self.font_sample_cache.insert(key, tex.clone());
        Some(tex)
    }

    /// Lazily build + cache the actual-font thumbnail for an onomatopoeia
    /// preset. This mirrors insertion font resolution, including fallback.
    fn onomatopoeia_thumb_texture(
        &mut self,
        ctx: &egui::Context,
        preset: OnomatopoeiaPreset,
    ) -> Option<TextureHandle> {
        let font_key = self.resolve_onomatopoeia_font(preset.font_candidate());
        let key = format!("{}|{}|{}", preset.label(), preset.text(), font_key);
        if let Some(tex) = self.onomatopoeia_thumb_cache.get(&key) {
            return Some(tex.clone());
        }
        self.ensure_font_loaded(&font_key);
        let img = self.render_onomatopoeia_preview(preset, &font_key)?;
        let tex_name = format!(
            "onomato_preview_{}_{}",
            font_lookup_key(preset.label()),
            font_lookup_key(&font_key)
        );
        let tex = ctx.load_texture(tex_name, img, TextureOptions::LINEAR);
        self.onomatopoeia_thumb_cache.insert(key, tex.clone());
        Some(tex)
    }

    /// The font-sample picker modal: a scrollable grid of font cards, each
    /// showing the font name + a rendered sample of the current text. Only the
    /// visible rows are rasterized (lazy + cached). Clicking a card assigns the
    /// font to the target object.
    fn draw_font_dialog(&mut self, ctx: &egui::Context) {
        if !self.show_font_dialog {
            return;
        }
        let Some(target) = self.font_dialog_target else {
            self.show_font_dialog = false;
            return;
        };
        let mut open = true;
        let mut chosen: Option<String> = None;
        let mut open_file = false;
        // Snapshot the font list + current selection up front (avoids borrowing
        // self.* through the closure while we also call &mut self methods).
        let fonts_list = self.available_fonts.clone();
        let current_key = self
            .objects
            .iter()
            .find(|o| o.id == target)
            .and_then(|o| o.text_block().map(|tb| tb.font_key.clone()))
            .unwrap_or_default();

        egui::Window::new("フォントを見本から選択")
            .open(&mut open)
            .collapsible(false)
            .resizable(true)
            .default_size([720.0, 540.0])
            .default_pos(ctx.content_rect().min + egui::vec2(60.0, 40.0))
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    ui.label("見本テキスト:");
                    if ui
                        .add(
                            egui::TextEdit::singleline(&mut self.font_dialog_sample)
                                .desired_width(220.0),
                        )
                        .changed()
                    {
                        // Sample changed → previously rendered textures are stale.
                        self.font_sample_cache.clear();
                    }
                    ui.label("絞り込み:");
                    ui.add(
                        egui::TextEdit::singleline(&mut self.font_dialog_filter)
                            .desired_width(180.0),
                    );
                    // File picker (low-frequency) lives here, top-right.
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui.button("フォントファイルを開く").clicked() {
                            open_file = true;
                        }
                    });
                });
                ui.horizontal(|ui| {
                    ui.label("種別:");
                    for (label, cat) in [
                        ("日本語", FontCategory::Japanese),
                        ("英語", FontCategory::Latin),
                        ("すべて", FontCategory::All),
                    ] {
                        ui.radio_value(&mut self.font_dialog_category, cat, label);
                    }
                });
                ui.separator();

                let filter_lc = self.font_dialog_filter.to_lowercase();
                let cat = self.font_dialog_category;
                let visible: Vec<(String, String)> = fonts_list
                    .iter()
                    .filter(|(key, label)| {
                        let name_ok =
                            filter_lc.is_empty() || label.to_lowercase().contains(&filter_lc);
                        // Unknown (not yet classified) shown optimistically in the
                        // JP / Latin categories so the grid isn't empty during the
                        // background classification.
                        let cat_ok = match cat {
                            FontCategory::All => true,
                            FontCategory::Japanese => matches!(
                                self.font_script.get(key.as_str()),
                                None | Some(FontScript::Japanese)
                            ),
                            FontCategory::Latin => matches!(
                                self.font_script.get(key.as_str()),
                                None | Some(FontScript::Latin)
                            ),
                        };
                        name_ok && cat_ok
                    })
                    .cloned()
                    .collect();

                let avail_w = ui.available_width();
                let cols = ((avail_w / (FONT_CARD_W + 8.0)).floor() as usize).max(1);
                let rows = visible.len().div_ceil(cols);

                // Cap new sample renders per frame so opening / scrolling the
                // dialog (which parses fonts + uploads textures on this thread)
                // never freezes; remaining cards fill in over the next frames.
                let mut build_budget = 6i32;
                let mut need_repaint = false;
                let sample = self.font_dialog_sample.clone();

                egui::ScrollArea::vertical()
                    .auto_shrink([false, false])
                    .show_rows(ui, FONT_CARD_H + 8.0, rows, |ui, row_range| {
                        for row in row_range {
                            ui.horizontal(|ui| {
                                for col in 0..cols {
                                    let idx = row * cols + col;
                                    let Some((key, label)) = visible.get(idx) else {
                                        break;
                                    };
                                    let selected = key == &current_key;
                                    let cached = self
                                        .font_sample_cache
                                        .contains_key(&(key.clone(), sample.clone()));
                                    let allow = cached || build_budget > 0;
                                    if !cached && allow {
                                        build_budget -= 1;
                                    }
                                    if !cached && !allow {
                                        need_repaint = true;
                                    }
                                    if self.draw_font_card(ui, ctx, key, label, selected, allow) {
                                        chosen = Some(key.clone());
                                    }
                                }
                            });
                        }
                        if need_repaint {
                            ui.ctx().request_repaint();
                        }
                    });
            });

        if open_file {
            if let Some(path) = rfd::FileDialog::new()
                .add_filter("フォント", &["ttf", "otf", "ttc"])
                .pick_file()
            {
                // Loads + assigns the font to `target` and breaks its preset link.
                self.add_font_file(&path, target);
                self.font_sample_cache.clear();
            }
        }
        if let Some(key) = chosen {
            if let Some(tb) = self
                .objects
                .iter_mut()
                .find(|o| o.id == target)
                .and_then(|o| o.text_block_mut())
            {
                tb.font_key = key.clone();
                // Choosing a font is an individual edit → break the preset link.
                tb.preset_link = None;
            }
            self.ensure_font_loaded(&key);
            self.baked_dirty = true;
            self.show_font_dialog = false;
        }
        if !open {
            self.show_font_dialog = false;
        }
    }

    /// One font card (fixed box): the rendered sample on top, the font name
    /// below. Returns true when clicked. Builds the sample texture only when
    /// `allow_build` (per-frame budget); otherwise uses the cache if present.
    fn draw_font_card(
        &mut self,
        ui: &mut egui::Ui,
        ctx: &egui::Context,
        key: &str,
        label: &str,
        selected: bool,
        allow_build: bool,
    ) -> bool {
        let (rect, resp) =
            ui.allocate_exact_size(egui::vec2(FONT_CARD_W, FONT_CARD_H), Sense::click());
        let hovered = resp.hovered();
        let painter = ui.painter_at(rect);
        let bg = if selected {
            Color32::from_rgb(50, 70, 100)
        } else if hovered {
            Color32::from_rgb(58, 58, 64)
        } else {
            Color32::from_rgb(40, 40, 44)
        };
        painter.rect_filled(rect, 4.0, bg);
        painter.rect_stroke(
            rect,
            4.0,
            egui::Stroke::new(
                if selected { 2.0 } else { 1.0 },
                if selected || hovered {
                    Color32::from_rgb(150, 195, 255)
                } else {
                    Color32::from_gray(70)
                },
            ),
            egui::StrokeKind::Inside,
        );
        // Layout: light preview strip on top, a dedicated name band below it so the
        // (white-on-dark) font name never overlaps the light sample or clips at the
        // card's bottom edge.
        let pad = 6.0;
        let name_band = 22.0;
        let sample_h = (FONT_CARD_H - pad * 2.0 - name_band).max(8.0);
        // Sample image (lazy). Drawn on a light strip so black text reads.
        let sample_area = Rect::from_min_size(
            rect.min + egui::vec2(pad, pad),
            egui::vec2(FONT_CARD_W - pad * 2.0, sample_h),
        );
        painter.rect_filled(sample_area, 2.0, Color32::from_gray(235));
        let tex = if allow_build {
            self.font_sample_texture(ctx, key)
        } else {
            self.font_sample_cached(key)
        };
        if let Some(tex) = tex {
            let sz = tex.size_vec2();
            let scale = (sample_area.width() / sz.x)
                .min(sample_area.height() / sz.y)
                .min(1.0);
            let draw = egui::vec2(sz.x * scale, sz.y * scale);
            let origin = sample_area.center() - draw * 0.5;
            painter.image(
                tex.id(),
                Rect::from_min_size(origin, draw),
                Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
                Color32::WHITE,
            );
        }
        // Font name centered in the name band (below the preview strip), so it is
        // fully visible and clear of the light sample.
        painter.text(
            egui::pos2(rect.min.x + 8.0, rect.max.y - pad - name_band * 0.5),
            egui::Align2::LEFT_CENTER,
            label,
            egui::FontId::proportional(12.0),
            Color32::WHITE,
        );
        resp.clicked()
    }

    // ----- Presets -------------------------------------------------------

    /// Persist the current USER presets (system entries are excluded — they are
    /// rebuilt at startup). Called whenever user presets change.
    fn save_user_presets(&mut self) {
        let doc = UserPresetDoc {
            text: self
                .text_presets
                .iter()
                .filter(|p| !is_system_preset(&p.id))
                .cloned()
                .collect(),
            shape: self
                .shape_presets
                .iter()
                .filter(|p| !is_system_preset(&p.id))
                .cloned()
                .collect(),
            window: self
                .window_presets
                .iter()
                .filter(|p| !is_system_preset(&p.id))
                .cloned()
                .collect(),
        };
        let path = presets_path();
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        match serde_json::to_string_pretty(&doc) {
            Ok(json) => {
                if let Err(e) = std::fs::write(&path, json) {
                    self.status = format!("プリセット保存失敗: {e}");
                }
            }
            Err(e) => self.status = format!("プリセット直列化失敗: {e}"),
        }
    }

    /// Apply text-style preset `idx` to the selected object (links + glows it).
    fn apply_text_preset_by_index(&mut self, idx: usize) {
        let Some(preset) = self.text_presets.get(idx).cloned() else {
            return;
        };
        self.ensure_font_loaded(&preset.font_key);
        // Pre-fill the name field with the applied preset (so 登録=update /
        // 削除 act on it, and renaming + 登録 creates a new one).
        self.text_preset_name_input = preset.name.clone();
        let id = self.selected;
        let mut is_window = false;
        if let Some(o) = self.selected_obj_mut() {
            if let Some(tb) = o.text_block_mut() {
                preset.apply_to(tb);
            }
            // A window's body text style is also part of its window-style preset,
            // so changing it unlinks the window preset (and may resize an
            // AutoFitText window → re-anchor below).
            if let AnnotationKind::MessageWindow(w) = &mut o.kind {
                w.style_preset_link = None;
                is_window = true;
            }
            self.baked_dirty = true;
        }
        if is_window {
            if let Some(id) = id {
                self.apply_window_placement(id);
            }
        }
    }

    /// Apply shape-style preset `idx` to the selected bubble (links + glows it).
    fn apply_shape_preset_by_index(&mut self, idx: usize) {
        let Some(preset) = self.shape_presets.get(idx).cloned() else {
            return;
        };
        self.shape_preset_name_input = preset.name.clone();
        let pivot = self
            .selected_obj_mut()
            .map(|o| o.pivot)
            .unwrap_or((0.0, 0.0));
        if let Some(o) = self.selected_obj_mut() {
            if let AnnotationKind::Bubble(b) = &mut o.kind {
                preset.apply_to(b, pivot);
                self.baked_dirty = true;
            }
        }
    }

    /// 登録 for a text preset: overwrite an existing same-named USER preset (and
    /// bulk-reapply to every linked object), or create a new user preset and
    /// link the current selection to it. System names never overwrite.
    fn register_text_preset(&mut self, name: &str) {
        let name = name.trim();
        if name.is_empty() {
            self.status = "プリセット名を入力してください".to_string();
            return;
        }
        let Some(tb) = self
            .selected_obj_mut()
            .and_then(|o| o.text_block().cloned())
        else {
            return;
        };
        // Overwrite path: a USER preset with this exact name already exists.
        if let Some(idx) = self
            .text_presets
            .iter()
            .position(|p| !is_system_preset(&p.id) && p.name == name)
        {
            let id = self.text_presets[idx].id.clone();
            let updated = TextStylePreset::from_text(id.clone(), name.to_string(), &tb);
            self.text_presets[idx] = updated.clone();
            // Bulk-reapply the new values to every object linked to this preset.
            // A linked message window's body style is also part of its window
            // preset → unlink it and re-anchor (its fitted size may change).
            let mut reanchor: Vec<u64> = Vec::new();
            for obj in self.objects.iter_mut() {
                let Some(tb) = obj.text_block_mut() else {
                    continue;
                };
                if tb.preset_link.as_deref() != Some(id.as_str()) {
                    continue;
                }
                updated.apply_to(tb);
                if let AnnotationKind::MessageWindow(w) = &mut obj.kind {
                    w.style_preset_link = None;
                    reanchor.push(obj.id);
                }
            }
            for oid in reanchor {
                self.apply_window_placement(oid);
            }
            self.save_user_presets();
            self.baked_dirty = true;
            self.status = format!("セリフプリセット「{name}」を上書きしました");
            return;
        }
        // New preset (also handles a name that collides with a system preset:
        // we just create a distinct user:* id, never touching the system one).
        let id = self.unique_user_id(name, true);
        let preset = TextStylePreset::from_text(id, name.to_string(), &tb);
        // Link the current selection to the freshly-created preset.
        if let Some(tb) = self.selected_obj_mut().and_then(|o| o.text_block_mut()) {
            tb.preset_link = Some(preset.id.clone());
        }
        self.text_presets.push(preset);
        self.save_user_presets();
        self.baked_dirty = true;
        self.status = format!("セリフプリセット「{name}」を登録しました");
    }

    /// 登録 for a shape preset (same overwrite + bulk + new logic).
    fn register_shape_preset(&mut self, name: &str) {
        let name = name.trim();
        if name.is_empty() {
            self.status = "プリセット名を入力してください".to_string();
            return;
        }
        let Some(b) = self.selected_obj_mut().and_then(|o| match &o.kind {
            AnnotationKind::Bubble(b) => Some(b.clone()),
            _ => None,
        }) else {
            return;
        };
        if let Some(idx) = self
            .shape_presets
            .iter()
            .position(|p| !is_system_preset(&p.id) && p.name == name)
        {
            let id = self.shape_presets[idx].id.clone();
            let updated = ShapeStylePreset::from_bubble(id.clone(), name.to_string(), &b);
            self.shape_presets[idx] = updated.clone();
            // Bulk-reapply to every linked bubble (each keeps its own pivot/tail).
            let ids: Vec<u64> = self
                .objects
                .iter()
                .filter(|o| match &o.kind {
                    AnnotationKind::Bubble(bb) => {
                        bb.shape_preset_link.as_deref() == Some(id.as_str())
                    }
                    _ => false,
                })
                .map(|o| o.id)
                .collect();
            for oid in ids {
                let pivot = self
                    .objects
                    .iter()
                    .find(|o| o.id == oid)
                    .map(|o| o.pivot)
                    .unwrap_or((0.0, 0.0));
                if let Some(o) = self.objects.iter_mut().find(|o| o.id == oid) {
                    if let AnnotationKind::Bubble(bb) = &mut o.kind {
                        updated.apply_to(bb, pivot);
                    }
                }
            }
            self.save_user_presets();
            self.baked_dirty = true;
            self.status = format!("本体プリセット「{name}」を上書きしました");
            return;
        }
        let id = self.unique_user_id(name, false);
        let preset = ShapeStylePreset::from_bubble(id, name.to_string(), &b);
        if let Some(o) = self.selected_obj_mut() {
            if let AnnotationKind::Bubble(bb) = &mut o.kind {
                bb.shape_preset_link = Some(preset.id.clone());
            }
        }
        self.shape_presets.push(preset);
        self.save_user_presets();
        self.baked_dirty = true;
        self.status = format!("本体プリセット「{name}」を登録しました");
    }

    /// Build a unique `user:<name>` id that doesn't collide with an existing
    /// preset id in the relevant list (`is_text` selects which list to scan).
    fn unique_user_id(&self, name: &str, _is_text: bool) -> String {
        let base = format!("user:{name}");
        // Scan all three preset lists so a user id is unique across kinds.
        let exists = |id: &str| -> bool {
            self.text_presets.iter().any(|p| p.id == id)
                || self.shape_presets.iter().any(|p| p.id == id)
                || self.window_presets.iter().any(|p| p.id == id)
        };
        if !exists(&base) {
            return base;
        }
        let mut n = 2;
        loop {
            let cand = format!("{base}#{n}");
            if !exists(&cand) {
                return cand;
            }
            n += 1;
        }
    }

    /// Delete a USER text preset by index: unlink any objects pointing to it,
    /// remove it, and re-save. System presets are never deletable.
    fn delete_text_preset(&mut self, idx: usize) {
        let Some(p) = self.text_presets.get(idx) else {
            return;
        };
        if is_system_preset(&p.id) {
            return;
        }
        let id = p.id.clone();
        for obj in self.objects.iter_mut() {
            let Some(t) = obj.text_block_mut() else {
                continue;
            };
            if t.preset_link.as_deref() == Some(id.as_str()) {
                t.preset_link = None;
            }
        }
        self.text_presets.remove(idx);
        self.save_user_presets();
        self.baked_dirty = true;
    }

    /// Delete a USER shape preset by index (unlink + remove + re-save).
    fn delete_shape_preset(&mut self, idx: usize) {
        let Some(p) = self.shape_presets.get(idx) else {
            return;
        };
        if is_system_preset(&p.id) {
            return;
        }
        let id = p.id.clone();
        for obj in self.objects.iter_mut() {
            if let AnnotationKind::Bubble(b) = &mut obj.kind {
                if b.shape_preset_link.as_deref() == Some(id.as_str()) {
                    b.shape_preset_link = None;
                }
            }
        }
        self.shape_presets.remove(idx);
        self.save_user_presets();
        self.baked_dirty = true;
    }

    /// Apply window-style preset `idx` to the selected window (links + glows it),
    /// then re-resolve its placement (position/size may have changed).
    fn apply_window_preset_by_index(&mut self, idx: usize) {
        let Some(preset) = self.window_presets.get(idx).cloned() else {
            return;
        };
        self.window_preset_name_input = preset.name.clone();
        let id = self.selected;
        if let Some(o) = self.selected_obj_mut() {
            if let AnnotationKind::MessageWindow(w) = &mut o.kind {
                preset.apply_to(w);
            }
        }
        if let Some(id) = id {
            self.apply_window_placement(id);
        }
        self.baked_dirty = true;
    }

    /// 登録 for a window preset (overwrite same-named user preset + bulk-reapply,
    /// or create a new user preset and link the selection).
    fn register_window_preset(&mut self, name: &str) {
        let name = name.trim();
        if name.is_empty() {
            self.status = "プリセット名を入力してください".to_string();
            return;
        }
        let Some(w) = self.selected_obj_mut().and_then(|o| match &o.kind {
            AnnotationKind::MessageWindow(w) => Some(w.clone()),
            _ => None,
        }) else {
            return;
        };
        if let Some(idx) = self
            .window_presets
            .iter()
            .position(|p| !is_system_preset(&p.id) && p.name == name)
        {
            let id = self.window_presets[idx].id.clone();
            let updated = WindowStylePreset::from_window(id.clone(), name.to_string(), &w);
            self.window_presets[idx] = updated.clone();
            let ids: Vec<u64> = self
                .objects
                .iter()
                .filter(|o| match &o.kind {
                    AnnotationKind::MessageWindow(ww) => {
                        ww.style_preset_link.as_deref() == Some(id.as_str())
                    }
                    _ => false,
                })
                .map(|o| o.id)
                .collect();
            for oid in &ids {
                if let Some(o) = self.objects.iter_mut().find(|o| o.id == *oid) {
                    if let AnnotationKind::MessageWindow(ww) = &mut o.kind {
                        updated.apply_to(ww);
                    }
                }
                self.apply_window_placement(*oid);
            }
            self.save_user_presets();
            self.baked_dirty = true;
            self.status = format!("ウィンドウプリセット「{name}」を上書きしました");
            return;
        }
        let id = self.unique_user_id(name, false);
        let preset = WindowStylePreset::from_window(id, name.to_string(), &w);
        if let Some(o) = self.selected_obj_mut() {
            if let AnnotationKind::MessageWindow(ww) = &mut o.kind {
                ww.style_preset_link = Some(preset.id.clone());
            }
        }
        self.window_presets.push(preset);
        self.save_user_presets();
        self.baked_dirty = true;
        self.status = format!("ウィンドウプリセット「{name}」を登録しました");
    }

    /// Delete a USER window preset by index (unlink + remove + re-save).
    fn delete_window_preset(&mut self, idx: usize) {
        let Some(p) = self.window_presets.get(idx) else {
            return;
        };
        if is_system_preset(&p.id) {
            return;
        }
        let id = p.id.clone();
        for obj in self.objects.iter_mut() {
            if let AnnotationKind::MessageWindow(w) = &mut obj.kind {
                if w.style_preset_link.as_deref() == Some(id.as_str()) {
                    w.style_preset_link = None;
                }
            }
        }
        self.window_presets.remove(idx);
        self.save_user_presets();
        self.baked_dirty = true;
    }
}

fn rgba_to_color32(c: Rgba) -> Color32 {
    Color32::from_rgba_unmultiplied(c.r, c.g, c.b, c.a)
}

fn color32_to_rgba(c: Color32) -> Rgba {
    Rgba::new(c.r(), c.g(), c.b(), c.a())
}

impl ComicLab {
    /// Drop the Enter event that confirms a Japanese IME conversion, so it does
    /// not leak into the multiline TextEdit as a stray newline. Normal Enter
    /// (not during composition) is left intact so it still inserts a newline.
    /// Mirrors the main app's `update_ime_state` + `events.retain` idiom.
    fn consume_ime_enter(&mut self, ctx: &egui::Context) {
        ctx.input_mut(|i| {
            // Was an IME composition active when this frame's events arrived?
            let mut active = self.ime_composing;
            for e in &i.events {
                if let egui::Event::Ime(ime) = e {
                    match ime {
                        egui::ImeEvent::Enabled => active = true,
                        egui::ImeEvent::Preedit(s) => active = !s.is_empty(),
                        egui::ImeEvent::Commit(_) => active = true,
                        egui::ImeEvent::Disabled => {}
                    }
                }
            }
            // Update the persistent composing flag from the latest IME event.
            for e in &i.events {
                if let egui::Event::Ime(ime) = e {
                    self.ime_composing = match ime {
                        egui::ImeEvent::Enabled => true,
                        egui::ImeEvent::Preedit(s) => !s.is_empty(),
                        egui::ImeEvent::Commit(_) | egui::ImeEvent::Disabled => false,
                    };
                }
            }
            if active {
                i.events.retain(|e| {
                    !matches!(
                        e,
                        egui::Event::Key {
                            key: egui::Key::Enter,
                            pressed: true,
                            ..
                        }
                    )
                });
            }
        });
    }

    /// Drain background font classifications into `font_script`. Requests a
    /// repaint while results are still streaming so the filtered lists refine.
    fn drain_font_scripts(&mut self, ctx: &egui::Context) {
        let mut disconnected = false;
        if let Some(rx) = &self.font_script_rx {
            loop {
                match rx.try_recv() {
                    Ok((key, script)) => {
                        self.font_script.insert(key, script);
                    }
                    Err(mpsc::TryRecvError::Empty) => break,
                    Err(mpsc::TryRecvError::Disconnected) => {
                        disconnected = true;
                        break;
                    }
                }
            }
            ctx.request_repaint();
        }
        if disconnected {
            self.font_script_rx = None;
        }
    }
}

impl eframe::App for ComicLab {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.consume_ime_enter(ctx);
        self.drain_font_scripts(ctx);
        self.handle_dropped_files(ctx);
        self.draw_menu_bar(ctx);
        self.draw_left_panel(ctx);
        self.draw_right_panel(ctx);

        // Object-level undo/redo BEFORE the canvas bakes, so a Ctrl+Z/Y frame
        // shows the restored state immediately (not one frame late). Skipped
        // while a text field is focused so the TextEdit keeps Ctrl+Z / Delete.
        if !ctx.wants_keyboard_input() {
            let undo = ctx.input(|i| {
                i.modifiers.command && i.key_pressed(egui::Key::Z) && !i.modifiers.shift
            });
            let redo = ctx.input(|i| {
                i.modifiers.command
                    && (i.key_pressed(egui::Key::Y)
                        || (i.key_pressed(egui::Key::Z) && i.modifiers.shift))
            });
            if undo {
                self.do_undo();
            } else if redo {
                self.do_redo();
            }
        }

        self.draw_canvas(ctx);
        self.draw_add_dialog(ctx);
        self.draw_add_window_dialog(ctx);
        self.draw_onomatopoeia_dialog(ctx);
        self.draw_font_dialog(ctx);
        self.draw_stamp_dialog(ctx);

        // Commit a coalesced undo snapshot once the interaction settles.
        let busy = ctx.input(|i| i.pointer.any_down()) || ctx.wants_keyboard_input();
        if !busy {
            self.commit_pending();
        }
    }
}

impl ComicLab {
    /// Left panel: add tools + object cards + action buttons (補正レイヤー風)。
    fn draw_left_panel(&mut self, ctx: &egui::Context) {
        egui::SidePanel::left("comic_left")
            .resizable(false)
            .exact_width(LEFT_W)
            .show(ctx, |ui| {
                ui.add_space(6.0);
                ui.heading("吹き出し / テキスト");
                ui.separator();

                // One full-width add button per row. Generated onomatopoeia is a
                // text object; image onomatopoeia stays under stamps.
                let add_w = LEFT_W - 20.0;
                if ui
                    .add_sized(egui::vec2(add_w, 26.0), egui::Button::new("吹き出し追加"))
                    .clicked()
                {
                    self.show_add_dialog = true;
                }
                ui.add_space(2.0);
                if ui
                    .add_sized(egui::vec2(add_w, 26.0), egui::Button::new("ウィンドウ追加"))
                    .clicked()
                {
                    self.show_add_window_dialog = true;
                }
                ui.add_space(2.0);
                if ui
                    .add_sized(egui::vec2(add_w, 26.0), egui::Button::new("テキスト追加"))
                    .clicked()
                {
                    self.add_text();
                }
                ui.add_space(2.0);
                if ui
                    .add_sized(egui::vec2(add_w, 26.0), egui::Button::new("オノマトペ追加"))
                    .clicked()
                {
                    self.show_onomatopoeia_dialog = true;
                }
                ui.add_space(2.0);
                if ui
                    .add_sized(egui::vec2(add_w, 26.0), egui::Button::new("スタンプ追加"))
                    .clicked()
                {
                    self.stamp_dialog_replace_target = None;
                    self.show_stamp_dialog = true;
                }
                ui.add_space(4.0);
                ui.separator();

                ui.label(egui::RichText::new("オブジェクト一覧").strong());
                self.draw_object_list(ui);
                ui.add_space(4.0);
                self.draw_object_action_row(ui);

                ui.separator();
                ui.label(egui::RichText::new(&self.status).small().weak());
                if !self.font_loaded {
                    ui.colored_label(
                        Color32::YELLOW,
                        "日本語フォント未検出: テキストは空になります。",
                    );
                }
            });
    }

    /// Right panel: detailed settings for the selected object (補正レイヤー風)。
    fn draw_right_panel(&mut self, ctx: &egui::Context) {
        egui::SidePanel::right("comic_right")
            .resizable(false)
            .exact_width(RIGHT_W)
            .show(ctx, |ui| {
                ui.add_space(6.0);
                ui.heading("詳細設定");
                ui.separator();
                egui::ScrollArea::vertical()
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        self.draw_properties(ui);
                    });
            });
    }

    /// z-DESC order of object indices (front-most first), like a layer stack.
    fn z_sorted_indices(&self) -> Vec<usize> {
        let mut idx: Vec<usize> = (0..self.objects.len()).collect();
        idx.sort_by(|&a, &b| {
            self.objects[b]
                .z
                .cmp(&self.objects[a].z)
                .then(self.objects[b].id.cmp(&self.objects[a].id))
        });
        idx
    }

    /// Object list as selectable cards (local_adjust_lab layer-card style), z-DESC
    /// (front on top). Each card = enabled checkbox + a kind/text label; clicking
    /// the card selects it. No per-row delete (moved to the action row).
    fn draw_object_list(&mut self, ui: &mut egui::Ui) {
        let order = self.z_sorted_indices();
        if order.is_empty() {
            ui.label(egui::RichText::new("(オブジェクトなし)").weak());
            return;
        }
        let mut toggle: Option<u64> = None;
        let mut select: Option<u64> = None;
        let card_w = LEFT_W - 20.0;
        egui::ScrollArea::vertical()
            .max_height(300.0)
            .auto_shrink([false, false])
            .show(ui, |ui| {
                for &i in &order {
                    let obj = &self.objects[i];
                    let oid = obj.id;
                    let enabled = obj.enabled;
                    let label = match &obj.kind {
                        AnnotationKind::Bubble(b) => format!("吹き出し: {}", short(&b.text.text)),
                        AnnotationKind::Text(t) => format!("テキスト: {}", short(&t.text)),
                        AnnotationKind::MessageWindow(w) => {
                            format!("ウィンドウ: {}", short(&w.text.text))
                        }
                        AnnotationKind::Stamp(s) => format!("スタンプ: {}", stamp_label(&s.source)),
                    };
                    let selected = self.selected == Some(oid);
                    let frame_resp = egui::Frame::new()
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
                            ui.set_min_width(card_w - 12.0);
                            let mut row_clicked = false;
                            ui.horizontal(|ui| {
                                ui.spacing_mut().item_spacing.x = 6.0;
                                let mut en = enabled;
                                if ui.checkbox(&mut en, "").changed() {
                                    toggle = Some(oid);
                                }
                                let text_color = if enabled {
                                    Color32::WHITE
                                } else {
                                    Color32::from_gray(140)
                                };
                                if ui
                                    .add(
                                        egui::Label::new(
                                            egui::RichText::new(label).color(text_color),
                                        )
                                        .truncate()
                                        .sense(Sense::click()),
                                    )
                                    .on_hover_cursor(egui::CursorIcon::PointingHand)
                                    .clicked()
                                {
                                    row_clicked = true;
                                }
                                let spacer_w = ui.available_width().max(0.0);
                                if spacer_w > 2.0 {
                                    let (_, sresp) = ui.allocate_exact_size(
                                        egui::vec2(spacer_w, 18.0),
                                        Sense::click(),
                                    );
                                    if sresp
                                        .on_hover_cursor(egui::CursorIcon::PointingHand)
                                        .clicked()
                                    {
                                        row_clicked = true;
                                    }
                                }
                            });
                            row_clicked
                        });
                    if frame_resp.inner {
                        select = Some(oid);
                    }
                    ui.add_space(3.0);
                }
            });
        if let Some(id) = toggle {
            if let Some(o) = self.objects.iter_mut().find(|o| o.id == id) {
                o.enabled = !o.enabled;
            }
            self.baked_dirty = true;
        }
        if let Some(id) = select {
            self.selected = Some(id);
        }
    }

    /// Action button row operating on the SELECTED object: 上へ / 下へ / 複製 /
    /// 削除 (削除 red, like local_adjust_lab). Disabled when nothing is selected.
    fn draw_object_action_row(&mut self, ui: &mut egui::Ui) {
        let has_sel = self.selected.is_some();
        let mut action: Option<ObjAction> = None;
        ui.horizontal(|ui| {
            let gap = 4.0;
            ui.spacing_mut().item_spacing.x = gap;
            let unit_w = ((LEFT_W - 20.0 - gap * 3.0) / 4.0).max(28.0);
            let btn = egui::vec2(unit_w, 24.0);
            if ui
                .add_enabled(has_sel, egui::Button::new("上へ").min_size(btn))
                .clicked()
            {
                action = Some(ObjAction::MoveUp);
            }
            if ui
                .add_enabled(has_sel, egui::Button::new("下へ").min_size(btn))
                .clicked()
            {
                action = Some(ObjAction::MoveDown);
            }
            if ui
                .add_enabled(has_sel, egui::Button::new("複製").min_size(btn))
                .clicked()
            {
                action = Some(ObjAction::Duplicate);
            }
            if ui
                .add_enabled(
                    has_sel,
                    egui::Button::new("削除")
                        .min_size(btn)
                        .fill(Color32::from_rgb(120, 50, 50)),
                )
                .clicked()
            {
                action = Some(ObjAction::Delete);
            }
        });
        if let (Some(a), Some(id)) = (action, self.selected) {
            self.apply_obj_action(id, a);
        }
    }

    /// Swap the selected object's z with its z-order neighbor. `dir = -1` moves
    /// it toward the front (up in the list); `dir = +1` toward the back.
    fn move_selected_z(&mut self, id: u64, dir: i32) {
        // Dense, unique z first so the move is a clean adjacent swap.
        self.normalize_z();
        let order = self.z_sorted_indices();
        let Some(pos) = order.iter().position(|&i| self.objects[i].id == id) else {
            return;
        };
        let neighbor_pos = pos as i32 + dir;
        if neighbor_pos < 0 || neighbor_pos as usize >= order.len() {
            return;
        }
        let a = order[pos];
        let b = order[neighbor_pos as usize];
        let za = self.objects[a].z;
        self.objects[a].z = self.objects[b].z;
        self.objects[b].z = za;
        self.baked_dirty = true;
    }

    /// Modal preset-picker for "吹き出し追加" (like the adjustment lab's effect
    /// picker). Shows the bubble presets as a grid of rendered thumbnails; a
    /// click inserts that preset bubble at the default pivot and closes.
    fn draw_add_dialog(&mut self, ctx: &egui::Context) {
        if !self.show_add_dialog {
            return;
        }
        let mut open = self.show_add_dialog;
        let mut chosen: Option<BubblePreset> = None;
        let dialog_frame = egui::Frame::window(ctx.style().as_ref())
            .fill(Color32::from_rgba_unmultiplied(24, 24, 26, 248))
            .stroke(egui::Stroke::new(
                1.0,
                Color32::from_rgba_unmultiplied(255, 255, 255, 70),
            ));
        // Resizable window. The grid is laid out MANUALLY (column count from the
        // actual width) instead of `horizontal_wrapped`, which reports its
        // unwrapped one-row width as the window's min width and forces the window
        // full-width (the "幅が目一杯" + weird-resize symptom).
        let avail = ctx.content_rect();
        let max_w = (avail.width() - 24.0).max(PRESET_CELL_W + 48.0);
        let default_w = max_w.min(540.0);
        let default_h = (avail.height() - 120.0).clamp(220.0, 560.0);
        egui::Window::new("吹き出しを追加")
            // Fresh id so egui doesn't restore the full-width size remembered from
            // the earlier horizontal_wrapped layout (a .max_width clamp can't shrink
            // an already-remembered window size — Codex).
            .id(egui::Id::new("add_bubble_dialog_grid"))
            .order(egui::Order::Foreground)
            .frame(dialog_frame)
            // No pivot: a CENTER_CENTER pivot makes resizing feel odd (size changes
            // keep the center fixed). Center via default_pos instead (Codex).
            .default_pos(avail.center() - egui::vec2(default_w, default_h) * 0.5)
            .collapsible(false)
            .resizable(true)
            .default_size([default_w, default_h])
            .min_width(PRESET_CELL_W + 30.0)
            .max_width(max_w)
            .open(&mut open)
            .show(ctx, |ui| {
                ui.label(
                    egui::RichText::new("形を選んでください。クリックで追加します。")
                        .size(11.0)
                        .color(Color32::from_gray(180)),
                );
                ui.separator();
                egui::ScrollArea::vertical()
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        let cols = grid_cols(ui.available_width(), PRESET_CELL_W);
                        ui.spacing_mut().item_spacing =
                            egui::vec2(PRESET_CELL_SPACING, PRESET_CELL_SPACING);
                        for chunk in BubblePreset::ALL.chunks(cols) {
                            ui.horizontal_top(|ui| {
                                for &preset in chunk {
                                    if draw_preset_thumbnail(ui, preset) {
                                        chosen = Some(preset);
                                    }
                                }
                            });
                        }
                    });
            });

        self.show_add_dialog = open;
        if let Some(preset) = chosen {
            self.add_bubble_preset(preset);
            self.show_add_dialog = false;
        }
    }

    /// Message-window add dialog: a grid of window-style preset previews; a click
    /// inserts a window with that preset applied at the default placement.
    fn draw_add_window_dialog(&mut self, ctx: &egui::Context) {
        if !self.show_add_window_dialog {
            return;
        }
        let mut open = self.show_add_window_dialog;
        let mut chosen: Option<usize> = None;
        let dialog_frame = egui::Frame::window(ctx.style().as_ref())
            .fill(Color32::from_rgba_unmultiplied(24, 24, 26, 248))
            .stroke(egui::Stroke::new(
                1.0,
                Color32::from_rgba_unmultiplied(255, 255, 255, 70),
            ));
        // Resizable window; grid laid out manually (column count from the actual
        // width) so it reflows on resize instead of forcing a one-row min width.
        let avail = ctx.content_rect();
        let max_w = (avail.width() - 24.0).max(WINDOW_PRESET_CELL_W + 48.0);
        let default_w = max_w.min(640.0);
        let default_h = (avail.height() - 120.0).clamp(220.0, 560.0);
        egui::Window::new("メッセージウィンドウを追加")
            .id(egui::Id::new("add_window_dialog_grid"))
            .order(egui::Order::Foreground)
            .frame(dialog_frame)
            .default_pos(avail.center() - egui::vec2(default_w, default_h) * 0.5)
            .collapsible(false)
            .resizable(true)
            .default_size([default_w, default_h])
            .min_width(WINDOW_PRESET_CELL_W + 30.0)
            .max_width(max_w)
            .open(&mut open)
            .show(ctx, |ui| {
                ui.label(
                    egui::RichText::new("デザインを選んでください。クリックで追加します。")
                        .size(11.0)
                        .color(Color32::from_gray(180)),
                );
                ui.separator();
                egui::ScrollArea::vertical()
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        let cols = grid_cols(ui.available_width(), WINDOW_PRESET_CELL_W);
                        ui.spacing_mut().item_spacing =
                            egui::vec2(PRESET_CELL_SPACING, PRESET_CELL_SPACING);
                        let n = self.window_presets.len();
                        for row_start in (0..n).step_by(cols) {
                            ui.horizontal_top(|ui| {
                                for i in row_start..(row_start + cols).min(n) {
                                    if draw_window_preset_thumbnail(ui, &self.window_presets[i]) {
                                        chosen = Some(i);
                                    }
                                }
                            });
                        }
                    });
            });

        self.show_add_window_dialog = open;
        if let Some(idx) = chosen {
            self.add_message_window_with_preset(idx);
            self.show_add_window_dialog = false;
        }
    }

    /// One fixed-size onomatopoeia preset card. The preview is a baked
    /// comic-core image, so it reflects the actual bundled/system font instead
    /// of egui's UI font.
    fn draw_onomatopoeia_thumbnail(
        &mut self,
        ui: &mut egui::Ui,
        ctx: &egui::Context,
        preset: OnomatopoeiaPreset,
    ) -> bool {
        const CELL_W: f32 = ONOMATO_PRESET_CELL_W;
        const PREVIEW_H: f32 = 104.0;
        const LABEL_H: f32 = 30.0;
        const PAD: f32 = 7.0;
        let (rect, resp) =
            ui.allocate_exact_size(egui::vec2(CELL_W, PREVIEW_H + LABEL_H), Sense::click());
        let hovered = resp.hovered();
        let painter = ui.painter_at(rect);
        painter.rect_filled(
            rect,
            4.0,
            if hovered {
                Color32::from_rgb(66, 66, 70)
            } else {
                Color32::from_rgb(42, 42, 46)
            },
        );
        painter.rect_stroke(
            rect,
            4.0,
            egui::Stroke::new(
                1.0,
                if hovered {
                    Color32::from_rgb(150, 195, 255)
                } else {
                    Color32::from_gray(70)
                },
            ),
            egui::StrokeKind::Inside,
        );
        let preview_area = Rect::from_min_max(
            rect.min + egui::vec2(PAD, PAD),
            egui::pos2(rect.max.x - PAD, rect.min.y + PREVIEW_H - 5.0),
        );
        painter.rect_filled(preview_area, 3.0, Color32::from_rgb(34, 34, 38));
        if let Some(tex) = self.onomatopoeia_thumb_texture(ctx, preset) {
            let sz = tex.size_vec2();
            if sz.x > 0.0 && sz.y > 0.0 {
                let scale = (preview_area.width() / sz.x)
                    .min(preview_area.height() / sz.y)
                    .min(1.45);
                let draw = egui::vec2(sz.x * scale, sz.y * scale);
                let origin = preview_area.center() - draw * 0.5;
                painter.image(
                    tex.id(),
                    Rect::from_min_size(origin, draw),
                    Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
                    Color32::WHITE,
                );
            }
        } else {
            paint_onomatopoeia_preview(&painter, preview_area.shrink(6.0), preset);
        }
        let label_area = Rect::from_min_max(
            egui::pos2(rect.min.x + 6.0, rect.max.y - LABEL_H),
            egui::pos2(rect.max.x - 6.0, rect.max.y - 3.0),
        );
        painter.text(
            label_area.center(),
            egui::Align2::CENTER_CENTER,
            preset.label(),
            egui::FontId::proportional(11.0),
            Color32::WHITE,
        );
        resp.clicked()
    }

    /// Font-generated SFX add dialog. The chosen preset inserts a normal
    /// standalone Text object with bold comic styling; image SFX remain stamps.
    fn draw_onomatopoeia_dialog(&mut self, ctx: &egui::Context) {
        if !self.show_onomatopoeia_dialog {
            return;
        }
        let mut open = self.show_onomatopoeia_dialog;
        let mut chosen: Option<OnomatopoeiaPreset> = None;
        let dialog_frame = egui::Frame::window(ctx.style().as_ref())
            .fill(Color32::from_rgba_unmultiplied(24, 24, 26, 248))
            .stroke(egui::Stroke::new(
                1.0,
                Color32::from_rgba_unmultiplied(255, 255, 255, 70),
            ));
        let avail = ctx.content_rect();
        let max_w = (avail.width() - 24.0).max(ONOMATO_PRESET_CELL_W + 48.0);
        let default_w = max_w.min(860.0);
        let default_h = (avail.height() - 120.0).clamp(300.0, 660.0);
        egui::Window::new("オノマトペを追加")
            .id(egui::Id::new("onomatopoeia_dialog_grid"))
            .order(egui::Order::Foreground)
            .frame(dialog_frame)
            .default_pos(avail.center() - egui::vec2(default_w, default_h) * 0.5)
            .collapsible(false)
            .resizable(true)
            .default_size([default_w, default_h])
            .min_width(ONOMATO_PRESET_CELL_W + 30.0)
            .max_width(max_w)
            .open(&mut open)
            .show(ctx, |ui| {
                ui.label(
                    egui::RichText::new(
                        "フォントごとのサンプルを選んでください。クリックで追加します。",
                    )
                    .size(11.0)
                    .color(Color32::from_gray(180)),
                );
                ui.separator();
                egui::ScrollArea::vertical()
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        let cols = grid_cols(ui.available_width(), ONOMATO_PRESET_CELL_W);
                        ui.spacing_mut().item_spacing =
                            egui::vec2(PRESET_CELL_SPACING, PRESET_CELL_SPACING);
                        for chunk in OnomatopoeiaPreset::ALL.chunks(cols) {
                            ui.horizontal_top(|ui| {
                                for &preset in chunk {
                                    if self.draw_onomatopoeia_thumbnail(ui, ctx, preset) {
                                        chosen = Some(preset);
                                    }
                                }
                            });
                        }
                    });
            });

        self.show_onomatopoeia_dialog = open;
        if let Some(preset) = chosen {
            self.add_onomatopoeia_preset(preset);
            self.show_onomatopoeia_dialog = false;
        }
    }

    fn apply_obj_action(&mut self, id: u64, action: ObjAction) {
        match action {
            ObjAction::MoveUp => {
                self.move_selected_z(id, -1);
            }
            ObjAction::MoveDown => {
                self.move_selected_z(id, 1);
            }
            ObjAction::Duplicate => {
                if let Some(src) = self.objects.iter().find(|o| o.id == id).cloned() {
                    let maxz = self.objects.iter().map(|o| o.z).max().unwrap_or(0);
                    let new_id = self.next_id;
                    self.next_id += 1;
                    let mut dup = src;
                    dup.id = new_id;
                    dup.pivot = (dup.pivot.0 + 24.0, dup.pivot.1 + 24.0);
                    dup.z = maxz + 1;
                    self.objects.push(dup);
                    self.selected = Some(new_id);
                }
            }
            ObjAction::Delete => {
                self.objects.retain(|o| o.id != id);
                self.tail_stash.remove(&id);
                if self.selected == Some(id) {
                    self.selected = None;
                }
            }
        }
        self.normalize_z();
        self.baked_dirty = true;
    }

    /// Right-panel properties for the selected object. Text is the star: the
    /// テキスト section comes first; for a bubble, the 吹き出し section follows.
    /// (操作 row moved to the left panel; shape ComboBox replaced by a coherent
    /// preset switch.)
    fn draw_properties(&mut self, ui: &mut egui::Ui) {
        let Some(sel) = self.selected else {
            ui.label(egui::RichText::new("(選択なし)").weak());
            return;
        };
        if !self.objects.iter().any(|o| o.id == sel) {
            self.selected = None;
            return;
        }

        let kind_disc = self.objects.iter().find(|o| o.id == sel).map(|o| &o.kind);
        // Stamps have their own (image, not text) property panel.
        if matches!(kind_disc, Some(AnnotationKind::Stamp(_))) {
            self.draw_stamp_properties(ui, sel);
            return;
        }
        let is_bubble = matches!(kind_disc, Some(AnnotationKind::Bubble(_)));
        let is_window = matches!(kind_disc, Some(AnnotationKind::MessageWindow(_)));
        // Keep the active tab valid for the selected kind: text-only → セリフ;
        // window has no 飾り (Deco) → fall back to 本体/枠.
        if !is_bubble && !is_window {
            self.prop_tab = PropTab::Serifu;
        } else if is_window && self.prop_tab == PropTab::Deco {
            self.prop_tab = PropTab::Body;
        }
        ui.label(if is_bubble {
            "種類: 吹き出し"
        } else if is_window {
            "種類: メッセージウィンドウ"
        } else {
            "種類: テキスト"
        });
        ui.separator();

        let mut dirty = false;

        // ===== 常時表示 (above the tabs) =====
        // Windows: a 名前 (speaker) row above the body — names change often, so
        // it lives at the top rather than in the 部品 tab.
        if is_window {
            self.draw_window_name_header(ui, sel, &mut dirty);
        }

        // Does the window's body text overflow its panel? (Used to flag the
        // text field red so the user notices.)
        let window_overflow = is_window
            && match self.objects.iter().find(|o| o.id == sel).map(|o| &o.kind) {
                Some(AnnotationKind::MessageWindow(w)) => {
                    comic_core::message_window_overflows(w, &self.fonts)
                }
                _ => false,
            };

        // 本文テキスト + (記法 ON 時) 記号挿入ボタン. Red frame + warning when the
        // window text overflows.
        if window_overflow {
            ui.colored_label(
                Color32::from_rgb(235, 100, 100),
                "(!) テキストが枠に収まっていません",
            );
            egui::Frame::new()
                .stroke(egui::Stroke::new(2.0, Color32::from_rgb(220, 70, 70)))
                .inner_margin(3.0)
                .show(ui, |ui| {
                    let obj = self.objects.iter_mut().find(|o| o.id == sel).unwrap();
                    let tb = obj.text_block_mut().expect("text-bearing object");
                    let res = draw_text_body(ui, tb, sel);
                    if res.break_link {
                        tb.preset_link = None;
                    }
                    if res.dirty {
                        dirty = true;
                    }
                });
        } else {
            let obj = self.objects.iter_mut().find(|o| o.id == sel).unwrap();
            let tb = obj.text_block_mut().expect("text-bearing object");
            let res = draw_text_body(ui, tb, sel);
            if res.break_link {
                tb.preset_link = None;
            }
            dirty |= res.dirty;
        }

        // 吹き出し自動サイズ: just under 記号挿入 (frequently toggled, so it's
        // here rather than buried in the 本体 tab).
        if is_bubble {
            self.draw_bubble_autosize_toggle(ui, sel, &mut dirty);
        }

        // プリセット (color-coded): セリフプリセット (always) + 本体プリセット
        // (bubbles) / ウィンドウプリセット (windows). Each in its color bar.
        ui.add_space(4.0);
        draw_section_bar(ui, PropTab::Serifu.color(), |ui| {
            self.draw_text_preset_area(ui, sel)
        });
        if is_bubble {
            draw_section_bar(ui, PropTab::Body.color(), |ui| {
                self.draw_shape_preset_area(ui, sel, &mut dirty)
            });
        } else if is_window {
            draw_section_bar(ui, PropTab::Body.color(), |ui| {
                self.draw_window_preset_area(ui, sel, &mut dirty)
            });
        }

        // 構造トグル (bubbles only): 結合 / しっぽ有無 / 飾り有無. These gate which
        // detail tabs are enabled.
        let (tail_enabled, deco_enabled) = if is_bubble {
            self.draw_bubble_toggles(ui, sel, &mut dirty);
            match self.objects.iter().find(|o| o.id == sel).map(|o| &o.kind) {
                // A tail only counts when the shape actually renders one — a stale
                // tail on a tailless shape (e.g. a hand-edited sidecar) must not
                // enable the Tail tab.
                Some(AnnotationKind::Bubble(b)) => (
                    b.tail.is_some() && shape_renders_tail(&b.shape),
                    !b.decorations.is_empty(),
                ),
                _ => (false, false),
            }
        } else {
            (false, false)
        };

        if (self.prop_tab == PropTab::Tail && !tail_enabled && is_bubble)
            || (self.prop_tab == PropTab::Deco && !deco_enabled)
        {
            self.prop_tab = if is_bubble || is_window {
                PropTab::Body
            } else {
                PropTab::Serifu
            };
        }

        // ===== 詳細タブ (color-coded, only the active tab is drawn) =====
        ui.add_space(6.0);
        ui.separator();
        if is_bubble {
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing = egui::vec2(4.0, 4.0);
                for tab in [PropTab::Serifu, PropTab::Body, PropTab::Tail, PropTab::Deco] {
                    let enabled = match tab {
                        PropTab::Tail => tail_enabled,
                        PropTab::Deco => deco_enabled,
                        _ => true,
                    };
                    if prop_tab_button(ui, tab, self.prop_tab == tab, enabled) {
                        self.prop_tab = tab;
                    }
                }
            });
            ui.add_space(4.0);
            let tab = self.prop_tab;
            let color = tab.color();
            match tab {
                PropTab::Serifu => {
                    draw_section_bar(ui, color, |ui| self.tab_serifu(ui, sel, &mut dirty));
                }
                PropTab::Body => {
                    draw_section_bar(ui, color, |ui| self.tab_body(ui, sel, &mut dirty));
                }
                PropTab::Tail => {
                    draw_section_bar(ui, color, |ui| self.tab_tail(ui, sel, &mut dirty));
                }
                PropTab::Deco => {
                    draw_section_bar(ui, color, |ui| self.tab_deco(ui, sel, &mut dirty));
                }
            }
        } else if is_window {
            // Window tabs reuse the Serifu / Body / Tail slots+colors under the
            // names セリフ / 枠 / 部品 (no 飾り tab).
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing = egui::vec2(4.0, 4.0);
                for (tab, label) in [
                    (PropTab::Serifu, "セリフ"),
                    (PropTab::Body, "枠"),
                    (PropTab::Tail, "部品"),
                ] {
                    if prop_tab_button_labeled(ui, tab, self.prop_tab == tab, true, label) {
                        self.prop_tab = tab;
                    }
                }
            });
            ui.add_space(4.0);
            let tab = self.prop_tab;
            let color = tab.color();
            match tab {
                PropTab::Serifu => {
                    draw_section_bar(ui, color, |ui| self.tab_serifu(ui, sel, &mut dirty));
                }
                PropTab::Tail => {
                    draw_section_bar(ui, color, |ui| self.tab_window_parts(ui, sel, &mut dirty));
                }
                // Body (or any fallback) → window frame/geometry.
                _ => {
                    draw_section_bar(ui, PropTab::Body.color(), |ui| {
                        self.tab_window_body(ui, sel, &mut dirty)
                    });
                }
            }
        } else {
            // Text-only object: only the セリフ category applies.
            draw_section_bar(ui, PropTab::Serifu.color(), |ui| {
                self.tab_serifu(ui, sel, &mut dirty)
            });
        }

        // Re-resolve a positioned window's placement after any edit that could
        // change its fitted size (AutoFitText body/size/padding, etc.). No-op for
        // Free windows (apply_window_placement early-returns).
        if is_window && dirty {
            self.apply_window_placement(sel);
        }

        if dirty {
            self.baked_dirty = true;
        }
    }

    /// 吹き出し自動サイズ checkbox (always-visible, below 記号挿入). Turning it
    /// off freezes the current fitted size into the shape so manual size edits
    /// continue from there; toggling it clears the shape-preset link.
    fn draw_bubble_autosize_toggle(&mut self, ui: &mut egui::Ui, sel: u64, dirty: &mut bool) {
        let obj = self.objects.iter_mut().find(|o| o.id == sel).unwrap();
        let AnnotationKind::Bubble(b) = &mut obj.kind else {
            return;
        };
        let mut auto = b.auto_size;
        if ui.checkbox(&mut auto, "吹き出し自動サイズ").changed() {
            if !auto {
                // Freeze the fitted size before leaving auto mode.
                b.shape = effective_bubble_shape(b, &self.fonts);
            }
            b.auto_size = auto;
            b.shape_preset_link = None;
            *dirty = true;
        }
    }

    /// Structural on/off toggles shown ABOVE the detail tabs: merge-with-below,
    /// しっぽ有無 (tail Some/None, stashed), 飾り有無 (decorations, stashed). The
    /// tail/deco toggles gate whether the しっぽ / 飾り tabs are enabled.
    fn draw_bubble_toggles(&mut self, ui: &mut egui::Ui, sel: u64, dirty: &mut bool) {
        let pivot = self
            .objects
            .iter()
            .find(|o| o.id == sel)
            .map(|o| o.pivot)
            .unwrap_or((0.0, 0.0));
        let obj = self.objects.iter_mut().find(|o| o.id == sel).unwrap();
        let AnnotationKind::Bubble(b) = &mut obj.kind else {
            return;
        };

        ui.add_space(2.0);
        // 結合 (structural; not part of any preset → re-bake only). Only shapes with
        // a solid fillable body can union (the fill→stroke→erase trick); fuzzy /
        // line-field / text-only shapes (意識 / 集中線 / 流線 / なし) can't, so disable
        // the toggle for them and clear any stale flag (e.g. set on a previous shape).
        let merge_supported = comic_core::shape_is_mergeable(&b.shape);
        if !merge_supported && b.merge_with_below {
            b.merge_with_below = false;
            *dirty = true;
        }
        let merge_resp = ui.add_enabled(
            merge_supported,
            egui::Checkbox::new(&mut b.merge_with_below, "下の吹き出しと結合"),
        );
        if merge_supported && merge_resp.changed() {
            *dirty = true;
        }
        if !merge_supported {
            // Disabled widgets ignore `on_hover_text`; must use the disabled variant.
            merge_resp.on_disabled_hover_text("この形状は結合に対応していません");
        }

        // しっぽ有無 (stashed so toggling off→on doesn't move it). Tailless shapes
        // (集中線 / 流線 / 意識 / なし) draw no tail, so disable the toggle for them
        // (prevents creating invisible selectable tail geometry).
        let tail_supported = shape_renders_tail(&b.shape);
        let mut has_tail = b.tail.is_some();
        let tail_resp = ui.add_enabled(
            tail_supported,
            egui::Checkbox::new(&mut has_tail, "しっぽを表示"),
        );
        if tail_supported && tail_resp.changed() {
            if has_tail {
                b.tail = Some(
                    self.tail_stash
                        .remove(&sel)
                        .unwrap_or_else(|| default_bubble_tail(pivot)),
                );
            } else if let Some(t) = b.tail.take() {
                self.tail_stash.insert(sel, t);
            }
            // Tail kind is captured by the shape preset → adding/removing it
            // breaks the link (glow off).
            b.shape_preset_link = None;
            *dirty = true;
        }
        if !tail_supported {
            // Disabled widgets ignore `on_hover_text`; must use the disabled variant.
            tail_resp.on_disabled_hover_text("この形状はしっぽに対応していません");
        }

        // 飾り有無 (decorations non-empty = on, stashed). Seeding a single
        // default layer is fine (no overlap issue until a 2nd is added).
        let mut has_deco = !b.decorations.is_empty();
        if ui.checkbox(&mut has_deco, "飾りを使う").changed() {
            if has_deco {
                b.decorations = self
                    .deco_stash
                    .remove(&sel)
                    .filter(|v| !v.is_empty())
                    .unwrap_or_else(|| vec![comic_core::DecorationLayer::default()]);
            } else {
                let taken = std::mem::take(&mut b.decorations);
                self.deco_stash.insert(sel, taken);
            }
            *dirty = true;
        }
    }

    /// セリフ tab: フォント (見本ダイアログで選択) + サイズ + 文字スタイル
    /// (色 / 組方向 / 縦中横 / 記法 / 行揃え / 行間字間 / 袋文字). Operates on the
    /// TextBlock so it works for bubbles / text / windows.
    fn tab_serifu(&mut self, ui: &mut egui::Ui, sel: u64, dirty: &mut bool) {
        // Body-text STYLE edits (font/size/color/orientation/markup/outline) are
        // also captured by a window-style preset, so a break here must clear the
        // window's `style_preset_link` too (handled after the borrows end).
        let mut style_broke = false;
        let open_font_dialog = {
            let obj = self.objects.iter_mut().find(|o| o.id == sel).unwrap();
            let tb = obj.text_block_mut().expect("text-bearing object");
            let res = draw_text_font(ui, tb, sel);
            if res.break_link {
                tb.preset_link = None;
                style_broke = true;
            }
            if res.dirty {
                *dirty = true;
            }
            res.open_font_dialog
        };
        if open_font_dialog {
            self.open_font_dialog(sel);
        }

        ui.add_space(2.0);
        // 文字スタイル controls.
        {
            let obj = self.objects.iter_mut().find(|o| o.id == sel).unwrap();
            let tb = obj.text_block_mut().expect("text-bearing object");
            let res = draw_serifu_tab(ui, tb);
            if res.break_link {
                tb.preset_link = None;
                style_broke = true;
            }
            if res.dirty {
                *dirty = true;
            }
        }
        // A body-text style change also unlinks a message-window style preset.
        if style_broke {
            if let Some(o) = self.objects.iter_mut().find(|o| o.id == sel) {
                if let AnnotationKind::MessageWindow(w) = &mut o.kind {
                    w.style_preset_link = None;
                }
            }
        }
    }

    /// セリフプリセット area: system + user preset buttons (1-click apply / link;
    /// the active one is highlighted), a name field (pre-filled with the active
    /// preset's name on apply) + 登録 + 削除. 登録 with the same name updates, a
    /// new name creates; 削除 removes the same-named user preset.
    fn draw_text_preset_area(&mut self, ui: &mut egui::Ui, sel: u64) {
        ui.label(egui::RichText::new("セリフプリセット").strong());
        let active_link = self
            .objects
            .iter()
            .find(|o| o.id == sel)
            .and_then(|o| o.text_block())
            .and_then(|tb| tb.preset_link.clone());

        let mut apply_idx: Option<usize> = None;
        let n = self.text_presets.len();
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing = egui::vec2(4.0, 4.0);
            for i in 0..n {
                let (id, name) = {
                    let p = &self.text_presets[i];
                    (p.id.clone(), p.name.clone())
                };
                let active = active_link.as_deref() == Some(id.as_str());
                let fill = if active {
                    Color32::from_rgb(36, 112, 150)
                } else {
                    Color32::from_rgba_unmultiplied(70, 70, 70, 190)
                };
                if ui.add(egui::Button::new(&name).fill(fill)).clicked() {
                    apply_idx = Some(i);
                }
            }
        });
        // 削除 targets the USER preset whose name matches the field (the active
        // preset's name is pre-filled on apply).
        let name_trim = self.text_preset_name_input.trim().to_string();
        let user_match = self
            .text_presets
            .iter()
            .position(|p| !is_system_preset(&p.id) && p.name == name_trim);
        let mut do_register = false;
        let mut do_delete = false;
        ui.horizontal(|ui| {
            ui.add(
                egui::TextEdit::singleline(&mut self.text_preset_name_input)
                    .desired_width(120.0)
                    .hint_text("プリセット名"),
            );
            if ui.button("登録").clicked() {
                do_register = true;
            }
            if ui
                .add_enabled(user_match.is_some(), egui::Button::new("削除"))
                .on_hover_text("同名のユーザープリセットを削除")
                .clicked()
            {
                do_delete = true;
            }
        });

        if let Some(i) = apply_idx {
            self.apply_text_preset_by_index(i);
        }
        if do_register {
            let name = self.text_preset_name_input.clone();
            self.register_text_preset(&name);
        }
        if do_delete {
            // Re-resolve from the CURRENT field value (this frame's edits may have
            // changed it after `user_match` was computed for the button state).
            let name = self.text_preset_name_input.trim().to_string();
            if let Some(i) = self
                .text_presets
                .iter()
                .position(|p| !is_system_preset(&p.id) && p.name == name)
            {
                self.delete_text_preset(i);
                self.text_preset_name_input.clear();
            }
        }
    }

    /// 形状プリセット area: glowing system + user shape-preset buttons (1-click
    /// apply / link), a name field + 登録, and a small × on user presets. Sets
    /// `*dirty` when an apply happens (the per-control edits set it separately).
    fn draw_shape_preset_area(&mut self, ui: &mut egui::Ui, sel: u64, dirty: &mut bool) {
        ui.label(egui::RichText::new("本体プリセット").strong());
        let active_link = self
            .objects
            .iter()
            .find(|o| o.id == sel)
            .and_then(|o| match &o.kind {
                AnnotationKind::Bubble(b) => b.shape_preset_link.clone(),
                _ => None,
            });

        let mut apply_idx: Option<usize> = None;
        let n = self.shape_presets.len();
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing = egui::vec2(4.0, 4.0);
            for i in 0..n {
                let (id, name) = {
                    let p = &self.shape_presets[i];
                    (p.id.clone(), p.name.clone())
                };
                let active = active_link.as_deref() == Some(id.as_str());
                let fill = if active {
                    Color32::from_rgb(36, 112, 150)
                } else {
                    Color32::from_rgba_unmultiplied(70, 70, 70, 190)
                };
                if ui.add(egui::Button::new(&name).fill(fill)).clicked() {
                    apply_idx = Some(i);
                }
            }
        });
        let name_trim = self.shape_preset_name_input.trim().to_string();
        let user_match = self
            .shape_presets
            .iter()
            .position(|p| !is_system_preset(&p.id) && p.name == name_trim);
        let mut do_register = false;
        let mut do_delete = false;
        ui.horizontal(|ui| {
            ui.add(
                egui::TextEdit::singleline(&mut self.shape_preset_name_input)
                    .desired_width(120.0)
                    .hint_text("プリセット名"),
            );
            if ui.button("登録").clicked() {
                do_register = true;
            }
            if ui
                .add_enabled(user_match.is_some(), egui::Button::new("削除"))
                .on_hover_text("同名のユーザープリセットを削除")
                .clicked()
            {
                do_delete = true;
            }
        });

        if let Some(i) = apply_idx {
            self.apply_shape_preset_by_index(i);
            *dirty = true;
        }
        if do_register {
            let name = self.shape_preset_name_input.clone();
            self.register_shape_preset(&name);
            *dirty = true;
        }
        if do_delete {
            let name = self.shape_preset_name_input.trim().to_string();
            if let Some(i) = self
                .shape_presets
                .iter()
                .position(|p| !is_system_preset(&p.id) && p.name == name)
            {
                self.delete_shape_preset(i);
                self.shape_preset_name_input.clear();
                *dirty = true;
            }
        }
    }

    /// ウィンドウプリセット area (system + user, 1-click apply / 登録 / delete).
    fn draw_window_preset_area(&mut self, ui: &mut egui::Ui, sel: u64, dirty: &mut bool) {
        ui.label(egui::RichText::new("ウィンドウプリセット").strong());
        let active_link = self
            .objects
            .iter()
            .find(|o| o.id == sel)
            .and_then(|o| match &o.kind {
                AnnotationKind::MessageWindow(w) => w.style_preset_link.clone(),
                _ => None,
            });

        let mut apply_idx: Option<usize> = None;
        let n = self.window_presets.len();
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing = egui::vec2(4.0, 4.0);
            for i in 0..n {
                let (id, name) = {
                    let p = &self.window_presets[i];
                    (p.id.clone(), p.name.clone())
                };
                let active = active_link.as_deref() == Some(id.as_str());
                let fill = if active {
                    Color32::from_rgb(36, 112, 150)
                } else {
                    Color32::from_rgba_unmultiplied(70, 70, 70, 190)
                };
                if ui.add(egui::Button::new(&name).fill(fill)).clicked() {
                    apply_idx = Some(i);
                }
            }
        });
        let name_trim = self.window_preset_name_input.trim().to_string();
        let user_match = self
            .window_presets
            .iter()
            .position(|p| !is_system_preset(&p.id) && p.name == name_trim);
        let mut do_register = false;
        let mut do_delete = false;
        ui.horizontal(|ui| {
            ui.add(
                egui::TextEdit::singleline(&mut self.window_preset_name_input)
                    .desired_width(120.0)
                    .hint_text("プリセット名"),
            );
            if ui.button("登録").clicked() {
                do_register = true;
            }
            if ui
                .add_enabled(user_match.is_some(), egui::Button::new("削除"))
                .on_hover_text("同名のユーザープリセットを削除")
                .clicked()
            {
                do_delete = true;
            }
        });

        if let Some(i) = apply_idx {
            self.apply_window_preset_by_index(i);
            *dirty = true;
        }
        if do_register {
            let name = self.window_preset_name_input.clone();
            self.register_window_preset(&name);
            *dirty = true;
        }
        if do_delete {
            let name = self.window_preset_name_input.trim().to_string();
            if let Some(i) = self
                .window_presets
                .iter()
                .position(|p| !is_system_preset(&p.id) && p.name == name)
            {
                self.delete_window_preset(i);
                self.window_preset_name_input.clear();
                *dirty = true;
            }
        }
    }

    /// 枠 tab (windows): position / size / corner / fill / frame / shadow / text
    /// anchor / wrap / padding. Position/size edits re-resolve the placement.
    /// Any individual edit clears `style_preset_link`.
    fn tab_window_body(&mut self, ui: &mut egui::Ui, sel: u64, dirty: &mut bool) {
        let mut edited = false;
        let mut placement_changed = false;
        {
            let obj = self.objects.iter_mut().find(|o| o.id == sel).unwrap();
            let AnnotationKind::MessageWindow(w) = &mut obj.kind else {
                return;
            };

            ui.horizontal(|ui| {
                ui.label("位置");
                for (lbl, p) in [
                    ("上", WindowPosition::Top),
                    ("中", WindowPosition::Middle),
                    ("下", WindowPosition::Bottom),
                    ("中央", WindowPosition::Center),
                    ("自由", WindowPosition::Free),
                ] {
                    if ui.radio(w.position == p, lbl).clicked() {
                        w.position = p;
                        edited = true;
                        placement_changed = true;
                    }
                }
            });
            ui.horizontal(|ui| {
                ui.label("サイズ");
                for (lbl, m) in [
                    ("全幅", SizeMode::FullWidth),
                    ("固定", SizeMode::Inset),
                    ("文字に合わせ", SizeMode::AutoFitText),
                ] {
                    if ui.radio(w.size_mode == m, lbl).clicked() {
                        w.size_mode = m;
                        edited = true;
                        placement_changed = true;
                    }
                }
            });
            match w.size_mode {
                SizeMode::Inset => {
                    edited |= ui
                        .add(egui::Slider::new(&mut w.half_w, 40.0..=1200.0).text("半幅"))
                        .changed();
                    edited |= ui
                        .add(egui::Slider::new(&mut w.half_h, 24.0..=800.0).text("半高"))
                        .changed();
                }
                SizeMode::FullWidth => {
                    if ui
                        .add(egui::Slider::new(&mut w.half_h, 24.0..=800.0).text("半高"))
                        .changed()
                    {
                        edited = true;
                        placement_changed = true;
                    }
                    if ui
                        .add(egui::Slider::new(&mut w.margin_px, 0.0..=300.0).text("左右余白"))
                        .changed()
                    {
                        edited = true;
                        placement_changed = true;
                    }
                }
                SizeMode::AutoFitText => {
                    ui.label(
                        egui::RichText::new("文字量に合わせて自動サイズ")
                            .small()
                            .weak(),
                    );
                }
            }
            edited |= ui
                .add(egui::Slider::new(&mut w.corner_px, 0.0..=80.0).text("角丸"))
                .changed();

            ui.add_space(2.0);
            ui.label(egui::RichText::new("背景").strong());
            ui.horizontal(|ui| {
                ui.label("種類");
                for (lbl, m) in [
                    ("なし", FillMode::None),
                    ("単色", FillMode::Solid),
                    ("半透明", FillMode::Translucent),
                    ("スクリム", FillMode::GradientScrim),
                    ("グラデ", FillMode::LinearGradient),
                ] {
                    if ui.radio(w.fill_mode == m, lbl).clicked() {
                        w.fill_mode = m;
                        edited = true;
                    }
                }
            });
            if w.fill_mode != FillMode::None {
                if w.fill.is_none() {
                    w.fill = Some(Rgba::new(20, 24, 48, 235));
                }
                if let Some(c) = &mut w.fill {
                    ui.horizontal(|ui| {
                        ui.label("色");
                        let mut col = rgba_to_color32(*c);
                        if ui.color_edit_button_srgba(&mut col).changed() {
                            *c = color32_to_rgba(col);
                            edited = true;
                        }
                    });
                }
                edited |= ui
                    .add(egui::Slider::new(&mut w.fill_opacity, 0.0..=1.0).text("不透明度"))
                    .changed();
                if w.fill_mode == FillMode::GradientScrim {
                    ui.horizontal(|ui| {
                        ui.label("濃い側");
                        for (lbl, a) in [
                            ("上", VAnchor::Top),
                            ("中", VAnchor::Center),
                            ("下", VAnchor::Bottom),
                        ] {
                            if ui.radio(w.scrim_dense_side == a, lbl).clicked() {
                                w.scrim_dense_side = a;
                                edited = true;
                            }
                        }
                    });
                }
                if w.fill_mode == FillMode::LinearGradient {
                    if w.gradient_to.is_none() {
                        w.gradient_to = Some(Rgba::new(8, 12, 40, 255));
                    }
                    if let Some(c) = &mut w.gradient_to {
                        ui.horizontal(|ui| {
                            ui.label("下端色");
                            let mut col = rgba_to_color32(*c);
                            if ui.color_edit_button_srgba(&mut col).changed() {
                                *c = color32_to_rgba(col);
                                edited = true;
                            }
                        });
                    }
                }
            }

            ui.add_space(2.0);
            ui.label(egui::RichText::new("枠").strong());
            ui.horizontal(|ui| {
                ui.label("種類");
                for (lbl, f) in [
                    ("なし", FrameStyle::None),
                    ("単線", FrameStyle::SolidRounded),
                    ("二重線", FrameStyle::DoubleLine),
                ] {
                    if ui.radio(w.frame == f, lbl).clicked() {
                        w.frame = f;
                        edited = true;
                    }
                }
            });
            if w.frame != FrameStyle::None {
                ui.horizontal(|ui| {
                    ui.label("枠色");
                    let mut col = rgba_to_color32(w.outline.color);
                    if ui.color_edit_button_srgba(&mut col).changed() {
                        w.outline.color = color32_to_rgba(col);
                        edited = true;
                    }
                });
                edited |= ui
                    .add(egui::Slider::new(&mut w.outline.width_px, 0.0..=12.0).text("枠太さ"))
                    .changed();
                if w.frame == FrameStyle::DoubleLine {
                    edited |= ui
                        .add(egui::Slider::new(&mut w.frame_gap_px, 2.0..=24.0).text("二重間隔"))
                        .changed();
                }
            }

            ui.add_space(2.0);
            let mut has_shadow = w.shadow.is_some();
            if ui.checkbox(&mut has_shadow, "影").changed() {
                w.shadow = if has_shadow {
                    Some(ShadowStyle::default())
                } else {
                    None
                };
                edited = true;
            }
            if let Some(sh) = &mut w.shadow {
                ui.horizontal(|ui| {
                    ui.label("影色");
                    let mut col = rgba_to_color32(sh.color);
                    if ui.color_edit_button_srgba(&mut col).changed() {
                        sh.color = color32_to_rgba(col);
                        edited = true;
                    }
                    ui.label("X");
                    edited |= ui
                        .add(egui::DragValue::new(&mut sh.offset.0).speed(0.5))
                        .changed();
                    ui.label("Y");
                    edited |= ui
                        .add(egui::DragValue::new(&mut sh.offset.1).speed(0.5))
                        .changed();
                });
            }

            ui.add_space(2.0);
            ui.label(egui::RichText::new("テキスト配置").strong());
            ui.horizontal(|ui| {
                ui.label("縦位置");
                for (lbl, a) in [
                    ("上", VAnchor::Top),
                    ("中", VAnchor::Center),
                    ("下", VAnchor::Bottom),
                ] {
                    if ui.radio(w.v_anchor == a, lbl).clicked() {
                        w.v_anchor = a;
                        edited = true;
                    }
                }
            });
            edited |= ui
                .checkbox(&mut w.wrap, "本文を折り返す (禁則処理)")
                .changed();

            ui.add_space(2.0);
            ui.label("余白 (左/上/右/下)");
            ui.horizontal(|ui| {
                edited |= ui
                    .add(egui::DragValue::new(&mut w.padding.left).speed(1.0))
                    .changed();
                edited |= ui
                    .add(egui::DragValue::new(&mut w.padding.top).speed(1.0))
                    .changed();
                edited |= ui
                    .add(egui::DragValue::new(&mut w.padding.right).speed(1.0))
                    .changed();
                edited |= ui
                    .add(egui::DragValue::new(&mut w.padding.bottom).speed(1.0))
                    .changed();
            });

            if edited {
                w.style_preset_link = None;
            }
        }
        if placement_changed {
            self.apply_window_placement(sel);
        }
        if edited || placement_changed {
            *dirty = true;
        }
    }

    /// 部品 tab (windows): name plate / portrait slot / continue indicator.
    fn tab_window_parts(&mut self, ui: &mut egui::Ui, sel: u64, dirty: &mut bool) {
        let obj = self.objects.iter_mut().find(|o| o.id == sel).unwrap();
        let AnnotationKind::MessageWindow(w) = &mut obj.kind else {
            return;
        };
        let mut edited = false;

        ui.label(egui::RichText::new("名前プレート 装飾").strong());
        ui.label(
            egui::RichText::new("表示モード・名前・文字色は上部の「名前」で設定")
                .small()
                .weak(),
        );
        if w.name_plate.mode != NamePlateMode::None {
            edited |= ui
                .add(
                    egui::Slider::new(&mut w.name_plate.name.size_px, 8.0..=120.0)
                        .text("文字サイズ"),
                )
                .changed();
            if matches!(
                w.name_plate.mode,
                NamePlateMode::Boxed | NamePlateMode::Above
            ) {
                let mut has_fill = w.name_plate.fill.is_some();
                if ui.checkbox(&mut has_fill, "プレート塗り").changed() {
                    w.name_plate.fill = if has_fill {
                        Some(Rgba::new(30, 32, 44, 255))
                    } else {
                        None
                    };
                    edited = true;
                }
                if let Some(c) = &mut w.name_plate.fill {
                    ui.horizontal(|ui| {
                        ui.label("塗り色");
                        let mut col = rgba_to_color32(*c);
                        if ui.color_edit_button_srgba(&mut col).changed() {
                            *c = color32_to_rgba(col);
                            edited = true;
                        }
                    });
                }
                ui.horizontal(|ui| {
                    ui.label("枠色");
                    let mut col = rgba_to_color32(w.name_plate.outline.color);
                    if ui.color_edit_button_srgba(&mut col).changed() {
                        w.name_plate.outline.color = color32_to_rgba(col);
                        edited = true;
                    }
                });
                edited |= ui
                    .add(
                        egui::Slider::new(&mut w.name_plate.outline.width_px, 0.0..=10.0)
                            .text("枠太さ"),
                    )
                    .changed();
                edited |= ui
                    .add(egui::Slider::new(&mut w.name_plate.corner_px, 0.0..=40.0).text("角丸"))
                    .changed();
                edited |= ui
                    .add(egui::Slider::new(&mut w.name_plate.padding_px, 0.0..=40.0).text("余白"))
                    .changed();
            }
            ui.horizontal(|ui| {
                ui.label("位置オフセット X/Y");
                edited |= ui
                    .add(egui::DragValue::new(&mut w.name_plate.offset.0).speed(1.0))
                    .changed();
                edited |= ui
                    .add(egui::DragValue::new(&mut w.name_plate.offset.1).speed(1.0))
                    .changed();
            });
        }

        ui.add_space(2.0);
        ui.label(egui::RichText::new("立ち絵枠 (プレースホルダ)").strong());
        ui.horizontal(|ui| {
            ui.label("配置");
            for (lbl, s) in [
                ("なし", PortraitSide::None),
                ("左", PortraitSide::Left),
                ("右", PortraitSide::Right),
            ] {
                if ui.radio(w.portrait.side == s, lbl).clicked() {
                    w.portrait.side = s;
                    edited = true;
                }
            }
        });
        if w.portrait.side != PortraitSide::None {
            edited |= ui
                .add(egui::Slider::new(&mut w.portrait.width_px, 40.0..=600.0).text("幅"))
                .changed();
            if w.portrait.fill.is_none() {
                w.portrait.fill = Some(Rgba::new(70, 74, 92, 255));
            }
            if let Some(c) = &mut w.portrait.fill {
                ui.horizontal(|ui| {
                    ui.label("色");
                    let mut col = rgba_to_color32(*c);
                    if ui.color_edit_button_srgba(&mut col).changed() {
                        *c = color32_to_rgba(col);
                        edited = true;
                    }
                });
            }
            edited |= ui
                .add(egui::Slider::new(&mut w.portrait.margin_px, 0.0..=60.0).text("余白"))
                .changed();
        }

        ui.add_space(2.0);
        ui.label(egui::RichText::new("続き指標").strong());
        ui.horizontal(|ui| {
            for (lbl, k) in [
                ("なし", IndicatorKind::None),
                ("三角", IndicatorKind::Triangle),
                ("山", IndicatorKind::Chevron),
                ("菱", IndicatorKind::Diamond),
                ("点々", IndicatorKind::Dots),
            ] {
                if ui.radio(w.indicator == k, lbl).clicked() {
                    w.indicator = k;
                    edited = true;
                }
            }
        });
        if w.indicator != IndicatorKind::None {
            // Game-like "there's more" behavior: only show the indicator when the
            // body text actually overflows the panel.
            edited |= ui
                .checkbox(&mut w.indicator_auto, "テキストが溢れた時だけ表示")
                .changed();
        }

        if edited {
            w.style_preset_link = None;
            *dirty = true;
        }
    }

    /// Always-visible (windows): speaker-name plate MODE + name text + color. The
    /// frequently-edited name sits above the body text; plate STYLING (size /
    /// fill / outline / corner / padding / offset) stays in the 部品 tab.
    fn draw_window_name_header(&mut self, ui: &mut egui::Ui, sel: u64, dirty: &mut bool) {
        let obj = self.objects.iter_mut().find(|o| o.id == sel).unwrap();
        let AnnotationKind::MessageWindow(w) = &mut obj.kind else {
            return;
        };
        let mut edited = false;
        ui.horizontal(|ui| {
            ui.label("名前:");
            egui::ComboBox::from_id_salt("win_name_mode")
                .selected_text(match w.name_plate.mode {
                    NamePlateMode::None => "なし",
                    NamePlateMode::Inline => "ラベル",
                    NamePlateMode::Boxed => "枠付き",
                    NamePlateMode::Above => "上に",
                })
                .show_ui(ui, |ui| {
                    for (lbl, m) in [
                        ("なし", NamePlateMode::None),
                        ("ラベル", NamePlateMode::Inline),
                        ("枠付き", NamePlateMode::Boxed),
                        ("上に", NamePlateMode::Above),
                    ] {
                        if ui
                            .selectable_value(&mut w.name_plate.mode, m, lbl)
                            .changed()
                        {
                            edited = true;
                        }
                    }
                });
            if w.name_plate.mode != NamePlateMode::None {
                let mut col = rgba_to_color32(w.name_plate.name.color);
                if ui.color_edit_button_srgba(&mut col).changed() {
                    w.name_plate.name.color = color32_to_rgba(col);
                    edited = true;
                }
            }
        });
        if w.name_plate.mode != NamePlateMode::None {
            edited |= ui
                .add(
                    egui::TextEdit::singleline(&mut w.name_plate.name.text)
                        .desired_width(f32::INFINITY)
                        .hint_text("話者名"),
                )
                .changed();
        }
        if edited {
            w.style_preset_link = None;
            *dirty = true;
        }
    }

    /// 吹き出し section: 形状プリセット area → active-shape parameter sliders →
    /// 塗り/枠/不透明度/内側余白 → しっぽ → 装飾. `dirty` is OR'd with any change.
    /// Any INDIVIDUAL edit clears `b.shape_preset_link` (turns the glow off).
    /// 本体 tab: shape kind + auto-size + per-shape params, merge, fill / outline
    /// / padding. Any individual edit clears `b.shape_preset_link` (glow off).
    fn tab_body(&mut self, ui: &mut egui::Ui, sel: u64, dirty: &mut bool) {
        let obj = self.objects.iter_mut().find(|o| o.id == sel).unwrap();
        let AnnotationKind::Bubble(b) = &mut obj.kind else {
            return;
        };

        // Track INDIVIDUAL edits separately so we can clear the link at the end
        // (the preset-area apply, handled elsewhere, is the only thing that SETs it).
        let mut edited = false;

        // The 自動サイズ checkbox itself lives in the always-visible area (below
        // 記号挿入) since it's toggled often; here we just gate the size sliders.
        let auto = b.auto_size;

        // Per-shape fine-tuning sliders for the active shape. Every individual
        // change feeds `edited` (which clears the link below) as well as `dirty`.
        match &mut b.shape {
            BubbleShape::Ellipse { rx, ry, .. } => {
                if !auto {
                    edited |= ui
                        .add(egui::Slider::new(rx, 20.0..=800.0).text("rx"))
                        .changed();
                    edited |= ui
                        .add(egui::Slider::new(ry, 20.0..=800.0).text("ry"))
                        .changed();
                }
            }
            BubbleShape::RoundRect {
                half_w,
                half_h,
                corner_px,
            } => {
                if !auto {
                    edited |= ui
                        .add(egui::Slider::new(half_w, 20.0..=800.0).text("半幅"))
                        .changed();
                    edited |= ui
                        .add(egui::Slider::new(half_h, 20.0..=800.0).text("半高"))
                        .changed();
                }
                edited |= ui
                    .add(egui::Slider::new(corner_px, 0.0..=200.0).text("角丸"))
                    .changed();
            }
            BubbleShape::Burst {
                rx,
                ry,
                spikes,
                jag,
                shape_seed,
            } => {
                if !auto {
                    edited |= ui
                        .add(egui::Slider::new(rx, 20.0..=800.0).text("rx"))
                        .changed();
                    edited |= ui
                        .add(egui::Slider::new(ry, 20.0..=800.0).text("ry"))
                        .changed();
                }
                edited |= ui
                    .add(egui::Slider::new(spikes, 5..=40).text("トゲ数"))
                    .changed();
                edited |= ui
                    .add(egui::Slider::new(jag, 0.2..=0.9).text("トゲの深さ"))
                    .changed();
                ui.horizontal(|ui| {
                    ui.label(format!("seed {shape_seed}"));
                    if ui.button("再生成").clicked() {
                        *shape_seed = shape_seed.wrapping_add(1);
                        edited = true;
                    }
                });
            }
            BubbleShape::Cloud {
                rx,
                ry,
                lobes,
                amp,
                shape_seed,
            } => {
                if !auto {
                    edited |= ui
                        .add(egui::Slider::new(rx, 20.0..=800.0).text("rx"))
                        .changed();
                    edited |= ui
                        .add(egui::Slider::new(ry, 20.0..=800.0).text("ry"))
                        .changed();
                }
                edited |= ui
                    .add(egui::Slider::new(lobes, 5..=24).text("こぶ数"))
                    .changed();
                edited |= ui
                    .add(egui::Slider::new(amp, 0.04..=0.4).text("こぶの深さ"))
                    .changed();
                ui.horizontal(|ui| {
                    ui.label(format!("seed {shape_seed}"));
                    if ui.button("再生成").clicked() {
                        *shape_seed = shape_seed.wrapping_add(1);
                        edited = true;
                    }
                });
            }
            BubbleShape::Polygon { rx, ry, sides } => {
                if !auto {
                    edited |= ui
                        .add(egui::Slider::new(rx, 20.0..=800.0).text("rx"))
                        .changed();
                    edited |= ui
                        .add(egui::Slider::new(ry, 20.0..=800.0).text("ry"))
                        .changed();
                }
                edited |= ui
                    .add(egui::Slider::new(sides, 3..=12).text("辺の数"))
                    .changed();
            }
            BubbleShape::Diamond { half_w, half_h } => {
                if !auto {
                    edited |= ui
                        .add(egui::Slider::new(half_w, 20.0..=800.0).text("半幅"))
                        .changed();
                    edited |= ui
                        .add(egui::Slider::new(half_h, 20.0..=800.0).text("半高"))
                        .changed();
                }
            }
            BubbleShape::Heart { rx, ry } => {
                if !auto {
                    edited |= ui
                        .add(egui::Slider::new(rx, 20.0..=800.0).text("rx"))
                        .changed();
                    edited |= ui
                        .add(egui::Slider::new(ry, 20.0..=800.0).text("ry"))
                        .changed();
                }
            }
            BubbleShape::Arrow {
                half_w,
                half_h,
                dir_rad,
                ..
            } => {
                if !auto {
                    edited |= ui
                        .add(egui::Slider::new(half_w, 20.0..=800.0).text("長さ半分"))
                        .changed();
                    edited |= ui
                        .add(egui::Slider::new(half_h, 20.0..=800.0).text("幅半分"))
                        .changed();
                }
                let mut deg = dir_rad.to_degrees();
                if ui
                    .add(egui::Slider::new(&mut deg, -180.0..=180.0).text("向き(度)"))
                    .changed()
                {
                    *dir_rad = deg.to_radians();
                    edited = true;
                }
            }
            BubbleShape::Soft {
                half_w,
                half_h,
                corner_px,
                shape_seed,
            } => {
                if !auto {
                    edited |= ui
                        .add(egui::Slider::new(half_w, 20.0..=800.0).text("半幅"))
                        .changed();
                    edited |= ui
                        .add(egui::Slider::new(half_h, 20.0..=800.0).text("半高"))
                        .changed();
                }
                edited |= ui
                    .add(egui::Slider::new(corner_px, 0.0..=200.0).text("角丸"))
                    .changed();
                ui.horizontal(|ui| {
                    ui.label(format!("seed {shape_seed}"));
                    if ui.button("再生成").clicked() {
                        *shape_seed = shape_seed.wrapping_add(1);
                        edited = true;
                    }
                });
            }
            BubbleShape::MotionLines {
                rx,
                ry,
                count,
                shape_seed,
            } => {
                if !auto {
                    edited |= ui
                        .add(egui::Slider::new(rx, 40.0..=1000.0).text("外半径rx"))
                        .changed();
                    edited |= ui
                        .add(egui::Slider::new(ry, 40.0..=1000.0).text("外半径ry"))
                        .changed();
                }
                edited |= ui
                    .add(egui::Slider::new(count, 8..=200).text("線の本数"))
                    .changed();
                ui.horizontal(|ui| {
                    ui.label(format!("seed {shape_seed}"));
                    if ui.button("再生成").clicked() {
                        *shape_seed = shape_seed.wrapping_add(1);
                        edited = true;
                    }
                });
            }
            BubbleShape::SpeedLines {
                half_w,
                half_h,
                dir_rad,
                count,
                shape_seed,
            } => {
                if !auto {
                    edited |= ui
                        .add(egui::Slider::new(half_w, 40.0..=1000.0).text("半幅"))
                        .changed();
                    edited |= ui
                        .add(egui::Slider::new(half_h, 40.0..=1000.0).text("半高"))
                        .changed();
                }
                edited |= ui
                    .add(egui::Slider::new(count, 8..=200).text("線の本数"))
                    .changed();
                let mut deg = dir_rad.to_degrees();
                if ui
                    .add(egui::Slider::new(&mut deg, -180.0..=180.0).text("向き(度)"))
                    .changed()
                {
                    *dir_rad = deg.to_radians();
                    edited = true;
                }
                ui.horizontal(|ui| {
                    ui.label(format!("seed {shape_seed}"));
                    if ui.button("再生成").clicked() {
                        *shape_seed = shape_seed.wrapping_add(1);
                        edited = true;
                    }
                });
            }
            BubbleShape::TextOnly { half_w, half_h } => {
                if !auto {
                    edited |= ui
                        .add(egui::Slider::new(half_w, 20.0..=800.0).text("半幅"))
                        .changed();
                    edited |= ui
                        .add(egui::Slider::new(half_h, 20.0..=800.0).text("半高"))
                        .changed();
                }
                ui.label(
                    egui::RichText::new("枠なし・テキストのみ (塗り/枠は描画されません)")
                        .small()
                        .weak(),
                );
            }
            BubbleShape::Concentration { rx, ry, shape_seed } => {
                if !auto {
                    edited |= ui
                        .add(egui::Slider::new(rx, 20.0..=800.0).text("rx"))
                        .changed();
                    edited |= ui
                        .add(egui::Slider::new(ry, 20.0..=800.0).text("ry"))
                        .changed();
                }
                ui.horizontal(|ui| {
                    ui.label(format!("seed {shape_seed}"));
                    if ui.button("再生成").clicked() {
                        *shape_seed = shape_seed.wrapping_add(1);
                        edited = true;
                    }
                });
            }
            BubbleShape::Strokes {
                half_w,
                half_h,
                corner_px,
                shape_seed,
            } => {
                if !auto {
                    edited |= ui
                        .add(egui::Slider::new(half_w, 20.0..=800.0).text("半幅"))
                        .changed();
                    edited |= ui
                        .add(egui::Slider::new(half_h, 20.0..=800.0).text("半高"))
                        .changed();
                }
                edited |= ui
                    .add(egui::Slider::new(corner_px, 0.0..=200.0).text("角丸"))
                    .changed();
                ui.horizontal(|ui| {
                    ui.label(format!("seed {shape_seed}"));
                    if ui.button("再生成").clicked() {
                        *shape_seed = shape_seed.wrapping_add(1);
                        edited = true;
                    }
                });
            }
            BubbleShape::DoubleStroke {
                half_w,
                half_h,
                corner_px,
                gap_px,
            } => {
                if !auto {
                    edited |= ui
                        .add(egui::Slider::new(half_w, 20.0..=800.0).text("半幅"))
                        .changed();
                    edited |= ui
                        .add(egui::Slider::new(half_h, 20.0..=800.0).text("半高"))
                        .changed();
                }
                edited |= ui
                    .add(egui::Slider::new(corner_px, 0.0..=200.0).text("角丸"))
                    .changed();
                edited |= ui
                    .add(egui::Slider::new(gap_px, 2.0..=40.0).text("線の間隔"))
                    .changed();
            }
        }
        if auto {
            ui.label(
                egui::RichText::new("サイズは文字量に合わせて自動調整 (オフで手動)")
                    .small()
                    .weak(),
            );
        }

        ui.add_space(2.0);
        ui.label(egui::RichText::new("塗り / 枠").strong());
        let mut has_fill = b.fill.is_some();
        if ui.checkbox(&mut has_fill, "塗り").changed() {
            b.fill = if has_fill { Some(Rgba::WHITE) } else { None };
            edited = true;
        }
        if let Some(fill) = &mut b.fill {
            let mut c = rgba_to_color32(*fill);
            if ui.color_edit_button_srgba(&mut c).changed() {
                *fill = color32_to_rgba(c);
                edited = true;
            }
            edited |= ui
                .add(egui::Slider::new(&mut b.fill_opacity, 0.0..=1.0).text("塗り不透明度"))
                .changed();
        }
        ui.horizontal(|ui| {
            ui.label("枠線色");
            let mut oc = rgba_to_color32(b.outline.color);
            if ui.color_edit_button_srgba(&mut oc).changed() {
                b.outline.color = color32_to_rgba(oc);
                edited = true;
            }
        });
        edited |= ui
            .add(egui::Slider::new(&mut b.outline.width_px, 0.0..=20.0).text("枠線太さ"))
            .changed();
        edited |= ui
            .add(egui::Slider::new(&mut b.padding_px, 0.0..=80.0).text("内側余白"))
            .changed();

        if edited {
            b.shape_preset_link = None;
            *dirty = true;
        }
    }

    /// しっぽ tail: kind / tip / auto-base / width. (表示 on/off is a toggle
    /// above the tabs.) Individual edits clear `b.shape_preset_link` (tail kind
    /// is part of the shape preset). Only drawn when the tail exists (tab gated).
    fn tab_tail(&mut self, ui: &mut egui::Ui, sel: u64, dirty: &mut bool) {
        let obj = self.objects.iter_mut().find(|o| o.id == sel).unwrap();
        let AnnotationKind::Bubble(b) = &mut obj.kind else {
            return;
        };

        // Individual tail edits break the shape-preset link (the tail KIND is
        // captured by the shape preset; tip/base/width are not, but the break is
        // kept uniform with the previous single-section behavior).
        let mut edited = false;

        if let Some(tail) = &mut b.tail {
            ui.horizontal(|ui| {
                ui.label("形式");
                if ui
                    .radio(matches!(tail.kind, TailKind::Spike), "三角")
                    .clicked()
                {
                    tail.kind = TailKind::Spike;
                    edited = true;
                }
                if ui
                    .radio(matches!(tail.kind, TailKind::Thought), "思考(丸)")
                    .clicked()
                {
                    tail.kind = TailKind::Thought;
                    edited = true;
                }
            });
            ui.horizontal(|ui| {
                ui.label("先端");
                edited |= ui
                    .add(egui::DragValue::new(&mut tail.tip.0).speed(1.0))
                    .changed();
                edited |= ui
                    .add(egui::DragValue::new(&mut tail.tip.1).speed(1.0))
                    .changed();
            });
            // Auto base: the root attaches where the center→tip ray exits the
            // outline (points at the speaker). Off → manual `base_t` slider /
            // drag the base handle. Both keep the tip where it is.
            edited |= ui
                .checkbox(&mut tail.base_auto, "付け根を自動 (対象方向)")
                .changed();
            if !tail.base_auto {
                edited |= ui
                    .add(egui::Slider::new(&mut tail.base_t, 0.0..=1.0).text("付け根位置"))
                    .changed();
            }
            let w_label = if matches!(tail.kind, TailKind::Thought) {
                "円の大きさ"
            } else {
                "付け根の太さ"
            };
            edited |= ui
                .add(egui::Slider::new(&mut tail.width_px, 4.0..=200.0).text(w_label))
                .changed();
        }

        // Tail edits break the shape-preset link (the tail kind is part of it).
        if edited {
            b.shape_preset_link = None;
            *dirty = true;
        }
    }

    /// 飾り decorations: procedural sparkle / flower / bubble layers placed
    /// along the outline. Baked the same as the body (live=baked). Not captured
    /// by shape presets, so edits only mark `dirty` (no link break).
    fn tab_deco(&mut self, ui: &mut egui::Ui, sel: u64, dirty: &mut bool) {
        let obj = self.objects.iter_mut().find(|o| o.id == sel).unwrap();
        let AnnotationKind::Bubble(b) = &mut obj.kind else {
            return;
        };

        if ui.button("装飾を追加").clicked() {
            let mut layer = comic_core::DecorationLayer::default();
            // Distinct seed per layer: `place_decorations` is deterministic in
            // the seed, so identical default seeds make every stacked layer land
            // on the SAME positions (looked like only one was rendered). Use
            // max(existing)+1 so a freshly-added layer never collides.
            layer.seed = b
                .decorations
                .iter()
                .map(|l| l.seed)
                .max()
                .map(|m| m.wrapping_add(1))
                .unwrap_or(0);
            b.decorations.push(layer);
            *dirty = true;
        }
        let mut remove_deco: Option<usize> = None;
        for (di, layer) in b.decorations.iter_mut().enumerate() {
            ui.separator();
            ui.horizontal(|ui| {
                ui.label(format!("装飾 {}", di + 1));
                if ui.small_button("×").clicked() {
                    remove_deco = Some(di);
                }
            });
            ui.horizontal(|ui| {
                ui.label("種類");
                for (label, kind) in [
                    ("きらきら", comic_core::DecoKind::Sparkle),
                    ("花", comic_core::DecoKind::Flower),
                    ("泡", comic_core::DecoKind::Bubble),
                ] {
                    if ui.radio(layer.kind == kind, label).clicked() {
                        layer.kind = kind;
                        *dirty = true;
                    }
                }
            });
            ui.horizontal(|ui| {
                ui.label("配置");
                for (label, pl) in [
                    ("輪郭上", comic_core::DecoPlacement::Outline),
                    ("外側", comic_core::DecoPlacement::Outside),
                    ("内側", comic_core::DecoPlacement::Inside),
                    ("しっぽ", comic_core::DecoPlacement::Tail),
                ] {
                    if ui.radio(layer.placement == pl, label).clicked() {
                        layer.placement = pl;
                        *dirty = true;
                    }
                }
            });
            *dirty |= ui
                .add(egui::Slider::new(&mut layer.density, 0.5..=12.0).text("密度"))
                .changed();
            *dirty |= ui
                .add(egui::Slider::new(&mut layer.size_ratio, 0.04..=0.6).text("大きさ"))
                .changed();
            ui.horizontal(|ui| {
                ui.label("色");
                let mut c = rgba_to_color32(layer.color);
                if ui.color_edit_button_srgba(&mut c).changed() {
                    layer.color = color32_to_rgba(c);
                    *dirty = true;
                }
            });

            // Outline (0 px = none) + outline color.
            *dirty |= ui
                .add(egui::Slider::new(&mut layer.outline_width, 0.0..=10.0).text("縁取り太さ"))
                .changed();
            if layer.outline_width > 0.0 {
                ui.horizontal(|ui| {
                    ui.label("縁取り色");
                    let mut c = rgba_to_color32(layer.outline_color);
                    if ui.color_edit_button_srgba(&mut c).changed() {
                        layer.outline_color = color32_to_rgba(c);
                        *dirty = true;
                    }
                });
            }

            // Kind-specific shape controls.
            match layer.kind {
                comic_core::DecoKind::Sparkle => {
                    *dirty |= ui
                        .add(egui::Slider::new(&mut layer.points, 3..=12).text("とがり数"))
                        .changed();
                }
                comic_core::DecoKind::Flower => {
                    *dirty |= ui
                        .add(egui::Slider::new(&mut layer.petals, 3..=10).text("花びら数"))
                        .changed();
                    ui.horizontal(|ui| {
                        ui.label("中央色");
                        let mut c = rgba_to_color32(layer.center_color);
                        if ui.color_edit_button_srgba(&mut c).changed() {
                            layer.center_color = color32_to_rgba(c);
                            *dirty = true;
                        }
                    });
                }
                comic_core::DecoKind::Bubble => {
                    *dirty |= ui
                        .checkbox(&mut layer.gradient, "半透明グラデ (泡)")
                        .changed();
                }
            }

            ui.horizontal(|ui| {
                ui.label(format!("seed {}", layer.seed));
                if ui.button("再生成").clicked() {
                    layer.seed = layer.seed.wrapping_add(1);
                    *dirty = true;
                }
            });
        }
        if let Some(di) = remove_deco {
            b.decorations.remove(di);
            *dirty = true;
        }
    }

    fn draw_canvas(&mut self, ctx: &egui::Context) {
        egui::CentralPanel::default().show(ctx, |ui| {
            let avail = ui.available_rect_before_wrap();
            let (resp, painter) = ui.allocate_painter(avail.size(), Sense::click_and_drag());
            let canvas_rect = resp.rect;
            painter.rect_filled(canvas_rect, 0.0, Color32::from_rgb(28, 28, 32));

            let Some((img_w, img_h)) = self.image.as_ref().map(|i| (i.width, i.height)) else {
                let hovering = ctx.input(|i| !i.raw.hovered_files.is_empty());
                let (msg, col) = if hovering {
                    ("ドロップして画像を開く", Color32::WHITE)
                } else {
                    (
                        "画像をドロップ、またはメニュー「読み込み」から開く",
                        Color32::GRAY,
                    )
                };
                painter.text(
                    canvas_rect.center(),
                    egui::Align2::CENTER_CENTER,
                    msg,
                    egui::FontId::proportional(20.0),
                    col,
                );
                return;
            };

            // Initialize fit-to-window once.
            if !self.view_initialized {
                let zx = canvas_rect.width() / img_w as f32;
                let zy = canvas_rect.height() / img_h as f32;
                let zoom = zx.min(zy).clamp(ZOOM_MIN, ZOOM_MAX);
                let img_screen_w = img_w as f32 * zoom;
                let img_screen_h = img_h as f32 * zoom;
                self.view.zoom = zoom;
                self.view.offset = canvas_rect.min.to_vec2()
                    + egui::vec2(
                        (canvas_rect.width() - img_screen_w) * 0.5,
                        (canvas_rect.height() - img_screen_h) * 0.5,
                    );
                self.view_initialized = true;
            }

            // Scroll-to-zoom around the pointer.
            let scroll = ctx.input(|i| i.raw_scroll_delta.y);
            if scroll.abs() > 0.0 && resp.hovered() {
                if let Some(ptr) = ctx.input(|i| i.pointer.hover_pos()) {
                    let before = self.view.screen_to_img(ptr);
                    let factor = (1.0 + scroll * 0.001).clamp(0.5, 2.0);
                    self.view.zoom = (self.view.zoom * factor).clamp(ZOOM_MIN, ZOOM_MAX);
                    let after = self.view.img_to_screen(before);
                    self.view.offset += ptr - after;
                }
            }

            // Handle interaction BEFORE baking so the overlay reflects this
            // frame's drag/pan (the handles are drawn from the same state, so
            // overlay and handles stay in sync — no one-frame lag).
            self.handle_canvas_input(ctx, &resp);

            // Draw background image.
            if let Some(tex) = &self.source_texture {
                let min = self.view.img_to_screen((0.0, 0.0));
                let max = self.view.img_to_screen((img_w as f32, img_h as f32));
                painter.image(
                    tex.id(),
                    Rect::from_min_max(min, max),
                    Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)),
                    Color32::WHITE,
                );
            }

            // Always show the WYSIWYG baked overlay (same pixels the export
            // would produce; the live egui text path can't match ab_glyph
            // positions). Throttle re-baking during a drag so large images
            // don't allocate + upload a full-resolution texture every frame.
            let now = ctx.input(|i| i.time);
            let dragging = self.drag != DragKind::None;
            // The top-z run of stamps is drawn as GPU textured quads and kept OUT of
            // the CPU bake entirely (scale/rotate/flip/opacity are ~free on the GPU).
            // Outlined stamps qualify too: their image+halo is pre-composited once
            // into a "sticker" texture (cached per source+outline+size) and reused, so
            // N duplicates cost one halo dilation instead of N every bake. The bake
            // then never rasterizes a stamp, so it's cheap no matter how many large/
            // rotated/outlined stamps exist, and a stamp's on-screen position never
            // changes between "dragging" and "idle" (no CPU↔GPU handoff, no drag-end
            // shift).
            let gpu_ids = self.gpu_stamp_ids(ctx);
            // The excluded SET changing (stamp added/removed, outline toggled, z
            // reordered) means the bake content changed → must re-bake.
            let set_changed = gpu_ids != self.baked_excluded_set;
            if set_changed {
                self.baked_dirty = true;
            }
            // While dragging one of the GPU stamps, the CPU bake (which excludes all
            // of them) is static — only the dragged quad moves — so skip re-baking
            // (this was the periodic stutter: re-rasterizing/re-uploading every
            // tick). Dragging a non-stamp (it IS in the bake and moving) re-bakes.
            let dragging_gpu_stamp = dragging && self.selected.is_some_and(|id| gpu_ids.contains(&id));
            let suppress = dragging_gpu_stamp && !set_changed;
            // During a drag, adapt the re-bake interval to the last bake's cost so
            // a heavy scene doesn't saturate the UI thread with back-to-back bakes.
            let min_interval = (self.last_bake_dur * 1.5).clamp(0.03, 0.25);
            if self.baked_dirty
                && !suppress
                && (!dragging || now - self.last_bake_time >= min_interval)
            {
                let t0 = std::time::Instant::now();
                self.rebake(ctx, &gpu_ids);
                self.last_bake_dur = t0.elapsed().as_secs_f64();
                self.last_bake_time = now;
            }
            if let Some(tex) = &self.baked_texture {
                let min = self.view.img_to_screen((0.0, 0.0));
                let max = self.view.img_to_screen((img_w as f32, img_h as f32));
                painter.image(
                    tex.id(),
                    Rect::from_min_max(min, max),
                    Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)),
                    Color32::WHITE,
                );
            }
            // GPU stamps (the top-z run) drawn on top of the bake, ascending z.
            // Lower stamps (below a bubble/text) stayed in the bake at true z.
            for &id in &gpu_ids {
                self.draw_stamp_preview(ctx, &painter, id);
            }

            // Selection decorations (handles) on top of the baked overlay.
            self.draw_selection_handles(&painter);

            // Drag-and-drop hint overlay (when files are dragged over).
            if ctx.input(|i| !i.raw.hovered_files.is_empty()) {
                painter.rect_filled(
                    canvas_rect,
                    0.0,
                    Color32::from_rgba_unmultiplied(20, 40, 70, 160),
                );
                painter.text(
                    canvas_rect.center(),
                    egui::Align2::CENTER_CENTER,
                    "ドロップして画像を開く",
                    egui::FontId::proportional(28.0),
                    Color32::WHITE,
                );
            }

            // Perf HUD (toggle with F1): last bake timings + object counts, to find
            // what's slow during interaction.
            if ctx.input(|i| i.key_pressed(egui::Key::F1)) {
                self.show_perf_hud = !self.show_perf_hud;
            }
            if self.show_perf_hud {
                let stamps = self
                    .objects
                    .iter()
                    .filter(|o| matches!(o.kind, AnnotationKind::Stamp(_)))
                    .count();
                let hud = format!(
                    "bake {:.1}ms (composite {:.1} + upload {:.1}) | img {}x{} | obj {} stamp {} | drag {}",
                    self.last_bake_dur * 1000.0,
                    self.last_composite_ms,
                    self.last_upload_ms,
                    img_w,
                    img_h,
                    self.objects.len(),
                    stamps,
                    if dragging { "yes" } else { "no" },
                );
                let pos = canvas_rect.left_top() + egui::vec2(8.0, 8.0);
                // Shadowed text for legibility over any background.
                painter.text(
                    pos + egui::vec2(1.0, 1.0),
                    egui::Align2::LEFT_TOP,
                    &hud,
                    egui::FontId::monospace(12.0),
                    Color32::from_black_alpha(200),
                );
                painter.text(
                    pos,
                    egui::Align2::LEFT_TOP,
                    &hud,
                    egui::FontId::monospace(12.0),
                    Color32::from_rgb(120, 230, 140),
                );
            }
        });

        if self.has_pending_repaint() {
            ctx.request_repaint();
        }
    }

    fn has_pending_repaint(&self) -> bool {
        // Repaint while dragging, and once more when a throttled bake is still
        // pending so the final result lands after the drag stops.
        self.drag != DragKind::None || self.baked_dirty
    }

    /// Corner points (TL,TR,BR,BL) + the rotation handle for a bubble, all in
    /// IMAGE space and honoring `obj.rotation_rad`. The rotation handle sits a
    /// roughly screen-constant distance above the top edge. None for non-bubbles.
    fn bubble_handle_points(
        &self,
        obj: &AnnotationObject,
    ) -> Option<([(f32, f32); 4], (f32, f32))> {
        let AnnotationKind::Bubble(b) = &obj.kind else {
            return None;
        };
        let (hw, hh) = match effective_bubble_shape(b, &self.fonts) {
            BubbleShape::Ellipse { rx, ry, .. } => (rx, ry),
            BubbleShape::RoundRect { half_w, half_h, .. } => (half_w, half_h),
            BubbleShape::Burst { rx, ry, .. } => (rx, ry),
            BubbleShape::Cloud { rx, ry, .. } => (rx, ry),
            BubbleShape::Polygon { rx, ry, .. } => (rx, ry),
            BubbleShape::Diamond { half_w, half_h } => (half_w, half_h),
            BubbleShape::Heart { rx, ry } => (rx, ry),
            BubbleShape::Arrow { half_w, half_h, .. } => (half_w, half_h),
            BubbleShape::Soft { half_w, half_h, .. } => (half_w, half_h),
            BubbleShape::MotionLines { rx, ry, .. } => (rx, ry),
            BubbleShape::SpeedLines { half_w, half_h, .. } => (half_w, half_h),
            BubbleShape::TextOnly { half_w, half_h } => (half_w, half_h),
            BubbleShape::Concentration { rx, ry, .. } => (rx, ry),
            BubbleShape::Strokes { half_w, half_h, .. } => (half_w, half_h),
            BubbleShape::DoubleStroke { half_w, half_h, .. } => (half_w, half_h),
        };
        let (sin, cos) = obj.rotation_rad.sin_cos();
        let p = obj.pivot;
        let rot = |lx: f32, ly: f32| (p.0 + lx * cos - ly * sin, p.1 + lx * sin + ly * cos);
        let corners = [rot(-hw, -hh), rot(hw, -hh), rot(hw, hh), rot(-hw, hh)];
        // ~28px above the top edge regardless of zoom.
        let offset_img = 28.0 / self.view.zoom.max(1e-3);
        let rot_handle = rot(0.0, -(hh + offset_img));
        Some((corners, rot_handle))
    }

    /// Corner points (TL,TR,BR,BL) + rotation handle for a message window.
    fn window_handle_points(
        &self,
        obj: &AnnotationObject,
    ) -> Option<([(f32, f32); 4], (f32, f32))> {
        let AnnotationKind::MessageWindow(w) = &obj.kind else {
            return None;
        };
        let (hw, hh) = effective_window_half_extents(w, &self.fonts);
        let (sin, cos) = obj.rotation_rad.sin_cos();
        let p = obj.pivot;
        let rot = |lx: f32, ly: f32| (p.0 + lx * cos - ly * sin, p.1 + lx * sin + ly * cos);
        let corners = [rot(-hw, -hh), rot(hw, -hh), rot(hw, hh), rot(-hw, hh)];
        let offset_img = 28.0 / self.view.zoom.max(1e-3);
        let rot_handle = rot(0.0, -(hh + offset_img));
        Some((corners, rot_handle))
    }

    /// Corner points (TL,TR,BR,BL) + rotation handle for a stamp.
    fn stamp_handle_points(&self, obj: &AnnotationObject) -> Option<([(f32, f32); 4], (f32, f32))> {
        let AnnotationKind::Stamp(s) = &obj.kind else {
            return None;
        };
        let (hw, hh) = (s.half_w, s.half_h);
        let (sin, cos) = obj.rotation_rad.sin_cos();
        let p = obj.pivot;
        let rot = |lx: f32, ly: f32| (p.0 + lx * cos - ly * sin, p.1 + lx * sin + ly * cos);
        let corners = [rot(-hw, -hh), rot(hw, -hh), rot(hw, hh), rot(-hw, hh)];
        let offset_img = 28.0 / self.view.zoom.max(1e-3);
        let rot_handle = rot(0.0, -(hh + offset_img));
        Some((corners, rot_handle))
    }

    /// Corner points (TL,TR,BR,BL) + rotation handle for standalone text.
    /// Text stores its pivot as the layout top-left, but its edit handles rotate
    /// around the text rectangle center to match the baked result.
    fn text_handle_points(&self, obj: &AnnotationObject) -> Option<([(f32, f32); 4], (f32, f32))> {
        let AnnotationKind::Text(t) = &obj.kind else {
            return None;
        };
        let (w, h) = self.text_layout_size(t);
        let (hw, hh) = (w * 0.5, h * 0.5);
        let p = (obj.pivot.0 + hw, obj.pivot.1 + hh);
        let (sin, cos) = obj.rotation_rad.sin_cos();
        let rot = |lx: f32, ly: f32| (p.0 + lx * cos - ly * sin, p.1 + lx * sin + ly * cos);
        let corners = [rot(-hw, -hh), rot(hw, -hh), rot(hw, hh), rot(-hw, hh)];
        let offset_img = 28.0 / self.view.zoom.max(1e-3);
        let rot_handle = rot(0.0, -(hh + offset_img));
        Some((corners, rot_handle))
    }

    /// Corner + rotation handles for whichever kind supports them.
    fn handle_points(&self, obj: &AnnotationObject) -> Option<([(f32, f32); 4], (f32, f32))> {
        match &obj.kind {
            AnnotationKind::Bubble(_) => self.bubble_handle_points(obj),
            AnnotationKind::MessageWindow(_) => self.window_handle_points(obj),
            AnnotationKind::Stamp(_) => self.stamp_handle_points(obj),
            AnnotationKind::Text(_) => self.text_handle_points(obj),
        }
    }

    fn draw_selection_handles(&self, painter: &egui::Painter) {
        let Some(sel) = self.selected else {
            return;
        };
        let Some(obj) = self.objects.iter().find(|o| o.id == sel) else {
            return;
        };
        let blue = Color32::from_rgb(90, 170, 255);
        match &obj.kind {
            AnnotationKind::Bubble(b) => {
                if let Some((corners, roth)) = self.bubble_handle_points(obj) {
                    let cs: Vec<Pos2> = corners
                        .iter()
                        .map(|c| self.view.img_to_screen(*c))
                        .collect();
                    // Rotated bounding quad.
                    for i in 0..4 {
                        painter
                            .line_segment([cs[i], cs[(i + 1) % 4]], egui::Stroke::new(1.5, blue));
                    }
                    // Rotation handle: stem from top-edge midpoint + a green knob.
                    let top_mid = egui::pos2((cs[0].x + cs[1].x) * 0.5, (cs[0].y + cs[1].y) * 0.5);
                    let roth_s = self.view.img_to_screen(roth);
                    painter.line_segment([top_mid, roth_s], egui::Stroke::new(1.5, blue));
                    painter.circle_filled(roth_s, HANDLE_R, Color32::from_rgb(120, 220, 120));
                    painter.circle_stroke(roth_s, HANDLE_R, egui::Stroke::new(1.5, Color32::BLACK));
                    // Corner resize handles (small squares).
                    for c in &cs {
                        let r =
                            Rect::from_center_size(*c, egui::vec2(HANDLE_R * 1.8, HANDLE_R * 1.8));
                        painter.rect_filled(r, 1.0, Color32::from_rgb(230, 230, 235));
                        painter.rect_stroke(
                            r,
                            1.0,
                            egui::Stroke::new(1.5, Color32::BLACK),
                            egui::StrokeKind::Outside,
                        );
                    }
                    // Tail handles: cyan base (on the outline) + orange tip.
                    // The whole object bakes rotated, so rotate the handle
                    // positions to match the visible (rotated) tail. Skip for
                    // tailless shapes (the tail isn't drawn, so no handles).
                    if let Some(tail) = b.tail.as_ref().filter(|_| shape_renders_tail(&b.shape)) {
                        let eff = effective_bubble_shape(b, &self.fonts);
                        let rot = obj.rotation_rad;
                        let base =
                            rotate_about(resolve_tail_base(&eff, obj.pivot, tail), obj.pivot, rot);
                        let bp = self.view.img_to_screen(base);
                        painter.circle_filled(bp, HANDLE_R, Color32::from_rgb(80, 200, 220));
                        painter.circle_stroke(bp, HANDLE_R, egui::Stroke::new(1.5, Color32::BLACK));
                        // Handle follows the drawn tip (kept outside the auto-sized
                        // outline), so it tracks the visible spike after auto-grow.
                        let tip =
                            rotate_about(resolve_tail_tip(&eff, obj.pivot, tail), obj.pivot, rot);
                        let p = self.view.img_to_screen(tip);
                        painter.circle_filled(p, HANDLE_R, Color32::from_rgb(255, 160, 60));
                        painter.circle_stroke(p, HANDLE_R, egui::Stroke::new(1.5, Color32::BLACK));
                    }
                }
            }
            AnnotationKind::Text(_) => {
                // Standalone text / generated onomatopoeia: same rotated quad,
                // resize corners and rotation knob as stamps.
                if let Some((corners, roth)) = self.text_handle_points(obj) {
                    let cs: Vec<Pos2> = corners
                        .iter()
                        .map(|c| self.view.img_to_screen(*c))
                        .collect();
                    for i in 0..4 {
                        painter
                            .line_segment([cs[i], cs[(i + 1) % 4]], egui::Stroke::new(1.5, blue));
                    }
                    let top_mid = egui::pos2((cs[0].x + cs[1].x) * 0.5, (cs[0].y + cs[1].y) * 0.5);
                    let roth_s = self.view.img_to_screen(roth);
                    painter.line_segment([top_mid, roth_s], egui::Stroke::new(1.5, blue));
                    painter.circle_filled(roth_s, HANDLE_R, Color32::from_rgb(120, 220, 120));
                    painter.circle_stroke(roth_s, HANDLE_R, egui::Stroke::new(1.5, Color32::BLACK));
                    for c in &cs {
                        let r =
                            Rect::from_center_size(*c, egui::vec2(HANDLE_R * 1.8, HANDLE_R * 1.8));
                        painter.rect_filled(r, 1.0, Color32::from_rgb(230, 230, 235));
                        painter.rect_stroke(
                            r,
                            1.0,
                            egui::Stroke::new(1.5, Color32::BLACK),
                            egui::StrokeKind::Outside,
                        );
                    }
                }
            }
            AnnotationKind::MessageWindow(_) => {
                // Same rotated quad + rotate knob + corner squares as a bubble
                // (no tail). Corner drag resizes (switches to Inset size mode).
                if let Some((corners, roth)) = self.window_handle_points(obj) {
                    let cs: Vec<Pos2> = corners
                        .iter()
                        .map(|c| self.view.img_to_screen(*c))
                        .collect();
                    for i in 0..4 {
                        painter
                            .line_segment([cs[i], cs[(i + 1) % 4]], egui::Stroke::new(1.5, blue));
                    }
                    let top_mid = egui::pos2((cs[0].x + cs[1].x) * 0.5, (cs[0].y + cs[1].y) * 0.5);
                    let roth_s = self.view.img_to_screen(roth);
                    painter.line_segment([top_mid, roth_s], egui::Stroke::new(1.5, blue));
                    painter.circle_filled(roth_s, HANDLE_R, Color32::from_rgb(120, 220, 120));
                    painter.circle_stroke(roth_s, HANDLE_R, egui::Stroke::new(1.5, Color32::BLACK));
                    for c in &cs {
                        let r =
                            Rect::from_center_size(*c, egui::vec2(HANDLE_R * 1.8, HANDLE_R * 1.8));
                        painter.rect_filled(r, 1.0, Color32::from_rgb(230, 230, 235));
                        painter.rect_stroke(
                            r,
                            1.0,
                            egui::Stroke::new(1.5, Color32::BLACK),
                            egui::StrokeKind::Outside,
                        );
                    }
                }
            }
            AnnotationKind::Stamp(_) => {
                // Rotated quad + rotate knob + corner squares (corner drag does a
                // uniform, aspect-preserving scale).
                if let Some((corners, roth)) = self.stamp_handle_points(obj) {
                    let cs: Vec<Pos2> = corners
                        .iter()
                        .map(|c| self.view.img_to_screen(*c))
                        .collect();
                    for i in 0..4 {
                        painter
                            .line_segment([cs[i], cs[(i + 1) % 4]], egui::Stroke::new(1.5, blue));
                    }
                    let top_mid = egui::pos2((cs[0].x + cs[1].x) * 0.5, (cs[0].y + cs[1].y) * 0.5);
                    let roth_s = self.view.img_to_screen(roth);
                    painter.line_segment([top_mid, roth_s], egui::Stroke::new(1.5, blue));
                    painter.circle_filled(roth_s, HANDLE_R, Color32::from_rgb(120, 220, 120));
                    painter.circle_stroke(roth_s, HANDLE_R, egui::Stroke::new(1.5, Color32::BLACK));
                    for c in &cs {
                        let r =
                            Rect::from_center_size(*c, egui::vec2(HANDLE_R * 1.8, HANDLE_R * 1.8));
                        painter.rect_filled(r, 1.0, Color32::from_rgb(230, 230, 235));
                        painter.rect_stroke(
                            r,
                            1.0,
                            egui::Stroke::new(1.5, Color32::BLACK),
                            egui::StrokeKind::Outside,
                        );
                    }
                }
            }
        }
    }

    fn handle_canvas_input(&mut self, ctx: &egui::Context, resp: &egui::Response) {
        let pointer = ctx.input(|i| i.pointer.hover_pos());

        // Begin drag.
        if resp.drag_started() {
            if let Some(ptr) = pointer {
                let img_pt = self.view.screen_to_img(ptr);
                // Tail handles of the selected bubble take priority (tip, then
                // base). Both are small circles drawn over the canvas.
                let mut started = false;
                if let Some(sel) = self.selected {
                    if let Some(obj) = self.objects.iter().find(|o| o.id == sel) {
                        if let AnnotationKind::Bubble(b) = &obj.kind {
                            // Tailless shapes draw no tail → no tail handles to grab.
                            if let Some(tail) =
                                b.tail.as_ref().filter(|_| shape_renders_tail(&b.shape))
                            {
                                let rot = obj.rotation_rad;
                                let eff = effective_bubble_shape(b, &self.fonts);
                                // Grab the drawn tip (kept outside the auto-sized
                                // outline), matching the visible spike handle.
                                let tip = rotate_about(
                                    resolve_tail_tip(&eff, obj.pivot, tail),
                                    obj.pivot,
                                    rot,
                                );
                                let tip_screen = self.view.img_to_screen(tip);
                                if (tip_screen - ptr).length() <= HANDLE_R + 4.0 {
                                    self.drag = DragKind::TailTip;
                                    started = true;
                                } else {
                                    let base = rotate_about(
                                        resolve_tail_base(&eff, obj.pivot, tail),
                                        obj.pivot,
                                        rot,
                                    );
                                    let base_screen = self.view.img_to_screen(base);
                                    if (base_screen - ptr).length() <= HANDLE_R + 4.0 {
                                        self.drag = DragKind::TailBase;
                                        started = true;
                                    }
                                }
                            }
                        }
                    }
                }
                if !started {
                    // Rotation handle + corner resize handles of the selected
                    // bubble (computed as owned points so no borrow lingers).
                    let handles = self
                        .selected
                        .and_then(|sel| self.objects.iter().find(|o| o.id == sel))
                        .and_then(|obj| self.handle_points(obj));
                    if let Some((corners, roth)) = handles {
                        let roth_s = self.view.img_to_screen(roth);
                        if (roth_s - ptr).length() <= HANDLE_R + 4.0 {
                            self.drag = DragKind::Rotate;
                            started = true;
                        } else {
                            for (i, c) in corners.iter().enumerate() {
                                let cs = self.view.img_to_screen(*c);
                                if (cs - ptr).length() <= HANDLE_R + 4.0 {
                                    self.drag = DragKind::Corner(i);
                                    started = true;
                                    break;
                                }
                            }
                        }
                    }
                }
                if !started {
                    // Hit-test top-most object under the cursor.
                    let hit = self.hit_test(img_pt);
                    if let Some(id) = hit {
                        self.selected = Some(id);
                        let pivot = self
                            .objects
                            .iter()
                            .find(|o| o.id == id)
                            .map(|o| o.pivot)
                            .unwrap_or((0.0, 0.0));
                        self.drag = DragKind::Move;
                        self.drag_img_anchor = img_pt;
                        self.drag_pivot_anchor = pivot;
                    } else {
                        // Empty area -> pan.
                        self.drag = DragKind::Pan;
                        self.drag_img_anchor = (ptr.x, ptr.y);
                    }
                }
            }
        }

        // Continue drag.
        if resp.dragged() {
            if let Some(ptr) = pointer {
                match self.drag {
                    DragKind::Move => {
                        let img_pt = self.view.screen_to_img(ptr);
                        let dx = img_pt.0 - self.drag_img_anchor.0;
                        let dy = img_pt.1 - self.drag_img_anchor.1;
                        let new_pivot =
                            (self.drag_pivot_anchor.0 + dx, self.drag_pivot_anchor.1 + dy);
                        let id = self.selected;
                        let mut d = (0.0f32, 0.0f32);
                        if let Some(obj) = self.selected_obj_mut() {
                            d = (new_pivot.0 - obj.pivot.0, new_pivot.1 - obj.pivot.1);
                            obj.pivot = new_pivot;
                            match &mut obj.kind {
                                // Translate the tail tip with the bubble.
                                AnnotationKind::Bubble(b) => {
                                    if let Some(t) = &mut b.tail {
                                        t.tip = (t.tip.0 + d.0, t.tip.1 + d.1);
                                    }
                                }
                                // Dragging a window = manual placement (don't let
                                // a position preset snap it back). Position is part
                                // of the window-style preset, so unlink it too.
                                AnnotationKind::MessageWindow(w) => {
                                    w.position = WindowPosition::Free;
                                    w.style_preset_link = None;
                                }
                                AnnotationKind::Text(_) | AnnotationKind::Stamp(_) => {}
                            }
                        }
                        // Keep a hidden (stashed) tail attached too, so showing it
                        // again after a move doesn't snap it back.
                        if let Some(id) = id {
                            if let Some(t) = self.tail_stash.get_mut(&id) {
                                t.tip = (t.tip.0 + d.0, t.tip.1 + d.1);
                            }
                        }
                        self.baked_dirty = true;
                    }
                    DragKind::TailTip => {
                        let img_pt = self.view.screen_to_img(ptr);
                        if let Some(obj) = self.selected_obj_mut() {
                            // Store the tip in the bubble's unrotated frame (bake
                            // re-applies the rotation).
                            let local = inv_rotate_about(img_pt, obj.pivot, obj.rotation_rad);
                            if let AnnotationKind::Bubble(b) = &mut obj.kind {
                                if let Some(tail) = &mut b.tail {
                                    tail.tip = local;
                                }
                            }
                        }
                        self.baked_dirty = true;
                    }
                    DragKind::TailBase => {
                        // Pin the base to the outline point nearest the cursor;
                        // dragging the base means the user wants manual control.
                        let img_pt = self.view.screen_to_img(ptr);
                        let sel = self.selected;
                        let fonts = &self.fonts;
                        if let Some(obj) = self.objects.iter_mut().find(|o| Some(o.id) == sel) {
                            let pivot = obj.pivot;
                            // Inverse-rotate into the bubble's unrotated frame.
                            let local = inv_rotate_about(img_pt, pivot, obj.rotation_rad);
                            if let AnnotationKind::Bubble(b) = &mut obj.kind {
                                if b.tail.is_some() {
                                    let eff = effective_bubble_shape(b, fonts);
                                    let t = nearest_base_t(&eff, pivot, local);
                                    if let Some(tail) = &mut b.tail {
                                        tail.base_auto = false;
                                        tail.base_t = t;
                                    }
                                }
                            }
                        }
                        self.baked_dirty = true;
                    }
                    DragKind::Corner(_) => {
                        // Resize symmetric about the pivot, measured along the
                        // object's local (rotated) axes. Text scales uniformly
                        // around its layout center; bubbles/windows turn auto
                        // sizing/position presets off as needed.
                        let img_pt = self.view.screen_to_img(ptr);
                        let text_resize = self
                            .selected
                            .and_then(|sel| self.objects.iter().find(|o| o.id == sel))
                            .and_then(|obj| {
                                if let AnnotationKind::Text(t) = &obj.kind {
                                    let (w, h) = self.text_layout_size(t);
                                    let center = (obj.pivot.0 + w * 0.5, obj.pivot.1 + h * 0.5);
                                    Some((w, h, center))
                                } else {
                                    None
                                }
                            });
                        let mut resized_text: Option<(u64, (f32, f32), TextBlock)> = None;
                        if let Some(obj) = self.selected_obj_mut() {
                            let pivot = obj.pivot;
                            let (sin, cos) = obj.rotation_rad.sin_cos();
                            let relx = img_pt.0 - pivot.0;
                            let rely = img_pt.1 - pivot.1;
                            // Inverse-rotate into local axes.
                            let lx = relx * cos + rely * sin;
                            let ly = -relx * sin + rely * cos;
                            match &mut obj.kind {
                                AnnotationKind::Bubble(b) => {
                                    set_bubble_half_extents(
                                        b,
                                        lx.abs().max(10.0),
                                        ly.abs().max(10.0),
                                    );
                                    b.auto_size = false;
                                    b.shape_preset_link = None;
                                }
                                AnnotationKind::MessageWindow(w) => {
                                    // Manual resize → switch to a fixed Inset size.
                                    w.half_w = lx.abs().max(20.0);
                                    w.half_h = ly.abs().max(12.0);
                                    w.size_mode = SizeMode::Inset;
                                    w.position = WindowPosition::Free;
                                    w.style_preset_link = None;
                                }
                                AnnotationKind::Stamp(s) => {
                                    // Uniform, aspect-preserving scale: grow the box
                                    // to contain the dragged corner while keeping the
                                    // source image's half_w:half_h ratio.
                                    let aspect = if s.half_h > 1e-3 {
                                        s.half_w / s.half_h
                                    } else {
                                        1.0
                                    };
                                    let cand_w = lx.abs().max(8.0);
                                    let cand_h = ly.abs().max(8.0);
                                    let new_w = cand_w.max(cand_h * aspect);
                                    s.half_w = new_w;
                                    s.half_h = (new_w / aspect.max(1e-3)).max(8.0);
                                }
                                AnnotationKind::Text(t) => {
                                    if let Some((w, h, center)) = text_resize {
                                        let local =
                                            inv_rotate_about(img_pt, center, obj.rotation_rad);
                                        let lx = local.0 - center.0;
                                        let ly = local.1 - center.1;
                                        let sx = lx.abs() / (w * 0.5).max(1.0);
                                        let sy = ly.abs() / (h * 0.5).max(1.0);
                                        let scale = sx.max(sy).clamp(0.12, 12.0);
                                        let new_size = (t.size_px * scale).clamp(6.0, 240.0);
                                        if (new_size - t.size_px).abs() > 0.01 {
                                            t.size_px = new_size;
                                            t.preset_link = None;
                                        }
                                        resized_text = Some((obj.id, center, t.clone()));
                                    }
                                }
                            }
                        }
                        if let Some((id, center, text)) = resized_text {
                            let (w, h) = self.text_layout_size(&text);
                            if let Some(obj) = self.objects.iter_mut().find(|o| o.id == id) {
                                obj.pivot = (center.0 - w * 0.5, center.1 - h * 0.5);
                            }
                        }
                        self.baked_dirty = true;
                    }
                    DragKind::Rotate => {
                        let img_pt = self.view.screen_to_img(ptr);
                        let center = self
                            .selected
                            .and_then(|sel| self.objects.iter().find(|o| o.id == sel))
                            .map(|obj| self.rotation_center(obj));
                        if let Some(obj) = self.selected_obj_mut() {
                            let center = center.unwrap_or(obj.pivot);
                            let relx = img_pt.0 - center.0;
                            let rely = img_pt.1 - center.1;
                            // Handle points "up" (local -Y) at rotation 0.
                            obj.rotation_rad = rely.atan2(relx) + std::f32::consts::FRAC_PI_2;
                        }
                        self.baked_dirty = true;
                    }
                    DragKind::Pan => {
                        let delta = egui::vec2(
                            ptr.x - self.drag_img_anchor.0,
                            ptr.y - self.drag_img_anchor.1,
                        );
                        self.view.offset += delta;
                        self.drag_img_anchor = (ptr.x, ptr.y);
                    }
                    DragKind::None => {}
                }
            }
        }

        // Click select (no drag).
        if resp.clicked() {
            if let Some(ptr) = pointer {
                let img_pt = self.view.screen_to_img(ptr);
                self.selected = self.hit_test(img_pt);
            }
        }

        if resp.drag_stopped() {
            self.drag = DragKind::None;
        }

        // Delete selected with Delete key — but NOT while editing text (the
        // TextEdit must keep Delete for character deletion).
        if !ctx.wants_keyboard_input() && ctx.input(|i| i.key_pressed(egui::Key::Delete)) {
            if let Some(id) = self.selected {
                self.objects.retain(|o| o.id != id);
                self.tail_stash.remove(&id);
                self.selected = None;
                self.baked_dirty = true;
            }
        }
    }

    /// Return the id of the top-most object whose bounds contain `img_pt`.
    fn hit_test(&self, img_pt: (f32, f32)) -> Option<u64> {
        let p = Pos2::new(img_pt.0, img_pt.1);
        let mut order: Vec<usize> = (0..self.objects.len())
            .filter(|&i| self.objects[i].enabled)
            .collect();
        // Top-most = highest z first.
        order.sort_by_key(|&i| std::cmp::Reverse(self.objects[i].z));
        for &i in &order {
            if self.object_bounds(&self.objects[i]).contains(p) {
                return Some(self.objects[i].id);
            }
        }
        None
    }
}

/// Insert `open`/`close` around the char range `[start,end)` of `text` (wraps a
/// selection, or inserts an empty pair at the cursor). Returns the new caret
/// char index (just after the wrapped content, before `close`).
fn insert_markers(text: &mut String, start: usize, end: usize, open: char, close: char) -> usize {
    let chars: Vec<char> = text.chars().collect();
    let n = chars.len();
    let start = start.min(n);
    let end = end.min(n).max(start);
    let mut out = String::with_capacity(text.len() + open.len_utf8() + close.len_utf8());
    out.extend(chars[..start].iter());
    out.push(open);
    out.extend(chars[start..end].iter());
    out.push(close);
    out.extend(chars[end..].iter());
    *text = out;
    start + 1 + (end - start)
}

/// Classify a font file by probing glyph coverage (runs on a background thread).
/// The parsed face is dropped immediately so we don't retain hundreds of fonts.
fn classify_font_file(path: &Path) -> FontScript {
    let Ok(bytes) = std::fs::read(path) else {
        return FontScript::Other;
    };
    let Ok(font) = LoadedFont::from_bytes(String::new(), bytes) else {
        return FontScript::Other;
    };
    // Japanese needs both kana and a common kanji; otherwise fall back to Latin.
    if font.covers('あ') && font.covers('日') {
        FontScript::Japanese
    } else if font.covers('A') {
        FontScript::Latin
    } else {
        FontScript::Other
    }
}

/// Rotate `p` by `theta` (radians, CW in image space) about `pivot`.
fn rotate_about(p: (f32, f32), pivot: (f32, f32), theta: f32) -> (f32, f32) {
    let (s, c) = theta.sin_cos();
    let rx = p.0 - pivot.0;
    let ry = p.1 - pivot.1;
    (pivot.0 + rx * c - ry * s, pivot.1 + rx * s + ry * c)
}

/// Inverse of `rotate_about` (rotate by `-theta`).
fn inv_rotate_about(p: (f32, f32), pivot: (f32, f32), theta: f32) -> (f32, f32) {
    rotate_about(p, pivot, -theta)
}

/// Set a bubble shape's half-extents (rx/ry or half_w/half_h), preserving the
/// variant + its other params. Used by corner-resize dragging.
fn set_bubble_half_extents(b: &mut BubbleObject, hw: f32, hh: f32) {
    match &mut b.shape {
        BubbleShape::Ellipse { rx, ry, .. } => {
            *rx = hw;
            *ry = hh;
        }
        BubbleShape::RoundRect { half_w, half_h, .. } => {
            *half_w = hw;
            *half_h = hh;
        }
        BubbleShape::Burst { rx, ry, .. } => {
            *rx = hw;
            *ry = hh;
        }
        BubbleShape::Cloud { rx, ry, .. } => {
            *rx = hw;
            *ry = hh;
        }
        BubbleShape::Polygon { rx, ry, .. }
        | BubbleShape::Heart { rx, ry }
        | BubbleShape::MotionLines { rx, ry, .. }
        | BubbleShape::Concentration { rx, ry, .. } => {
            *rx = hw;
            *ry = hh;
        }
        BubbleShape::SpeedLines { half_w, half_h, .. }
        | BubbleShape::Diamond { half_w, half_h }
        | BubbleShape::Arrow { half_w, half_h, .. }
        | BubbleShape::Soft { half_w, half_h, .. }
        | BubbleShape::TextOnly { half_w, half_h }
        | BubbleShape::Strokes { half_w, half_h, .. }
        | BubbleShape::DoubleStroke { half_w, half_h, .. } => {
            *half_w = hw;
            *half_h = hh;
        }
    }
}

/// True if two marker-rule lists use the same open/close char pairs in the same
/// order (used to detect which built-in marker set is active). Direction is
/// fixed by position so we only compare the bracket chars.
fn marker_pairs_eq(a: &[MarkupRule], b: &[MarkupRule]) -> bool {
    a.len() == b.len()
        && a.iter()
            .zip(b.iter())
            .all(|(x, y)| x.open == y.open && x.close == y.close)
}

fn default_bubble_tail(pivot: (f32, f32)) -> Tail {
    // Point the default tail down-LEFT at 45° rather than straight down — a
    // diagonal tail reads as "speaking toward someone" and looks more natural than
    // a vertical spike. Length ~150px (106 = 150/√2 on each axis).
    const TAIL_DIAG: f32 = 106.0;
    Tail {
        // Shorter + slimmer default so a fresh bubble's tail looks proportionate
        // (comic-core also caps the base width to the bubble's perpendicular
        // extent, so this width is an upper request).
        tip: (pivot.0 - TAIL_DIAG, pivot.1 + TAIL_DIAG),
        base_t: 0.25,
        base_auto: true,
        width_px: 32.0,
        kind: TailKind::Spike,
    }
}

/// One clickable preset thumbnail (a small rendered bubble + the preset name
/// below). Returns true when clicked. Used by the add dialog.
/// Cell width of a bubble-preset thumbnail (and the add-dialog grid column).
const PRESET_CELL_W: f32 = 96.0;
/// Cell width of a window-preset thumbnail.
const WINDOW_PRESET_CELL_W: f32 = 150.0;
/// Cell width of a font-generated onomatopoeia thumbnail.
const ONOMATO_PRESET_CELL_W: f32 = 184.0;
/// Spacing between thumbnail cells in the add dialogs.
const PRESET_CELL_SPACING: f32 = 10.0;

/// Number of columns that fit `cell_w`-wide thumbnails (+ spacing) into `avail`
/// width. At least 1. Used to lay out the add-dialog grids manually so they
/// reflow on resize without `horizontal_wrapped` forcing a one-row min width.
fn grid_cols(avail: f32, cell_w: f32) -> usize {
    (((avail + PRESET_CELL_SPACING) / (cell_w + PRESET_CELL_SPACING)).floor() as usize).max(1)
}

fn draw_preset_thumbnail(ui: &mut egui::Ui, preset: BubblePreset) -> bool {
    const CELL_W: f32 = PRESET_CELL_W;
    const PREVIEW_H: f32 = 64.0;
    let resp = ui
        .vertical(|ui| {
            ui.set_width(CELL_W);
            let (rect, r) = ui.allocate_exact_size(egui::vec2(CELL_W, PREVIEW_H), Sense::click());
            let hovered = r.hovered();
            let painter = ui.painter_at(rect);
            painter.rect_filled(
                rect,
                4.0,
                if hovered {
                    Color32::from_rgb(60, 60, 64)
                } else {
                    Color32::from_rgb(40, 40, 44)
                },
            );
            painter.rect_stroke(
                rect,
                4.0,
                egui::Stroke::new(
                    1.0,
                    if hovered {
                        Color32::from_rgb(150, 195, 255)
                    } else {
                        Color32::from_gray(70)
                    },
                ),
                egui::StrokeKind::Inside,
            );
            paint_bubble_preview(&painter, rect.shrink(8.0), preset);
            ui.add(egui::Label::new(
                egui::RichText::new(preset.label())
                    .size(11.0)
                    .color(Color32::WHITE),
            ));
            r
        })
        .inner;
    resp.clicked()
}

/// One clickable window-style preset thumbnail (a small rendered window panel +
/// the preset name below). Returns true when clicked.
fn draw_window_preset_thumbnail(ui: &mut egui::Ui, preset: &WindowStylePreset) -> bool {
    const CELL_W: f32 = WINDOW_PRESET_CELL_W;
    const PREVIEW_H: f32 = 60.0;
    let resp = ui
        .vertical(|ui| {
            ui.set_width(CELL_W);
            let (rect, r) = ui.allocate_exact_size(egui::vec2(CELL_W, PREVIEW_H), Sense::click());
            let hovered = r.hovered();
            let painter = ui.painter_at(rect);
            painter.rect_filled(
                rect,
                4.0,
                if hovered {
                    Color32::from_rgb(70, 70, 74)
                } else {
                    Color32::from_rgb(46, 46, 50)
                },
            );
            painter.rect_stroke(
                rect,
                4.0,
                egui::Stroke::new(
                    1.0,
                    if hovered {
                        Color32::from_rgb(150, 195, 255)
                    } else {
                        Color32::from_gray(70)
                    },
                ),
                egui::StrokeKind::Inside,
            );
            paint_window_preview(&painter, rect.shrink(7.0), preset);
            ui.add(egui::Label::new(
                egui::RichText::new(&preset.name)
                    .size(11.0)
                    .color(Color32::WHITE),
            ));
            r
        })
        .inner;
    resp.clicked()
}

fn paint_onomatopoeia_preview(painter: &egui::Painter, area: Rect, preset: OnomatopoeiaPreset) {
    let fill = rgba_to_color32(preset.color());
    let outline = preset
        .outline()
        .map(|s| rgba_to_color32(s.color))
        .unwrap_or(Color32::from_black_alpha(0));
    let center = area.center();
    let font_size = if preset.text().chars().count() >= 4 {
        24.0
    } else {
        28.0
    };
    let font_id = egui::FontId::proportional(font_size);
    let text = preset.text();
    if preset.outline().is_some() {
        for (dx, dy) in [
            (-2.0, 0.0),
            (2.0, 0.0),
            (0.0, -2.0),
            (0.0, 2.0),
            (-1.5, -1.5),
            (1.5, -1.5),
            (-1.5, 1.5),
            (1.5, 1.5),
        ] {
            painter.text(
                center + egui::vec2(dx, dy),
                egui::Align2::CENTER_CENTER,
                text,
                font_id.clone(),
                outline,
            );
        }
    }
    painter.text(center, egui::Align2::CENTER_CENTER, text, font_id, fill);
}

/// Paint a quick preview of a window-style preset inside `area`: the panel fill
/// (or gradient/scrim approximation), the frame, and a couple of text-color
/// lines as a body-text placeholder.
fn paint_window_preview(painter: &egui::Painter, area: Rect, p: &WindowStylePreset) {
    let to_c = |c: Rgba, a: u8| Color32::from_rgba_unmultiplied(c.r, c.g, c.b, a);
    let corner = (p.corner_px * 0.2).clamp(0.0, 10.0);
    if let Some(fill) = p.fill {
        let a = (fill.a as f32 * p.fill_opacity).round().clamp(0.0, 255.0) as u8;
        match p.fill_mode {
            FillMode::None => {}
            FillMode::GradientScrim => {
                // Approximate the scrim with bands matching the dense side.
                match p.scrim_dense_side {
                    VAnchor::Center => {
                        // faint top / dense middle / faint bottom.
                        let third = area.height() / 3.0;
                        let m1 = area.top() + third;
                        let m2 = area.bottom() - third;
                        painter.rect_filled(
                            Rect::from_min_max(area.min, egui::pos2(area.right(), m1)),
                            0.0,
                            to_c(fill, a / 5),
                        );
                        painter.rect_filled(
                            Rect::from_min_max(
                                egui::pos2(area.left(), m1),
                                egui::pos2(area.right(), m2),
                            ),
                            0.0,
                            to_c(fill, a),
                        );
                        painter.rect_filled(
                            Rect::from_min_max(egui::pos2(area.left(), m2), area.max),
                            0.0,
                            to_c(fill, a / 5),
                        );
                    }
                    other => {
                        let (top_a, bot_a) = match other {
                            VAnchor::Top => (a, a / 5),
                            _ => (a / 5, a),
                        };
                        let mid = area.center().y;
                        painter.rect_filled(
                            Rect::from_min_max(area.min, egui::pos2(area.right(), mid)),
                            0.0,
                            to_c(fill, top_a),
                        );
                        painter.rect_filled(
                            Rect::from_min_max(egui::pos2(area.left(), mid), area.max),
                            0.0,
                            to_c(fill, bot_a),
                        );
                    }
                }
            }
            FillMode::LinearGradient => {
                let to = p.gradient_to.unwrap_or(fill);
                let mid = area.center().y;
                painter.rect_filled(
                    Rect::from_min_max(area.min, egui::pos2(area.right(), mid)),
                    corner,
                    to_c(fill, a),
                );
                painter.rect_filled(
                    Rect::from_min_max(egui::pos2(area.left(), mid), area.max),
                    corner,
                    to_c(to, a),
                );
            }
            _ => {
                painter.rect_filled(area, corner, to_c(fill, a));
            }
        }
    }
    if !matches!(p.frame, FrameStyle::None) {
        let stroke = egui::Stroke::new(
            (p.outline.width_px * 0.4).clamp(1.0, 3.0),
            to_c(p.outline.color, p.outline.color.a),
        );
        painter.rect_stroke(area, corner, stroke, egui::StrokeKind::Inside);
        if matches!(p.frame, FrameStyle::DoubleLine) {
            painter.rect_stroke(
                area.shrink(3.0),
                (corner - 1.0).max(0.0),
                stroke,
                egui::StrokeKind::Inside,
            );
        }
    }
    // Body-text placeholder: two lines in the preset's text color.
    let line_col = to_c(p.text_style.color, 210);
    for i in 0..2 {
        let y = area.top() + area.height() * (0.45 + i as f32 * 0.22);
        painter.line_segment(
            [
                egui::pos2(area.left() + 8.0, y),
                egui::pos2(area.right() - 8.0 - i as f32 * 16.0, y),
            ],
            egui::Stroke::new(3.0, line_col),
        );
    }
}

/// Render a scaled-to-fit preview of `preset`'s bubble (fill + unified outline +
/// optional spike/thought tail) inside `area`, using comic-core geometry so the
/// preview matches what gets baked.
fn paint_bubble_preview(painter: &egui::Painter, area: Rect, preset: BubblePreset) {
    // Build the geometry in a neutral local space, then map to `area`.
    let pivot = (0.0f32, 0.0f32);
    let shape = preset.shape();

    // なし: no box — show a couple of text-placeholder lines so the thumbnail
    // reads as "text only".
    if matches!(shape, BubbleShape::TextOnly { .. }) {
        let line_col = Color32::from_gray(210);
        for i in 0..2 {
            let y = area.top() + area.height() * (0.40 + i as f32 * 0.24);
            painter.line_segment(
                [
                    egui::pos2(area.left() + 10.0, y),
                    egui::pos2(area.right() - 10.0 - i as f32 * 14.0, y),
                ],
                egui::Stroke::new(3.0, line_col),
            );
        }
        return;
    }

    // 集中線 / 流線: line fields aren't a filled polygon — draw the lines around a
    // clear center (light, so they show on the dark thumbnail). Matches the bake.
    const CLEAR: f32 = 0.55; // comic_core::LINE_FIELD_CLEAR_RATIO
    let line_col = Color32::from_gray(210);
    if matches!(shape, BubbleShape::MotionLines { .. }) {
        let (cx, cy) = (area.center().x, area.center().y);
        let (rx, ry) = (area.width() * 0.46, area.height() * 0.46);
        let n = 22;
        for i in 0..n {
            let a = i as f32 / n as f32 * std::f32::consts::TAU;
            let (c, s) = (a.cos(), a.sin());
            painter.line_segment(
                [
                    egui::pos2(cx + rx * CLEAR * c, cy + ry * CLEAR * s),
                    egui::pos2(cx + rx * c, cy + ry * s),
                ],
                egui::Stroke::new(1.4, line_col),
            );
        }
        return;
    }
    if matches!(shape, BubbleShape::SpeedLines { .. }) {
        // Horizontal parallel streaks (preset dir is 0) skipping a clear center.
        let (cx, cy) = (area.center().x, area.center().y);
        let (rx, ry) = (area.width() * 0.47, area.height() * 0.44);
        let n = 9;
        for i in 0..n {
            let f = i as f32 / (n as f32 - 1.0) * 2.0 - 1.0; // -1..1
            let yoff = f * ry;
            let outer_k = 1.0 - (yoff / ry).powi(2);
            if outer_k <= 0.0 {
                continue;
            }
            let half = rx * outer_k.sqrt();
            let y = cy + yoff;
            // Clear-ellipse half-width at this y (0 outside the clear band).
            let clear_k = (CLEAR * CLEAR) - (yoff / ry).powi(2);
            let gap = if clear_k > 0.0 {
                rx * clear_k.sqrt()
            } else {
                0.0
            };
            if gap > 0.0 {
                painter.line_segment(
                    [egui::pos2(cx - half, y), egui::pos2(cx - gap, y)],
                    egui::Stroke::new(1.4, line_col),
                );
                painter.line_segment(
                    [egui::pos2(cx + gap, y), egui::pos2(cx + half, y)],
                    egui::Stroke::new(1.4, line_col),
                );
            } else {
                painter.line_segment(
                    [egui::pos2(cx - half, y), egui::pos2(cx + half, y)],
                    egui::Stroke::new(1.4, line_col),
                );
            }
        }
        return;
    }
    // 意識: a soft, fuzzy ellipse — translucent fill + a faint thin rim (hint at
    // the feathered edge instead of the hard outline the generic path would draw).
    if let BubbleShape::Concentration { .. } = shape {
        let c = area.center();
        let r = egui::vec2(area.width() * 0.44, area.height() * 0.44);
        painter.add(egui::Shape::ellipse_filled(
            c,
            r,
            Color32::from_rgba_unmultiplied(255, 255, 255, 150),
        ));
        painter.add(egui::Shape::ellipse_stroke(
            c,
            r,
            egui::Stroke::new(1.2, Color32::from_gray(180)),
        ));
        return;
    }

    // A small tail pointing down-left, only for presets that have one.
    let tail = preset.tail_kind().map(|kind| Tail {
        tip: (-70.0, 200.0),
        base_t: 0.30,
        base_auto: true,
        width_px: 60.0,
        kind,
    });
    let geo = bubble_geometry(&shape, pivot, tail.as_ref());

    // Compute bounds over the union of outline + thought circles for scaling.
    let mut min = (f32::INFINITY, f32::INFINITY);
    let mut max = (f32::NEG_INFINITY, f32::NEG_INFINITY);
    let mut grow = |x: f32, y: f32| {
        min.0 = min.0.min(x);
        min.1 = min.1.min(y);
        max.0 = max.0.max(x);
        max.1 = max.1.max(y);
    };
    for &(x, y) in &geo.outline {
        grow(x, y);
    }
    for &(cx, cy, r) in &geo.thought {
        grow(cx - r, cy - r);
        grow(cx + r, cy + r);
    }
    if !min.0.is_finite() {
        return;
    }
    let w = (max.0 - min.0).max(1.0);
    let h = (max.1 - min.1).max(1.0);
    let scale = (area.width() / w).min(area.height() / h);
    let cx = (min.0 + max.0) * 0.5;
    let cy = (min.1 + max.1) * 0.5;
    let map = |p: (f32, f32)| -> Pos2 {
        Pos2::new(
            area.center().x + (p.0 - cx) * scale,
            area.center().y + (p.1 - cy) * scale,
        )
    };

    let fill = Color32::WHITE;
    let stroke = egui::Stroke::new((preset.outline_width() * scale).max(1.0), Color32::BLACK);

    // Fill the unified outline as a triangle fan from the (interior) pivot.
    // egui's convex_polygon only fills convex shapes correctly, but the burst /
    // cloud contours are concave; they are however star-shaped about the pivot,
    // so a fan from the center renders them solid without spilling. Then stroke
    // the same contour so the つの shares the body edge (no double stroke).
    let center = map(pivot);
    let outline: Vec<Pos2> = geo.outline.iter().map(|&p| map(p)).collect();
    if outline.len() >= 3 {
        let mut mesh = egui::Mesh::default();
        mesh.colored_vertex(center, fill);
        for &p in &outline {
            mesh.colored_vertex(p, fill);
        }
        let n = outline.len() as u32;
        for i in 0..n {
            let a = 1 + i;
            let b = 1 + (i + 1) % n;
            mesh.add_triangle(0, a, b);
        }
        painter.add(egui::Shape::mesh(mesh));
        painter.add(egui::Shape::closed_line(outline, stroke));
    }
    // 二重線: a second, inner concentric ring (matches the bake).
    if let BubbleShape::DoubleStroke {
        half_w,
        half_h,
        corner_px,
        gap_px,
    } = shape
    {
        let g = gap_px.max(1.0);
        let inner_shape = BubbleShape::RoundRect {
            half_w: (half_w - g).max(1.0),
            half_h: (half_h - g).max(1.0),
            corner_px: (corner_px - g).max(0.0),
        };
        let inner: Vec<Pos2> = tessellate_bubble(&inner_shape, pivot)
            .iter()
            .map(|&p| map(p))
            .collect();
        if inner.len() >= 3 {
            painter.add(egui::Shape::closed_line(inner, stroke));
        }
    }
    // Thought-tail circles (filled + stroked).
    for &(tcx, tcy, r) in &geo.thought {
        let c = map((tcx, tcy));
        painter.circle(c, r * scale, fill, stroke);
    }
}

fn short(s: &str) -> String {
    let line = s.lines().next().unwrap_or("");
    if line.chars().count() > 12 {
        let t: String = line.chars().take(12).collect();
        format!("{t}…")
    } else {
        line.to_string()
    }
}

#[derive(Clone, Copy)]
enum ObjAction {
    MoveUp,
    MoveDown,
    Duplicate,
    Delete,
}

/// A coherent bubble preset: shape + tail + fill + outline + text styling all
/// chosen together. The key fix over the old per-field shape ComboBox is that a
/// preset always sets the tail kind to match the shape (Cloud ⇒ Thought,
/// everything else ⇒ Spike), so switching never leaves a broken つの.
#[derive(Clone, Copy, PartialEq, Eq)]
enum BubblePreset {
    Normal,
    RoundRect,
    Narration,
    Thought,
    Shout,
    Whisper,
    Soft,
    Polygon,
    Diamond,
    Heart,
    Arrow,
    MotionLines,
    SpeedLines,
    Concentration,
    MindEllipse,
    Strokes,
    DoubleStroke,
    TextOnly,
}

impl BubblePreset {
    /// All presets in display order (used by both the add dialog and the
    /// right-panel preset switch row).
    const ALL: &'static [BubblePreset] = &[
        BubblePreset::Normal,
        BubblePreset::RoundRect,
        BubblePreset::Soft,
        BubblePreset::Narration,
        BubblePreset::Thought,
        BubblePreset::MindEllipse,
        BubblePreset::Shout,
        BubblePreset::Whisper,
        BubblePreset::Concentration,
        BubblePreset::Polygon,
        BubblePreset::Diamond,
        BubblePreset::Heart,
        BubblePreset::Arrow,
        BubblePreset::MotionLines,
        BubblePreset::SpeedLines,
        BubblePreset::Strokes,
        BubblePreset::DoubleStroke,
        BubblePreset::TextOnly,
    ];

    fn label(self) -> &'static str {
        match self {
            BubblePreset::Normal => "通常",
            BubblePreset::RoundRect => "角丸",
            BubblePreset::Narration => "ナレーション",
            BubblePreset::Thought => "思考",
            BubblePreset::Shout => "叫び",
            BubblePreset::Whisper => "ささやき",
            BubblePreset::Soft => "やわらか",
            BubblePreset::Polygon => "多角形",
            BubblePreset::Diamond => "ダイヤ",
            BubblePreset::Heart => "ハート",
            BubblePreset::Arrow => "矢印",
            BubblePreset::MotionLines => "集中線",
            BubblePreset::SpeedLines => "流線",
            BubblePreset::Concentration => "意識",
            BubblePreset::MindEllipse => "思考(楕円)",
            BubblePreset::Strokes => "線",
            BubblePreset::DoubleStroke => "二重線",
            BubblePreset::TextOnly => "なし",
        }
    }

    /// Stable ASCII slug for the system shape-preset id.
    fn sys_slug(self) -> &'static str {
        match self {
            BubblePreset::Normal => "normal",
            BubblePreset::RoundRect => "roundrect",
            BubblePreset::Narration => "narration",
            BubblePreset::Thought => "thought",
            BubblePreset::Shout => "shout",
            BubblePreset::Whisper => "whisper",
            BubblePreset::Soft => "soft",
            BubblePreset::Polygon => "polygon",
            BubblePreset::Diamond => "diamond",
            BubblePreset::Heart => "heart",
            BubblePreset::Arrow => "arrow",
            BubblePreset::MotionLines => "motion-lines",
            BubblePreset::SpeedLines => "speed-lines",
            BubblePreset::Concentration => "concentration",
            BubblePreset::MindEllipse => "mind-ellipse",
            BubblePreset::Strokes => "strokes",
            BubblePreset::DoubleStroke => "double-strokes",
            BubblePreset::TextOnly => "none",
        }
    }

    /// The shape for this preset (used to populate a fresh BubbleObject and to
    /// highlight the matching preset for the selected bubble).
    fn shape(self) -> BubbleShape {
        match self {
            BubblePreset::Normal | BubblePreset::Whisper => BubbleShape::Ellipse {
                rx: 160.0,
                ry: 100.0,
                circle: false,
            },
            BubblePreset::RoundRect => BubbleShape::RoundRect {
                half_w: 160.0,
                half_h: 100.0,
                corner_px: 28.0,
            },
            BubblePreset::Narration => BubbleShape::RoundRect {
                half_w: 170.0,
                half_h: 90.0,
                corner_px: 0.0,
            },
            BubblePreset::Thought => BubbleShape::Cloud {
                rx: 170.0,
                ry: 115.0,
                lobes: 11,
                amp: 0.14,
                shape_seed: 0,
            },
            BubblePreset::Shout => BubbleShape::Burst {
                rx: 170.0,
                ry: 120.0,
                spikes: 20,
                jag: 0.55,
                shape_seed: 1,
            },
            BubblePreset::Soft => BubbleShape::Soft {
                half_w: 165.0,
                half_h: 105.0,
                corner_px: 38.0,
                shape_seed: 0,
            },
            BubblePreset::Polygon => BubbleShape::Polygon {
                rx: 155.0,
                ry: 125.0,
                sides: 6,
            },
            BubblePreset::Diamond => BubbleShape::Diamond {
                half_w: 160.0,
                half_h: 130.0,
            },
            BubblePreset::Heart => BubbleShape::Heart {
                rx: 150.0,
                ry: 140.0,
            },
            BubblePreset::Arrow => BubbleShape::Arrow {
                half_w: 150.0,
                half_h: 110.0,
                dir_rad: -std::f32::consts::FRAC_PI_2,
                head_len_px: None,
                shaft_half_px: None,
            },
            BubblePreset::MotionLines => BubbleShape::MotionLines {
                rx: 240.0,
                ry: 180.0,
                count: 72,
                shape_seed: 0,
            },
            BubblePreset::SpeedLines => BubbleShape::SpeedLines {
                half_w: 260.0,
                half_h: 170.0,
                dir_rad: 0.0,
                count: 48,
                shape_seed: 0,
            },
            BubblePreset::Concentration => BubbleShape::Concentration {
                rx: 180.0,
                ry: 120.0,
                shape_seed: 0,
            },
            BubblePreset::MindEllipse => BubbleShape::Ellipse {
                rx: 165.0,
                ry: 110.0,
                circle: false,
            },
            BubblePreset::Strokes => BubbleShape::Strokes {
                half_w: 165.0,
                half_h: 105.0,
                corner_px: 36.0,
                shape_seed: 0,
            },
            BubblePreset::DoubleStroke => BubbleShape::DoubleStroke {
                half_w: 165.0,
                half_h: 105.0,
                corner_px: 26.0,
                gap_px: 8.0,
            },
            BubblePreset::TextOnly => BubbleShape::TextOnly {
                half_w: 150.0,
                half_h: 95.0,
            },
        }
    }

    fn tail_kind(self) -> Option<TailKind> {
        match self {
            // No tail for narration, the line-field effects (集中線 / 流線),
            // 意識 (fuzzy, edgeless), なし (text-only), or 矢印 (already a pointer —
            // a spike tail on top is redundant and looks broken once auto-sized).
            BubblePreset::Narration
            | BubblePreset::MotionLines
            | BubblePreset::SpeedLines
            | BubblePreset::Concentration
            | BubblePreset::TextOnly
            | BubblePreset::Arrow => None,
            // Thought-style tails (もくもく cloud + clean 楕円 thought).
            BubblePreset::Thought | BubblePreset::MindEllipse => Some(TailKind::Thought),
            _ => Some(TailKind::Spike),
        }
    }

    /// Default 袋文字 (text outline) for this preset. Line-field effects have no
    /// fill behind the text, so a white halo keeps the black text readable against
    /// the lines + the underlying image.
    fn text_outline(self) -> Option<StrokeStyle> {
        match self {
            BubblePreset::MotionLines | BubblePreset::SpeedLines => Some(StrokeStyle {
                color: Rgba::WHITE,
                width_px: 6.0,
            }),
            _ => None,
        }
    }

    fn outline_width(self) -> f32 {
        match self {
            BubblePreset::Shout => 5.0,
            BubblePreset::Whisper => 1.5,
            _ => 3.0,
        }
    }

    fn text_align(self) -> TextAlign {
        match self {
            BubblePreset::Narration => TextAlign::Start,
            _ => TextAlign::Center,
        }
    }

    fn text_color(self) -> Rgba {
        match self {
            BubblePreset::Whisper => Rgba::new(120, 120, 120, 255),
            _ => Rgba::BLACK,
        }
    }

    /// Build a brand-new bubble for this preset, with a fresh default text block
    /// (縦書き + markup ON, matching the add-defaults).
    fn build_bubble(self, pivot: (f32, f32), font_key: &str) -> BubbleObject {
        let mut b = BubbleObject {
            shape: self.shape(),
            fill: Some(Rgba::WHITE),
            fill_opacity: 1.0,
            blend: comic_core::FillBlend::Normal,
            outline: StrokeStyle {
                color: Rgba::BLACK,
                width_px: self.outline_width(),
            },
            tail: None,
            padding_px: 16.0,
            decorations: Vec::new(),
            text: TextBlock {
                text: "セリフ".to_string(),
                font_key: font_key.to_string(),
                size_px: 40.0,
                color: self.text_color(),
                align: self.text_align(),
                orientation: Orientation::Vertical,
                markup_enabled: true,
                outline: self.text_outline(),
                ..TextBlock::default()
            },
            auto_size: true,
            merge_with_below: false,
            shape_preset_link: None,
        };
        if let Some(kind) = self.tail_kind() {
            let mut tail = default_bubble_tail(pivot);
            tail.kind = kind;
            b.tail = Some(tail);
        }
        b
    }
}

// ---------------------------------------------------------------------------
// Preset data model (text-style + shape-style; system + user).
//
// A preset is a named bundle of style fields plus an opaque `id`. Applying a
// preset copies its fields onto the selected object AND stamps the object's
// link field (`TextBlock::preset_link` / `BubbleObject::shape_preset_link`)
// with the preset id. The link drives the button glow; any individual control
// edit clears it. System presets use `sys:*` ids and aren't editable/removable;
// user presets use `user:<name>` ids and persist to presets.json.
// ---------------------------------------------------------------------------

#[derive(Clone, Copy)]
struct OnomatopoeiaPreset {
    label: &'static str,
    text: &'static str,
    font_candidate: &'static str,
    size_px: f32,
    color: Rgba,
    orientation: Orientation,
    letter_gap: f32,
    outline: Option<StrokeStyle>,
    rotation_deg: f32,
}

const ONOMATOPOEIA_PRESETS: &[OnomatopoeiaPreset] = &[
    OnomatopoeiaPreset {
        label: "Otomanopee One",
        text: "ドンッ",
        font_candidate: "Otomanopee One",
        size_px: 92.0,
        color: Rgba::BLACK,
        orientation: Orientation::Horizontal,
        letter_gap: 0.0,
        outline: Some(StrokeStyle {
            color: Rgba::WHITE,
            width_px: 8.0,
        }),
        rotation_deg: -8.0,
    },
    OnomatopoeiaPreset {
        label: "Dela Gothic One",
        text: "ガーン",
        font_candidate: "Dela Gothic One",
        size_px: 92.0,
        color: Rgba::new(44, 78, 190, 255),
        orientation: Orientation::Horizontal,
        letter_gap: 0.0,
        outline: Some(StrokeStyle {
            color: Rgba::WHITE,
            width_px: 7.0,
        }),
        rotation_deg: 5.0,
    },
    OnomatopoeiaPreset {
        label: "Reggae One",
        text: "ゴゴゴ",
        font_candidate: "Reggae One",
        size_px: 78.0,
        color: Rgba::new(42, 28, 72, 255),
        orientation: Orientation::Vertical,
        letter_gap: 2.0,
        outline: Some(StrokeStyle {
            color: Rgba::new(218, 196, 255, 255),
            width_px: 5.0,
        }),
        rotation_deg: 0.0,
    },
    OnomatopoeiaPreset {
        label: "RocknRoll One",
        text: "ザザッ",
        font_candidate: "RocknRoll One",
        size_px: 82.0,
        color: Rgba::WHITE,
        orientation: Orientation::Horizontal,
        letter_gap: 0.0,
        outline: Some(StrokeStyle {
            color: Rgba::BLACK,
            width_px: 7.0,
        }),
        rotation_deg: -10.0,
    },
    OnomatopoeiaPreset {
        label: "Rampart One",
        text: "バァン",
        font_candidate: "Rampart One",
        size_px: 88.0,
        color: Rgba::new(255, 216, 64, 255),
        orientation: Orientation::Horizontal,
        letter_gap: 0.0,
        outline: Some(StrokeStyle {
            color: Rgba::BLACK,
            width_px: 4.0,
        }),
        rotation_deg: 6.0,
    },
    OnomatopoeiaPreset {
        label: "Stick",
        text: "シュッ",
        font_candidate: "Stick",
        size_px: 84.0,
        color: Rgba::WHITE,
        orientation: Orientation::Horizontal,
        letter_gap: 0.0,
        outline: Some(StrokeStyle {
            color: Rgba::BLACK,
            width_px: 6.0,
        }),
        rotation_deg: -14.0,
    },
    OnomatopoeiaPreset {
        label: "Train One",
        text: "ビューン",
        font_candidate: "Train One",
        size_px: 78.0,
        color: Rgba::new(90, 230, 255, 255),
        orientation: Orientation::Horizontal,
        letter_gap: 0.0,
        outline: Some(StrokeStyle {
            color: Rgba::new(12, 40, 70, 255),
            width_px: 5.0,
        }),
        rotation_deg: -12.0,
    },
    OnomatopoeiaPreset {
        label: "DotGothic16",
        text: "ピコ",
        font_candidate: "DotGothic16",
        size_px: 64.0,
        color: Rgba::new(125, 255, 110, 255),
        orientation: Orientation::Horizontal,
        letter_gap: 0.0,
        outline: Some(StrokeStyle {
            color: Rgba::new(20, 55, 25, 255),
            width_px: 4.0,
        }),
        rotation_deg: 0.0,
    },
    OnomatopoeiaPreset {
        label: "Hachi Maru Pop",
        text: "わーい",
        font_candidate: "Hachi Maru Pop",
        size_px: 74.0,
        color: Rgba::new(255, 140, 45, 255),
        orientation: Orientation::Horizontal,
        letter_gap: 0.0,
        outline: Some(StrokeStyle {
            color: Rgba::WHITE,
            width_px: 6.0,
        }),
        rotation_deg: -4.0,
    },
    OnomatopoeiaPreset {
        label: "Darumadrop One",
        text: "ぽよん",
        font_candidate: "Darumadrop One",
        size_px: 78.0,
        color: Rgba::new(245, 70, 145, 255),
        orientation: Orientation::Horizontal,
        letter_gap: 0.0,
        outline: Some(StrokeStyle {
            color: Rgba::WHITE,
            width_px: 6.0,
        }),
        rotation_deg: 6.0,
    },
    OnomatopoeiaPreset {
        label: "Yusei Magic",
        text: "キラキラ",
        font_candidate: "Yusei Magic",
        size_px: 68.0,
        color: Rgba::new(255, 230, 72, 255),
        orientation: Orientation::Horizontal,
        letter_gap: 0.0,
        outline: Some(StrokeStyle {
            color: Rgba::new(92, 62, 0, 255),
            width_px: 5.0,
        }),
        rotation_deg: 4.0,
    },
    OnomatopoeiaPreset {
        label: "Klee One SemiBold",
        text: "しーん",
        font_candidate: "Klee One SemiBold",
        size_px: 58.0,
        color: Rgba::new(122, 126, 132, 255),
        orientation: Orientation::Horizontal,
        letter_gap: 7.0,
        outline: None,
        rotation_deg: 0.0,
    },
    OnomatopoeiaPreset {
        label: "Kaisei Decol Bold",
        text: "ふわっ",
        font_candidate: "Kaisei Decol Bold",
        size_px: 64.0,
        color: Rgba::new(245, 220, 255, 255),
        orientation: Orientation::Horizontal,
        letter_gap: 0.0,
        outline: Some(StrokeStyle {
            color: Rgba::new(74, 40, 120, 255),
            width_px: 4.0,
        }),
        rotation_deg: -3.0,
    },
    OnomatopoeiaPreset {
        label: "Zen Kurenaido",
        text: "ひそ",
        font_candidate: "Zen Kurenaido",
        size_px: 50.0,
        color: Rgba::new(150, 150, 155, 255),
        orientation: Orientation::Horizontal,
        letter_gap: 3.0,
        outline: Some(StrokeStyle {
            color: Rgba::WHITE,
            width_px: 3.0,
        }),
        rotation_deg: 0.0,
    },
    OnomatopoeiaPreset {
        label: "Kaisei Tokumin ExtraBold",
        text: "ズシン",
        font_candidate: "Kaisei Tokumin ExtraBold",
        size_px: 86.0,
        color: Rgba::BLACK,
        orientation: Orientation::Horizontal,
        letter_gap: 0.0,
        outline: Some(StrokeStyle {
            color: Rgba::WHITE,
            width_px: 6.0,
        }),
        rotation_deg: 3.0,
    },
    OnomatopoeiaPreset {
        label: "Zen Maru Gothic Black",
        text: "ぷにっ",
        font_candidate: "Zen Maru Gothic Black",
        size_px: 76.0,
        color: Rgba::new(130, 235, 190, 255),
        orientation: Orientation::Horizontal,
        letter_gap: 0.0,
        outline: Some(StrokeStyle {
            color: Rgba::new(18, 76, 62, 255),
            width_px: 5.0,
        }),
        rotation_deg: -4.0,
    },
    OnomatopoeiaPreset {
        label: "M PLUS 1",
        text: "バシッ",
        font_candidate: "M PLUS 1",
        size_px: 82.0,
        color: Rgba::BLACK,
        orientation: Orientation::Horizontal,
        letter_gap: 0.0,
        outline: Some(StrokeStyle {
            color: Rgba::WHITE,
            width_px: 7.0,
        }),
        rotation_deg: -8.0,
    },
    OnomatopoeiaPreset {
        label: "Shippori Mincho Bold",
        text: "ぞくっ",
        font_candidate: "Shippori Mincho Bold",
        size_px: 70.0,
        color: Rgba::new(38, 48, 86, 255),
        orientation: Orientation::Horizontal,
        letter_gap: 0.0,
        outline: Some(StrokeStyle {
            color: Rgba::WHITE,
            width_px: 4.0,
        }),
        rotation_deg: -2.0,
    },
];

impl OnomatopoeiaPreset {
    const ALL: &'static [OnomatopoeiaPreset] = ONOMATOPOEIA_PRESETS;

    fn label(self) -> &'static str {
        self.label
    }

    fn text(self) -> &'static str {
        self.text
    }

    fn font_candidate(self) -> &'static str {
        self.font_candidate
    }

    fn color(self) -> Rgba {
        self.color
    }

    fn outline(self) -> Option<StrokeStyle> {
        self.outline
    }

    fn size_px(self) -> f32 {
        self.size_px
    }

    fn rotation_rad(self) -> f32 {
        self.rotation_deg.to_radians()
    }

    fn orientation(self) -> Orientation {
        self.orientation
    }

    fn letter_gap(self) -> f32 {
        self.letter_gap
    }

    fn build_text(self, font_key: &str) -> TextBlock {
        TextBlock {
            text: self.text().to_string(),
            font_key: font_key.to_string(),
            size_px: self.size_px(),
            color: self.color(),
            orientation: self.orientation(),
            align: TextAlign::Center,
            line_gap: 0.0,
            letter_gap: self.letter_gap(),
            outline: self.outline(),
            auto_tcy: false,
            markup_enabled: false,
            ..TextBlock::default()
        }
    }
}

/// A reusable text-style preset (everything that styles a TextBlock except its
/// content / markup rules).
#[derive(Clone, Serialize, Deserialize)]
struct TextStylePreset {
    id: String,
    name: String,
    font_key: String,
    size_px: f32,
    color: Rgba,
    orientation: Orientation,
    align: TextAlign,
    line_gap: f32,
    letter_gap: f32,
    outline: Option<StrokeStyle>,
    auto_tcy: bool,
    markup_enabled: bool,
}

impl TextStylePreset {
    /// Copy all style fields onto `tb` and link it to this preset.
    fn apply_to(&self, tb: &mut TextBlock) {
        tb.font_key = self.font_key.clone();
        tb.size_px = self.size_px;
        tb.color = self.color;
        tb.orientation = self.orientation;
        tb.align = self.align;
        tb.line_gap = self.line_gap;
        tb.letter_gap = self.letter_gap;
        tb.outline = self.outline;
        tb.auto_tcy = self.auto_tcy;
        tb.markup_enabled = self.markup_enabled;
        tb.preset_link = Some(self.id.clone());
    }

    /// Capture the current style of `tb` as a new user preset with `id`/`name`.
    fn from_text(id: String, name: String, tb: &TextBlock) -> Self {
        TextStylePreset {
            id,
            name,
            font_key: tb.font_key.clone(),
            size_px: tb.size_px,
            color: tb.color,
            orientation: tb.orientation,
            align: tb.align,
            line_gap: tb.line_gap,
            letter_gap: tb.letter_gap,
            outline: tb.outline,
            auto_tcy: tb.auto_tcy,
            markup_enabled: tb.markup_enabled,
        }
    }
}

/// A reusable shape-style preset (everything that styles a BubbleObject's
/// container — shape + tail kind + fill + outline + padding; text untouched).
#[derive(Clone, Serialize, Deserialize)]
struct ShapeStylePreset {
    id: String,
    name: String,
    shape: BubbleShape,
    tail_kind: Option<TailKind>,
    fill: Option<Rgba>,
    fill_opacity: f32,
    outline: StrokeStyle,
    padding_px: f32,
}

impl ShapeStylePreset {
    /// Apply the container style coherently to `b`, preserving its text and the
    /// tail tip/base (only the tail kind is reconciled with the preset), and
    /// link it to this preset.
    fn apply_to(&self, b: &mut BubbleObject, pivot: (f32, f32)) {
        b.shape = self.shape;
        b.fill = self.fill;
        b.fill_opacity = self.fill_opacity;
        b.outline = self.outline;
        b.padding_px = self.padding_px;
        match self.tail_kind {
            None => b.tail = None,
            Some(kind) => {
                let mut tail = b.tail.unwrap_or_else(|| default_bubble_tail(pivot));
                tail.kind = kind;
                b.tail = Some(tail);
            }
        }
        b.shape_preset_link = Some(self.id.clone());
    }

    /// Capture the current container style of `b` as a new user preset.
    fn from_bubble(id: String, name: String, b: &BubbleObject) -> Self {
        ShapeStylePreset {
            id,
            name,
            shape: b.shape,
            tail_kind: b.tail.map(|t| t.kind),
            fill: b.fill,
            fill_opacity: b.fill_opacity,
            outline: b.outline,
            padding_px: b.padding_px,
        }
    }
}

/// A reusable message-window style (everything except the body/name TEXT
/// content and the resolved pivot). Same link contract as ShapeStylePreset.
#[derive(Clone, Serialize, Deserialize)]
struct WindowStylePreset {
    id: String,
    name: String,
    size_mode: SizeMode,
    half_w: f32,
    half_h: f32,
    margin_px: f32,
    corner_px: f32,
    position: WindowPosition,
    fill_mode: FillMode,
    fill: Option<Rgba>,
    fill_opacity: f32,
    gradient_to: Option<Rgba>,
    scrim_dense_side: VAnchor,
    frame: FrameStyle,
    outline: StrokeStyle,
    frame_gap_px: f32,
    shadow: Option<ShadowStyle>,
    padding: Insets,
    v_anchor: VAnchor,
    wrap: bool,
    name_plate: NamePlate,
    portrait: PortraitSlot,
    indicator: IndicatorKind,
    #[serde(default)]
    indicator_auto: bool,
    /// Body text STYLE only (color/size/outline/orientation/align); the text
    /// content + font are preserved from the target on apply.
    text_style: TextBlock,
}

impl WindowStylePreset {
    fn apply_to(&self, w: &mut MessageWindowObject) {
        w.size_mode = self.size_mode;
        w.half_w = self.half_w;
        w.half_h = self.half_h;
        w.margin_px = self.margin_px;
        w.corner_px = self.corner_px;
        w.position = self.position;
        w.fill_mode = self.fill_mode;
        w.fill = self.fill;
        w.fill_opacity = self.fill_opacity;
        w.gradient_to = self.gradient_to;
        w.scrim_dense_side = self.scrim_dense_side;
        w.frame = self.frame;
        w.outline = self.outline;
        w.frame_gap_px = self.frame_gap_px;
        w.shadow = self.shadow;
        w.padding = self.padding;
        w.v_anchor = self.v_anchor;
        w.wrap = self.wrap;
        // Name plate: take the preset's styling but keep the user's name text/font.
        let name_text = w.name_plate.name.text.clone();
        let name_font = w.name_plate.name.font_key.clone();
        w.name_plate = self.name_plate.clone();
        w.name_plate.name.text = name_text;
        w.name_plate.name.font_key = name_font;
        w.portrait = self.portrait;
        w.indicator = self.indicator;
        w.indicator_auto = self.indicator_auto;
        // Body text: preset style, but keep the existing content + font.
        let content = std::mem::take(&mut w.text.text);
        let font = std::mem::take(&mut w.text.font_key);
        let mut ts = self.text_style.clone();
        ts.text = content;
        ts.font_key = font;
        ts.preset_link = None;
        w.text = ts;
        w.style_preset_link = Some(self.id.clone());
    }

    fn from_window(id: String, name: String, w: &MessageWindowObject) -> Self {
        let mut text_style = w.text.clone();
        text_style.text = String::new();
        text_style.preset_link = None;
        let mut name_plate = w.name_plate.clone();
        name_plate.name.text = String::new();
        WindowStylePreset {
            id,
            name,
            size_mode: w.size_mode,
            half_w: w.half_w,
            half_h: w.half_h,
            margin_px: w.margin_px,
            corner_px: w.corner_px,
            position: w.position,
            fill_mode: w.fill_mode,
            fill: w.fill,
            fill_opacity: w.fill_opacity,
            gradient_to: w.gradient_to,
            scrim_dense_side: w.scrim_dense_side,
            frame: w.frame,
            outline: w.outline,
            frame_gap_px: w.frame_gap_px,
            shadow: w.shadow,
            padding: w.padding,
            v_anchor: w.v_anchor,
            wrap: w.wrap,
            name_plate,
            portrait: w.portrait,
            indicator: w.indicator,
            indicator_auto: w.indicator_auto,
            text_style,
        }
    }
}

/// Built-in window-style presets (ids `sys:*`), ~10 covering JRPG / VN / social.
fn system_window_presets(default_font: &str) -> Vec<WindowStylePreset> {
    let white = |sz: f32| TextBlock {
        font_key: default_font.to_string(),
        size_px: sz,
        color: Rgba::WHITE,
        ..TextBlock::default()
    };
    let black = |sz: f32| TextBlock {
        font_key: default_font.to_string(),
        size_px: sz,
        color: Rgba::BLACK,
        ..TextBlock::default()
    };
    let name_tb = |color: Rgba| TextBlock {
        font_key: default_font.to_string(),
        size_px: 30.0,
        color,
        ..TextBlock::default()
    };
    let mk = |id: &str, name: &str, w: MessageWindowObject| {
        WindowStylePreset::from_window(id.to_string(), name.to_string(), &w)
    };
    vec![
        mk(
            "sys:win_dq",
            "DQ風 紺枠",
            MessageWindowObject {
                fill_mode: FillMode::Solid,
                fill: Some(Rgba::new(12, 18, 52, 255)),
                frame: FrameStyle::DoubleLine,
                outline: StrokeStyle {
                    color: Rgba::WHITE,
                    width_px: 3.0,
                },
                corner_px: 6.0,
                frame_gap_px: 6.0,
                text: white(40.0),
                indicator: IndicatorKind::Triangle,
                ..MessageWindowObject::default()
            },
        ),
        mk(
            "sys:win_ff",
            "FF風 青グラデ",
            MessageWindowObject {
                fill_mode: FillMode::LinearGradient,
                fill: Some(Rgba::new(30, 60, 160, 255)),
                gradient_to: Some(Rgba::new(8, 16, 60, 255)),
                frame: FrameStyle::SolidRounded,
                outline: StrokeStyle {
                    color: Rgba::WHITE,
                    width_px: 3.0,
                },
                corner_px: 10.0,
                text: TextBlock {
                    outline: Some(StrokeStyle {
                        color: Rgba::new(10, 20, 60, 255),
                        width_px: 3.0,
                    }),
                    ..white(40.0)
                },
                indicator: IndicatorKind::Triangle,
                ..MessageWindowObject::default()
            },
        ),
        mk(
            "sys:win_rpgm",
            "ツクール窓",
            MessageWindowObject {
                fill_mode: FillMode::Solid,
                fill: Some(Rgba::new(20, 24, 40, 235)),
                frame: FrameStyle::SolidRounded,
                outline: StrokeStyle {
                    color: Rgba::new(120, 150, 220, 255),
                    width_px: 3.0,
                },
                corner_px: 10.0,
                text: white(38.0),
                name_plate: NamePlate {
                    mode: NamePlateMode::Above,
                    name: name_tb(Rgba::WHITE),
                    ..NamePlate::default()
                },
                ..MessageWindowObject::default()
            },
        ),
        mk(
            "sys:win_dim",
            "ツクール暗幕",
            MessageWindowObject {
                fill_mode: FillMode::GradientScrim,
                fill: Some(Rgba::new(0, 0, 0, 255)),
                scrim_dense_side: VAnchor::Bottom,
                frame: FrameStyle::None,
                corner_px: 0.0,
                text: white(40.0),
                ..MessageWindowObject::default()
            },
        ),
        mk(
            "sys:win_adv_frameless",
            "枠なし下部",
            MessageWindowObject {
                fill_mode: FillMode::Translucent,
                fill: Some(Rgba::new(0, 0, 0, 140)),
                frame: FrameStyle::None,
                corner_px: 0.0,
                text: TextBlock {
                    outline: Some(StrokeStyle {
                        color: Rgba::BLACK,
                        width_px: 3.0,
                    }),
                    ..white(40.0)
                },
                ..MessageWindowObject::default()
            },
        ),
        mk(
            "sys:win_adv_framed",
            "枠あり下部",
            MessageWindowObject {
                fill_mode: FillMode::Translucent,
                fill: Some(Rgba::new(20, 20, 28, 200)),
                frame: FrameStyle::SolidRounded,
                outline: StrokeStyle {
                    color: Rgba::new(220, 220, 230, 255),
                    width_px: 2.0,
                },
                corner_px: 18.0,
                text: white(38.0),
                name_plate: NamePlate {
                    mode: NamePlateMode::Boxed,
                    name: name_tb(Rgba::WHITE),
                    fill: Some(Rgba::new(200, 80, 90, 255)),
                    ..NamePlate::default()
                },
                ..MessageWindowObject::default()
            },
        ),
        mk(
            "sys:win_vn_adv",
            "ノベルADV",
            MessageWindowObject {
                fill_mode: FillMode::Translucent,
                fill: Some(Rgba::new(10, 12, 24, 190)),
                frame: FrameStyle::SolidRounded,
                outline: StrokeStyle {
                    color: Rgba::new(180, 190, 210, 255),
                    width_px: 2.0,
                },
                corner_px: 14.0,
                text: white(38.0),
                name_plate: NamePlate {
                    mode: NamePlateMode::Inline,
                    name: name_tb(Rgba::new(255, 220, 120, 255)),
                    ..NamePlate::default()
                },
                ..MessageWindowObject::default()
            },
        ),
        mk(
            "sys:win_vn_nvl",
            "ノベルNVL",
            MessageWindowObject {
                size_mode: SizeMode::FullWidth,
                half_h: 300.0,
                position: WindowPosition::Center,
                fill_mode: FillMode::Translucent,
                fill: Some(Rgba::new(0, 0, 0, 150)),
                frame: FrameStyle::None,
                corner_px: 0.0,
                text: white(34.0),
                ..MessageWindowObject::default()
            },
        ),
        mk(
            "sys:win_vn_white",
            "ノベル白枠",
            MessageWindowObject {
                fill_mode: FillMode::Translucent,
                fill: Some(Rgba::new(250, 250, 250, 220)),
                frame: FrameStyle::SolidRounded,
                outline: StrokeStyle {
                    color: Rgba::new(90, 90, 100, 255),
                    width_px: 2.0,
                },
                corner_px: 16.0,
                text: black(38.0),
                ..MessageWindowObject::default()
            },
        ),
        mk(
            "sys:win_caption",
            "コミックキャプション",
            MessageWindowObject {
                size_mode: SizeMode::Inset,
                position: WindowPosition::Free,
                half_w: 220.0,
                half_h: 70.0,
                fill_mode: FillMode::Solid,
                fill: Some(Rgba::new(250, 245, 225, 255)),
                frame: FrameStyle::SolidRounded,
                outline: StrokeStyle {
                    color: Rgba::new(40, 40, 40, 255),
                    width_px: 2.0,
                },
                corner_px: 0.0,
                text: black(34.0),
                ..MessageWindowObject::default()
            },
        ),
    ]
}

/// The persisted user-preset document.
#[derive(Default, Serialize, Deserialize)]
struct UserPresetDoc {
    #[serde(default)]
    text: Vec<TextStylePreset>,
    #[serde(default)]
    shape: Vec<ShapeStylePreset>,
    #[serde(default)]
    window: Vec<WindowStylePreset>,
}

/// Built-in text-style presets (ids `sys:*`). Vertical + markup on, default font.
fn system_text_presets(default_font: &str) -> Vec<TextStylePreset> {
    let base = |id: &str, name: &str, color: Rgba, outline: Option<StrokeStyle>| TextStylePreset {
        id: id.to_string(),
        name: name.to_string(),
        font_key: default_font.to_string(),
        size_px: 40.0,
        color,
        orientation: Orientation::Vertical,
        align: TextAlign::Center,
        line_gap: 0.0,
        letter_gap: 0.0,
        outline,
        auto_tcy: true,
        markup_enabled: true,
    };
    vec![
        base(
            "sys:text_white",
            "白フチ",
            Rgba::WHITE,
            Some(StrokeStyle {
                color: Rgba::BLACK,
                width_px: 4.0,
            }),
        ),
        base(
            "sys:text_black",
            "黒フチ",
            Rgba::BLACK,
            Some(StrokeStyle {
                color: Rgba::WHITE,
                width_px: 4.0,
            }),
        ),
        base("sys:text_plain", "フチなし黒", Rgba::BLACK, None),
        base(
            "sys:text_quiet",
            "小声グレー",
            Rgba::new(120, 120, 120, 255),
            None,
        ),
    ]
}

/// Built-in shape-style presets (ids `sys:*`), reusing the 6 BubblePreset shapes.
fn system_shape_presets() -> Vec<ShapeStylePreset> {
    BubblePreset::ALL
        .iter()
        .map(|&p| ShapeStylePreset {
            id: format!("sys:shape_{}", p.sys_slug()),
            name: p.label().to_string(),
            shape: p.shape(),
            tail_kind: p.tail_kind(),
            fill: Some(Rgba::WHITE),
            fill_opacity: 1.0,
            outline: StrokeStyle {
                color: Rgba::BLACK,
                width_px: p.outline_width(),
            },
            padding_px: 16.0,
        })
        .collect()
}

fn is_system_preset(id: &str) -> bool {
    id.starts_with("sys:")
}

/// Path to the persisted user-preset file (`…/comic_lab/presets.json`).
fn presets_path() -> PathBuf {
    if let Some(appdata) = std::env::var_os("APPDATA") {
        return PathBuf::from(appdata)
            .join("mimageviewer")
            .join("comic_lab")
            .join("presets.json");
    }
    std::env::current_dir()
        .unwrap_or_else(|_| PathBuf::from("."))
        .join("target")
        .join("comic_lab_presets.json")
}

/// Load the user presets (ignoring system entries, which are rebuilt fresh).
fn load_user_presets() -> UserPresetDoc {
    let Ok(text) = std::fs::read_to_string(presets_path()) else {
        return UserPresetDoc::default();
    };
    match serde_json::from_str::<UserPresetDoc>(&text) {
        Ok(mut doc) => {
            doc.text.retain(|p| !is_system_preset(&p.id));
            doc.shape.retain(|p| !is_system_preset(&p.id));
            doc.window.retain(|p| !is_system_preset(&p.id));
            doc
        }
        Err(_) => UserPresetDoc::default(),
    }
}

#[derive(Clone, Copy)]
enum TextPreset {
    WhiteOnBlack,
    BlackOnWhite,
    NoOutline,
}

/// Result of rendering a shared text section (本文 / フォント / セリフ style).
struct TextSectionResult {
    dirty: bool,
    /// The user clicked "フォントを選択" (caller opens the font-sample modal).
    open_font_dialog: bool,
    /// Any INDIVIDUAL style control changed (size / color / orientation / align /
    /// gaps / outline / markup / 縦中横 / legacy style preset / marker insert).
    /// The caller clears `tb.preset_link` so the preset glow turns off.
    break_link: bool,
}

/// Draw the shared text section for `tb` (bubble text or standalone text):
/// 本文 → 文字スタイルプリセット(legacy) → フォント(検索) → サイズ → 文字色 →
/// 組方向 → 縦中横 → 記法 → 行揃え → 行間/字間 → 袋文字. `sel` salts the TextEdit id.
///
/// Sets `result.break_link` whenever an INDIVIDUAL control changes, so the
/// caller clears `tb.preset_link` (the new TextStylePreset buttons are the only
/// things that SET the link; everything here is an edit that breaks it).
/// Wrap `add_contents` in a framed section with a thin colored bar on the left
/// edge (mirrors local_adjust_lab's `draw_panel_section`). Used to color-code
/// the detail tabs by category.
fn draw_section_bar<R>(
    ui: &mut egui::Ui,
    color: Color32,
    add_contents: impl FnOnce(&mut egui::Ui) -> R,
) -> R {
    let response = egui::Frame::new()
        .inner_margin(egui::Margin {
            left: 10,
            right: 2,
            top: 4,
            bottom: 4,
        })
        .show(ui, |ui| add_contents(ui));
    let rect = response.response.rect;
    let line_rect = Rect::from_min_max(
        egui::pos2(rect.left(), rect.top() + 4.0),
        egui::pos2(rect.left() + 3.0, rect.bottom() - 4.0),
    );
    ui.painter().rect_filled(line_rect, 1.5, color);
    ui.add_space(2.0);
    response.inner
}

/// A detail-tab button that always carries its category accent color (so the
/// color coding is visible even when unselected). Selected = full accent +
/// black text + white border; unselected = dimmed accent; disabled = very dim +
/// non-interactive. Returns true on click (always false when disabled).
fn prop_tab_button(ui: &mut egui::Ui, tab: PropTab, selected: bool, enabled: bool) -> bool {
    prop_tab_button_labeled(ui, tab, selected, enabled, tab.label())
}

/// Like `prop_tab_button` but with an explicit label (windows reuse the Body /
/// Tail tab slots/colors under the names 枠 / 部品).
fn prop_tab_button_labeled(
    ui: &mut egui::Ui,
    tab: PropTab,
    selected: bool,
    enabled: bool,
    label: &str,
) -> bool {
    let base = tab.color();
    let fill = if !enabled {
        base.gamma_multiply(0.12)
    } else if selected {
        base
    } else {
        base.gamma_multiply(0.40)
    };
    let text_col = if !enabled {
        Color32::from_gray(110)
    } else if selected {
        Color32::BLACK
    } else {
        Color32::from_gray(235)
    };
    let mut btn = egui::Button::new(egui::RichText::new(label).color(text_col)).fill(fill);
    if selected && enabled {
        btn = btn.stroke(egui::Stroke::new(1.5, Color32::WHITE));
    }
    ui.add_enabled(enabled, btn).clicked()
}

/// 常時表示: 本文テキスト欄 + (記法 ON 時) カーソル位置への記号挿入ボタン。
/// 記法トグルと記号セット選択は セリフ タブ側にある。
fn draw_text_body(ui: &mut egui::Ui, tb: &mut TextBlock, sel: u64) -> TextSectionResult {
    let mut dirty = false;
    let mut break_link = false;

    let text_edit_id = egui::Id::new(("comic_text_edit", sel));
    let te_out = egui::TextEdit::multiline(&mut tb.text)
        .id(text_edit_id)
        .desired_rows(3)
        .desired_width(f32::INFINITY)
        .show(ui);
    dirty |= te_out.response.changed();
    let text_sel: Option<(usize, usize)> = te_out
        .cursor_range
        .map(|r| (r.primary.index, r.secondary.index));

    // Marker-insert buttons (only when markup is enabled). Placed right under
    // the textbox so a marker pair wraps the current selection at the cursor.
    if tb.markup_enabled {
        let rules = tb.markup_rules.clone();
        ui.horizontal(|ui| {
            ui.label("記号挿入:");
            for rule in &rules {
                let dir_label = match rule.dir {
                    InlineDir::TateChuYoko => "縦中横",
                    InlineDir::Sideways => "横倒し",
                    InlineDir::Upright => "正立",
                };
                if ui
                    .button(format!("{}{} {}", rule.open, rule.close, dir_label))
                    .clicked()
                {
                    let (a, b) = text_sel.unwrap_or_else(|| {
                        let n = tb.text.chars().count();
                        (n, n)
                    });
                    let (s, e) = (a.min(b), a.max(b));
                    let caret = insert_markers(&mut tb.text, s, e, rule.open, rule.close);
                    let ctx = ui.ctx();
                    if let Some(mut st) = egui::text_edit::TextEditState::load(ctx, text_edit_id) {
                        let cc = egui::text::CCursor::new(caret);
                        st.cursor
                            .set_char_range(Some(egui::text::CCursorRange::one(cc)));
                        st.store(ctx, text_edit_id);
                    }
                    ctx.memory_mut(|m| m.request_focus(text_edit_id));
                    dirty = true;
                    break_link = true;
                }
            }
        });
    }

    TextSectionResult {
        dirty,
        open_font_dialog: false,
        break_link,
    }
}

/// 常時表示: フォント (現在値 + 絞り込み + 一覧 + 見本/追加) + サイズ。
fn draw_text_font(ui: &mut egui::Ui, tb: &mut TextBlock, _sel: u64) -> TextSectionResult {
    let mut dirty = false;
    let mut open_font_dialog = false;
    let mut break_link = false;

    // No in-panel font list (it crowded out the other parameters). The current
    // font name + a single "フォントを選択" button that opens the sample dialog;
    // the dialog also hosts "フォントファイルを開く" for picking a file.
    ui.horizontal(|ui| {
        ui.label("フォント:");
        let cur = if tb.font_key.is_empty() {
            "(既定)".to_string()
        } else {
            tb.font_key.clone()
        };
        ui.label(egui::RichText::new(cur).strong());
    });
    if ui.button("フォントを選択 (見本)").clicked() {
        open_font_dialog = true;
    }

    ui.add_space(2.0);
    if ui
        .add(egui::Slider::new(&mut tb.size_px, 6.0..=300.0).text("サイズ"))
        .changed()
    {
        dirty = true;
        break_link = true;
    }

    TextSectionResult {
        dirty,
        open_font_dialog,
        break_link,
    }
}

/// セリフ tab: スタイル quick-toggle / 文字色 / 組方向 / 縦中横 / 記法(3セット) /
/// 行揃え / 行間字間 / 袋文字。(記号挿入ボタンは本文欄に常時表示。)
fn draw_serifu_tab(ui: &mut egui::Ui, tb: &mut TextBlock) -> TextSectionResult {
    let mut dirty = false;
    let mut break_link = false;

    // 文字スタイルプリセット (legacy quick-toggles; count as edits and break
    // the TextStylePreset link).
    ui.horizontal(|ui| {
        ui.label("スタイル:");
        let mut preset: Option<TextPreset> = None;
        if ui.button("白フチ").clicked() {
            preset = Some(TextPreset::WhiteOnBlack);
        }
        if ui.button("黒フチ").clicked() {
            preset = Some(TextPreset::BlackOnWhite);
        }
        if ui.button("フチなし").clicked() {
            preset = Some(TextPreset::NoOutline);
        }
        if let Some(p) = preset {
            apply_text_preset(tb, p);
            dirty = true;
            break_link = true;
        }
    });

    ui.horizontal(|ui| {
        ui.label("文字色");
        let mut c = rgba_to_color32(tb.color);
        if ui.color_edit_button_srgba(&mut c).changed() {
            tb.color = color32_to_rgba(c);
            dirty = true;
            break_link = true;
        }
    });

    ui.horizontal(|ui| {
        ui.label("組方向");
        let mut horiz = tb.orientation == Orientation::Horizontal;
        if ui.radio(horiz, "横").clicked() {
            tb.orientation = Orientation::Horizontal;
            horiz = true;
            dirty = true;
            break_link = true;
        }
        if ui.radio(!horiz, "縦").clicked() {
            tb.orientation = Orientation::Vertical;
            dirty = true;
            break_link = true;
        }
    });

    if tb.orientation == Orientation::Vertical
        && ui.checkbox(&mut tb.auto_tcy, "数字/!? を縦中横").changed()
    {
        dirty = true;
        break_link = true;
    }

    // Marker markup toggle + set selection (the insert buttons live in the
    // always-visible body so they sit under the textbox).
    if ui
        .checkbox(
            &mut tb.markup_enabled,
            "記法を使う (記号で囲んで縦中横/横倒し)",
        )
        .changed()
    {
        dirty = true;
        break_link = true;
    }
    if tb.markup_enabled {
        // 3 selectable marker sets. Each set has 2 pairs: first = 縦中横,
        // second = 横倒し. The current set is detected by comparing the
        // open/close chars (dir is fixed by position), so it survives reloads.
        let sets: [(&str, Vec<MarkupRule>); 3] = [
            ("[ ]  { }", markup_rules_brackets()),
            ("〈 〉  《 》", markup_rules_angle()),
            ("〚 〛  〘 〙", markup_rules_white()),
        ];
        let cur_idx = sets
            .iter()
            .position(|(_, r)| marker_pairs_eq(&tb.markup_rules, r));
        let cur_label = cur_idx.map(|i| sets[i].0).unwrap_or("カスタム");
        ui.horizontal(|ui| {
            ui.label("記号セット");
            egui::ComboBox::from_id_salt("marker_set_combo")
                .selected_text(cur_label)
                .show_ui(ui, |ui| {
                    for (i, (label, rules)) in sets.iter().enumerate() {
                        if ui.selectable_label(cur_idx == Some(i), *label).clicked() {
                            tb.markup_rules = rules.clone();
                            dirty = true;
                            break_link = true;
                        }
                    }
                });
        });
        ui.label(
            egui::RichText::new(
                "先頭の記号=縦中横, 2番目=横倒し (縦書きで有効)。挿入は本文欄の下。",
            )
            .small()
            .weak(),
        );
    }

    ui.horizontal(|ui| {
        ui.label("行揃え");
        for (label, val) in [
            ("始", TextAlign::Start),
            ("中", TextAlign::Center),
            ("終", TextAlign::End),
        ] {
            if ui.radio(tb.align == val, label).clicked() {
                tb.align = val;
                dirty = true;
                break_link = true;
            }
        }
    });

    if ui
        .add(egui::Slider::new(&mut tb.line_gap, -20.0..=80.0).text("行間"))
        .changed()
    {
        dirty = true;
        break_link = true;
    }
    if ui
        .add(egui::Slider::new(&mut tb.letter_gap, -10.0..=60.0).text("字間"))
        .changed()
    {
        dirty = true;
        break_link = true;
    }

    let mut has_outline = tb.outline.is_some();
    if ui.checkbox(&mut has_outline, "袋文字 (縁取り)").changed() {
        tb.outline = if has_outline {
            Some(StrokeStyle {
                color: Rgba::BLACK,
                width_px: 3.0,
            })
        } else {
            None
        };
        dirty = true;
        break_link = true;
    }
    if let Some(stroke) = &mut tb.outline {
        ui.horizontal(|ui| {
            ui.label("縁取り色");
            let mut c = rgba_to_color32(stroke.color);
            if ui.color_edit_button_srgba(&mut c).changed() {
                stroke.color = color32_to_rgba(c);
                dirty = true;
                break_link = true;
            }
        });
        if ui
            .add(egui::Slider::new(&mut stroke.width_px, 0.0..=20.0).text("縁取り太さ"))
            .changed()
        {
            dirty = true;
            break_link = true;
        }
    }

    TextSectionResult {
        dirty,
        open_font_dialog: false,
        break_link,
    }
}

fn apply_text_preset(tb: &mut TextBlock, preset: TextPreset) {
    match preset {
        TextPreset::WhiteOnBlack => {
            tb.color = Rgba::WHITE;
            tb.outline = Some(StrokeStyle {
                color: Rgba::BLACK,
                width_px: 4.0,
            });
        }
        TextPreset::BlackOnWhite => {
            tb.color = Rgba::BLACK;
            tb.outline = Some(StrokeStyle {
                color: Rgba::WHITE,
                width_px: 4.0,
            });
        }
        TextPreset::NoOutline => {
            tb.outline = None;
        }
    }
}

/// Recent-file history (mirrors local_adjust_lab's convention).
#[derive(Default, serde::Serialize, serde::Deserialize)]
struct LabHistory {
    #[serde(default)]
    recent_files: Vec<PathBuf>,
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
            .join("comic_lab")
            .join("recent_files.json");
    }
    std::env::current_dir()
        .unwrap_or_else(|_| PathBuf::from("."))
        .join("target")
        .join("comic_lab_recent_files.json")
}

#[cfg(test)]
mod sample_tests {
    use super::*;

    /// The shipped verification scene (docs/comic-lab-sample-scene.comic.json)
    /// must keep loading through the lab's real `SidecarDoc` path, with the IVS /
    /// combining-mark codepoints intact (regenerate via scripts/gen_comic_sample.py).
    #[test]
    fn sample_scene_loads() {
        let path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../docs/comic-lab-sample-scene.comic.json"
        );
        let text = std::fs::read_to_string(path).expect("sample scene present");
        let doc: SidecarDoc = serde_json::from_str(&text).expect("sample scene parses");
        assert_eq!(doc.schema_version, 1);
        assert_eq!(doc.objects.len(), 5, "5 demo blocks");
        // Every block is vertical standalone text.
        for o in &doc.objects {
            match &o.kind {
                AnnotationKind::Text(t) => {
                    assert_eq!(t.orientation, Orientation::Vertical)
                }
                other => panic!("unexpected kind: {other:?}"),
            }
        }
        let all: String = doc
            .objects
            .iter()
            .filter_map(|o| o.text_block().map(|t| t.text.clone()))
            .collect();
        assert!(all.contains('\u{E0100}'), "IVS selector present");
        assert!(all.contains('\u{3099}'), "combining dakuten present");
    }

    #[test]
    fn onomatopoeia_preset_builds_text_object_style() {
        let preset = OnomatopoeiaPreset::ALL
            .iter()
            .copied()
            .find(|p| p.font_candidate() == "Otomanopee One")
            .expect("Otomanopee preset");
        let tb = preset.build_text("OtomanopeeOne Regular");
        assert_eq!(tb.text, "ドンッ");
        assert_eq!(tb.font_key, "OtomanopeeOne Regular");
        assert_eq!(tb.orientation, Orientation::Horizontal);
        assert_eq!(tb.align, TextAlign::Center);
        assert!(!tb.markup_enabled);
        assert_eq!(tb.outline.expect("impact outline").width_px, 8.0);
        assert!(preset.rotation_rad().abs() > 0.01);
    }

    #[test]
    fn onomatopoeia_presets_cover_bundled_font_samples() {
        assert!(OnomatopoeiaPreset::ALL.len() >= 18);
        for preset in OnomatopoeiaPreset::ALL {
            assert!(!preset.label().is_empty());
            assert!(!preset.text().is_empty());
            assert!(!preset.font_candidate().is_empty());
        }
    }

    #[test]
    fn font_candidate_matching_ignores_separators() {
        assert!(font_name_matches_candidate(
            "DelaGothicOne-Regular",
            "Dela Gothic One"
        ));
        assert!(font_name_matches_candidate(
            "OtomanopeeOne Regular",
            "Otomanopee One"
        ));
        assert!(!font_name_matches_candidate("Yu Gothic", "Dela Gothic One"));
    }
}
