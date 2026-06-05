//! Data model for speech-bubble / text annotation objects.
//!
//! Mirrors `docs/speech-bubble-text-tool-plan.md` §4. Every type is
//! serde Serialize/Deserialize so the lab can persist a `<image>.comic.json`
//! sidecar, analogous to local_adjust_lab's `.miv` sidecar.
//!
//! Coordinates are in **source image-pixel space** (the lab's background image
//! resolution). `#[serde(default)]` is used liberally so older sidecars keep
//! loading after fields are added (forward-compat per Codex design §9).

use serde::{Deserialize, Serialize};

/// Straight-alpha RGBA color.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Rgba {
    pub r: u8,
    pub g: u8,
    pub b: u8,
    pub a: u8,
}

impl Rgba {
    pub const BLACK: Rgba = Rgba {
        r: 0,
        g: 0,
        b: 0,
        a: 255,
    };
    pub const WHITE: Rgba = Rgba {
        r: 255,
        g: 255,
        b: 255,
        a: 255,
    };
    pub const TRANSPARENT: Rgba = Rgba {
        r: 0,
        g: 0,
        b: 0,
        a: 0,
    };

    pub const fn new(r: u8, g: u8, b: u8, a: u8) -> Self {
        Rgba { r, g, b, a }
    }
}

/// Outline / stroke styling (color + width in pixels).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct StrokeStyle {
    pub color: Rgba,
    pub width_px: f32,
}

impl Default for StrokeStyle {
    fn default() -> Self {
        StrokeStyle {
            color: Rgba::BLACK,
            width_px: 2.0,
        }
    }
}

/// Text writing direction.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Orientation {
    /// Left-to-right, lines stacked top-to-bottom (横書き).
    Horizontal,
    /// Glyphs stacked top-to-bottom; columns advance right-to-left (縦書き).
    Vertical,
}

impl Default for Orientation {
    fn default() -> Self {
        Orientation::Horizontal
    }
}

/// Alignment along the line (horizontal) or column (vertical).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum TextAlign {
    /// Left (horizontal) / top (vertical).
    Start,
    Center,
    /// Right (horizontal) / bottom (vertical).
    End,
}

impl Default for TextAlign {
    fn default() -> Self {
        TextAlign::Start
    }
}

/// Forced inline writing direction for a marked run (縦書き only). Selected via
/// the marker-markup parser (see `layout.rs`) — no rich-text ranges.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum InlineDir {
    /// 縦中横: pack the run horizontally into one vertical cell.
    TateChuYoko,
    /// 横倒し: rotate the run 90° and stack the rotated glyphs down the column.
    Sideways,
    /// 正立: force every char upright (one cell each), even digits.
    Upright,
}

/// One marker-markup rule: chars between `open` and `close` get `dir`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct MarkupRule {
    pub open: char,
    pub close: char,
    pub dir: InlineDir,
}

/// Build a 2-pair rule set: `tcy` pair = 縦中横, `side` pair = 横倒し.
fn two_pair_rules(tcy: (char, char), side: (char, char)) -> Vec<MarkupRule> {
    vec![
        MarkupRule {
            open: tcy.0,
            close: tcy.1,
            dir: InlineDir::TateChuYoko,
        },
        MarkupRule {
            open: side.0,
            close: side.1,
            dir: InlineDir::Sideways,
        },
    ]
}

/// Marker set A (default, half-width): `[..]` = 縦中横, `{..}` = 横倒し.
pub fn markup_rules_brackets() -> Vec<MarkupRule> {
    two_pair_rules(('[', ']'), ('{', '}'))
}

/// Marker set B (angle): `〈..〉` = 縦中横, `《..》` = 横倒し.
pub fn markup_rules_angle() -> Vec<MarkupRule> {
    two_pair_rules(('〈', '〉'), ('《', '》'))
}

/// Marker set C (white brackets): `〚..〛` = 縦中横, `〘..〙` = 横倒し.
pub fn markup_rules_white() -> Vec<MarkupRule> {
    two_pair_rules(('〚', '〛'), ('〘', '〙'))
}

/// Default marker set: `[..]` = 縦中横, `{..}` = 横倒し.
pub fn default_markup_rules() -> Vec<MarkupRule> {
    markup_rules_brackets()
}

/// A run of text with a single style. Phase 1 keeps a single TextBlock per
/// annotation; per-run styling (ruby) is `// TODO(phase3):`. Manual inline
/// orientation (縦中横 / 横倒し / 正立) is expressed through marker markup
/// (`markup_enabled` + `markup_rules`) parsed straight from `text`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TextBlock {
    pub text: String,
    /// Logical font name (the FontSet maps this to a LoadedFont).
    #[serde(default)]
    pub font_key: String,
    pub size_px: f32,
    pub color: Rgba,
    #[serde(default)]
    pub orientation: Orientation,
    #[serde(default)]
    pub align: TextAlign,
    /// Extra spacing between lines/columns, added to the font's natural advance.
    #[serde(default)]
    pub line_gap: f32,
    /// Extra spacing between glyphs along the line/column.
    #[serde(default)]
    pub letter_gap: f32,
    /// 袋文字 (outline / halo) drawn behind the glyph fill for readability.
    #[serde(default)]
    pub outline: Option<StrokeStyle>,
    #[serde(default)]
    pub bold: bool,
    #[serde(default)]
    pub italic: bool,
    /// Auto 縦中横 for short half-width runs (2–3 digits, `!?`/`?!`) in vertical
    /// mode. Pure punctuation runs (`!!!` …) always stack upright.
    #[serde(default = "yes")]
    pub auto_tcy: bool,
    /// When true, marker characters in `text` (per `markup_rules`) force the
    /// inline orientation of the wrapped run. When false, markers are literal.
    #[serde(default)]
    pub markup_enabled: bool,
    /// Marker-pair → inline-direction rules. See `default_markup_rules`.
    #[serde(default = "default_markup_rules")]
    pub markup_rules: Vec<MarkupRule>,
    /// Opaque id of the text-style preset this block is linked to (if any).
    /// `comic-core` does NOT interpret this value — the UI layer assigns/clears
    /// it (apply a preset → `Some(id)`, edit any control → `None`). It rides
    /// along in the object so undo/redo + sidecar persist the link for free.
    #[serde(default)]
    pub preset_link: Option<String>,
}

impl Default for TextBlock {
    fn default() -> Self {
        TextBlock {
            text: String::new(),
            font_key: String::new(),
            size_px: 48.0,
            color: Rgba::BLACK,
            orientation: Orientation::Horizontal,
            align: TextAlign::Start,
            line_gap: 0.0,
            letter_gap: 0.0,
            outline: None,
            bold: false,
            italic: false,
            auto_tcy: true,
            markup_enabled: false,
            markup_rules: default_markup_rules(),
            preset_link: None,
        }
    }
}

/// Bubble container shape. Rect == RoundRect with corner_px == 0.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub enum BubbleShape {
    Ellipse {
        rx: f32,
        ry: f32,
    },
    RoundRect {
        half_w: f32,
        half_h: f32,
        corner_px: f32,
    },
    /// Spiky / burst balloon (叫び・強調). `spikes` outer points; `jag` is the
    /// inner radius ratio (0..1). `shape_seed` drives the deterministic
    /// per-spike jitter that gives the hand-drawn manga-explosion look.
    Burst {
        rx: f32,
        ry: f32,
        spikes: u32,
        jag: f32,
        #[serde(default)]
        shape_seed: u32,
    },
    /// Cloud / thought balloon (思考・もくもく). `lobes` bumps around the
    /// perimeter; `amp` is the bump depth ratio (0..0.5). `shape_seed` adds a
    /// subtle per-lobe jitter.
    Cloud {
        rx: f32,
        ry: f32,
        lobes: u32,
        amp: f32,
        #[serde(default)]
        shape_seed: u32,
    },
    /// Regular polygon (多角形) inscribed in the rx/ry ellipse. `sides` >= 3.
    Polygon {
        rx: f32,
        ry: f32,
        sides: u32,
    },
    /// Diamond / rhombus (ダイヤ) through the four axis points.
    Diamond {
        half_w: f32,
        half_h: f32,
    },
    /// Heart (ハート) parametric curve fitted to rx/ry.
    Heart {
        rx: f32,
        ry: f32,
    },
    /// Arrow (矢印): a shaft + head pointing toward `dir_rad` (0 = +x / right,
    /// -PI/2 = up), sized by half_w (length axis) / half_h (cross axis).
    Arrow {
        half_w: f32,
        half_h: f32,
        #[serde(default = "arrow_up")]
        dir_rad: f32,
    },
    /// Soft / やわらか balloon: a rounded rect with gently wavy edges.
    /// `corner_px` rounds the corners; `shape_seed` jitters the wave phase.
    Soft {
        half_w: f32,
        half_h: f32,
        #[serde(default = "soft_corner")]
        corner_px: f32,
        #[serde(default)]
        shape_seed: u32,
    },
    /// 集中線 (concentration / motion lines): `count` tapered lines radiating from
    /// a clear central ellipse outward to the rx/ry extent. Renders as a line
    /// field (no fill/stroke); text sits in the clear center.
    MotionLines {
        rx: f32,
        ry: f32,
        #[serde(default = "lines_count")]
        count: u32,
        #[serde(default)]
        shape_seed: u32,
    },
    /// 流線 (speed lines): `count` parallel tapered lines in `dir_rad`, across the
    /// half_w/half_h extent, skipping a clear central ellipse for text.
    SpeedLines {
        half_w: f32,
        half_h: f32,
        #[serde(default)]
        dir_rad: f32,
        #[serde(default = "lines_count")]
        count: u32,
        #[serde(default)]
        shape_seed: u32,
    },
    /// なし (text-only container): no fill / no stroke / no tail — just the
    /// centered text. `half_w`/`half_h` define the movable & selectable region and
    /// the auto-size box. Lets the user place free text that can later be switched
    /// to any other shape (it rides the bubble pipeline, unlike a Text object).
    TextOnly {
        half_w: f32,
        half_h: f32,
    },
    /// 意識 (concentration / awareness): a soft, fuzzy-edged ellipse drawn with a
    /// feathered fill rim + a soft outline ring (no hard edge), evoking an inner
    /// monologue / dazed feeling. `shape_seed` jitters the rim wobble.
    Concentration {
        rx: f32,
        ry: f32,
        #[serde(default)]
        shape_seed: u32,
    },
    /// 線 (sketchy strokes): a rounded rect whose outline is drawn with several
    /// hand-drawn, jittered passes (rough / pencil look). Fill works normally
    /// inside. `shape_seed` drives the deterministic per-pass jitter.
    Strokes {
        half_w: f32,
        half_h: f32,
        #[serde(default = "soft_corner")]
        corner_px: f32,
        #[serde(default)]
        shape_seed: u32,
    },
    /// 二重線 (double stroke): a rounded rect with two concentric outlines `gap_px`
    /// apart (the inner ring is body-only; a tail keeps a single line).
    DoubleStroke {
        half_w: f32,
        half_h: f32,
        #[serde(default = "soft_corner")]
        corner_px: f32,
        #[serde(default = "double_gap")]
        gap_px: f32,
    },
}

fn lines_count() -> u32 {
    64
}

fn arrow_up() -> f32 {
    -std::f32::consts::FRAC_PI_2
}
fn soft_corner() -> f32 {
    28.0
}
fn double_gap() -> f32 {
    8.0
}

impl Default for BubbleShape {
    fn default() -> Self {
        BubbleShape::Ellipse {
            rx: 160.0,
            ry: 100.0,
        }
    }
}

/// Tail rendering style.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum TailKind {
    /// Solid triangular spike spliced into the bubble outline (会話用).
    Spike,
    /// Trail of shrinking circles toward the speaker (思考用).
    Thought,
}

impl Default for TailKind {
    fn default() -> Self {
        TailKind::Spike
    }
}

/// A tail leaving the bubble toward a speaker. `base_t` is a 0..1 parameter for
/// where it leaves the outline; `kind` selects spike vs thought-circles.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Tail {
    /// Tail tip in source-image coordinates (relative to the object pivot is
    /// resolved at bake time — the lab stores absolute image coords here).
    pub tip: (f32, f32),
    /// Where the tail base attaches along the bubble outline (0..1). Ignored
    /// when `base_auto` is true (the base is then derived from the
    /// centroid→tip ray).
    pub base_t: f32,
    /// When true, the base attaches where the ray from the bubble center toward
    /// `tip` exits the outline (the natural "pointing at the speaker" position).
    /// Dragging the base handle clears this and pins `base_t`.
    #[serde(default = "yes")]
    pub base_auto: bool,
    /// Width of the tail base; it tapers to a point at the tip.
    pub width_px: f32,
    #[serde(default)]
    pub kind: TailKind,
}

impl Default for Tail {
    fn default() -> Self {
        Tail {
            tip: (0.0, 0.0),
            base_t: 0.5,
            base_auto: true,
            width_px: 40.0,
            kind: TailKind::Spike,
        }
    }
}

/// Procedural decoration kind placed along/around the bubble outline.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum DecoKind {
    /// 4/8-point star / cross sparkle (きらきら).
    Sparkle,
    /// 5 rounded petals around a center (花).
    Flower,
    /// Small filled + stroked circle (泡).
    Bubble,
}

impl Default for DecoKind {
    fn default() -> Self {
        DecoKind::Sparkle
    }
}

/// Where a decoration layer is placed relative to the bubble body outline.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum DecoPlacement {
    /// On the sampled outline points.
    Outline,
    /// Along the outward normal from the outline.
    Outside,
    /// Along the inward normal from the outline.
    Inside,
    /// Clustered near the tail base.
    Tail,
}

impl Default for DecoPlacement {
    fn default() -> Self {
        DecoPlacement::Outline
    }
}

/// One procedural decoration layer for a bubble.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct DecorationLayer {
    pub kind: DecoKind,
    pub placement: DecoPlacement,
    /// Items per ~100px of outline arc-length.
    pub density: f32,
    /// Decoration size as a ratio of the bubble's short side.
    pub size_ratio: f32,
    pub color: Rgba,
    /// Deterministic PRNG seed for position/size/rotation jitter.
    pub seed: u32,
    /// Outline width in px for each decoration (0 = no outline).
    #[serde(default = "deco_outline_width")]
    pub outline_width: f32,
    /// Outline color for each decoration.
    #[serde(default = "deco_outline_color")]
    pub outline_color: Rgba,
    /// Center dot color for the Flower kind.
    #[serde(default = "deco_center_color")]
    pub center_color: Rgba,
    /// Number of star points for the Sparkle kind (3..).
    #[serde(default = "deco_points")]
    pub points: u32,
    /// Number of petals for the Flower kind (3..).
    #[serde(default = "deco_petals")]
    pub petals: u32,
    /// For the Bubble kind: draw a translucent radial gradient + highlight
    /// (a soapy 泡 look) instead of a flat filled circle.
    #[serde(default)]
    pub gradient: bool,
}

fn deco_outline_width() -> f32 {
    2.0
}
fn deco_outline_color() -> Rgba {
    Rgba::new(40, 30, 20, 255)
}
fn deco_center_color() -> Rgba {
    Rgba::new(255, 230, 120, 255)
}
fn deco_points() -> u32 {
    4
}
fn deco_petals() -> u32 {
    5
}

impl Default for DecorationLayer {
    fn default() -> Self {
        DecorationLayer {
            kind: DecoKind::Sparkle,
            placement: DecoPlacement::Outside,
            density: 3.0,
            size_ratio: 0.18,
            color: Rgba::new(255, 220, 80, 255),
            seed: 0,
            outline_width: deco_outline_width(),
            outline_color: deco_outline_color(),
            center_color: deco_center_color(),
            points: deco_points(),
            petals: deco_petals(),
            gradient: false,
        }
    }
}

/// A bubble = container shape + embedded text.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BubbleObject {
    pub shape: BubbleShape,
    /// `None` = no fill (transparent interior).
    #[serde(default)]
    pub fill: Option<Rgba>,
    #[serde(default = "one")]
    pub fill_opacity: f32,
    #[serde(default)]
    pub outline: StrokeStyle,
    #[serde(default)]
    pub tail: Option<Tail>,
    #[serde(default)]
    pub padding_px: f32,
    /// Procedural decorations (星 / 花 / 泡) placed along the outline.
    #[serde(default)]
    pub decorations: Vec<DecorationLayer>,
    #[serde(default)]
    pub text: TextBlock,
    /// When true, the shape's dimensions (rx/ry or half_w/half_h) are derived
    /// from the embedded text's layout bounds + `padding_px` at render time
    /// (see `tessellate::fit_bubble_shape`). Manual size edits clear this.
    #[serde(default = "yes")]
    pub auto_size: bool,
    /// When true, this bubble fuses with the bubble directly below it in z-order
    /// into one merged outline (interior strokes are painted over). Lets two
    /// overlapping bubbles read as a single balloon.
    #[serde(default)]
    pub merge_with_below: bool,
    /// Opaque id of the shape-style preset this bubble is linked to (if any).
    /// Same contract as `TextBlock::preset_link`: comic-core only stores it;
    /// the UI sets it on apply and clears it on any individual edit.
    #[serde(default)]
    pub shape_preset_link: Option<String>,
}

fn one() -> f32 {
    1.0
}

impl Default for BubbleObject {
    fn default() -> Self {
        BubbleObject {
            shape: BubbleShape::default(),
            fill: Some(Rgba::WHITE),
            fill_opacity: 1.0,
            outline: StrokeStyle::default(),
            tail: None,
            padding_px: 16.0,
            decorations: Vec::new(),
            text: TextBlock::default(),
            auto_size: true,
            merge_with_below: false,
            shape_preset_link: None,
        }
    }
}

// =====================================================================
// Message window (dialogue box) — DQ/FF JRPG / VN / social-game style.
// A screen-space rectangular panel with flowed text, unlike a balloon.
// See docs/message-window-design.md. Coordinates are source-image space;
// `pivot` is the panel CENTER (position presets are resolved to a pivot by
// the lab/app, which knows the image size — comic-core stays size-agnostic).
// =====================================================================

/// Panel frame rendering. `NineSlice` (image-based) is reserved for the future;
/// v1 draws frames procedurally.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum FrameStyle {
    /// No frame line.
    None,
    /// A single rounded-rect outline.
    #[default]
    SolidRounded,
    /// Two concentric outlines `frame_gap_px` apart (JRPG / FF look).
    DoubleLine,
    // Future: NineSlice (user image + borders).
}

/// Panel background fill. `frame` (the border) is orthogonal to this.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum FillMode {
    /// No background (text over the raw image).
    None,
    /// Flat color (alpha = `fill.a * fill_opacity`).
    #[default]
    Solid,
    /// Flat color with a low default opacity (semantic alias of Solid; the UI
    /// presets a translucent alpha).
    Translucent,
    /// Gradient that fades the fill color to transparent toward the side
    /// OPPOSITE `scrim_dense_side` (frameless ADV/NVL bottom scrim).
    GradientScrim,
    /// Two-color linear gradient `fill` -> `gradient_to` across the panel
    /// (top->bottom), both fully opaque-ish (FF-like panel).
    LinearGradient,
}

/// Screen position preset. The lab resolves this (+ size) into the pivot.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum WindowPosition {
    Top,
    Middle,
    #[default]
    Bottom,
    Center,
    /// Free placement (the user dragged it; the lab leaves the pivot as-is).
    Free,
}

/// How the panel's width/height are determined.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum SizeMode {
    /// Image width minus `margin_px` on each side; height from `half_h`.
    #[default]
    FullWidth,
    /// Centered fixed size from `half_w` / `half_h`.
    Inset,
    /// Fit the panel to the (un-wrapped) text + padding (no wrapping).
    AutoFitText,
}

/// Vertical anchoring of text within the content rect, and the dense edge of a
/// gradient scrim.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum VAnchor {
    #[default]
    Top,
    Center,
    Bottom,
}

/// Speaker name-plate placement.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum NamePlateMode {
    #[default]
    None,
    /// Plain label inside the panel's top-left (no box).
    Inline,
    /// Boxed label inside the panel's top-left.
    Boxed,
    /// Separate box overlapping/above the panel's top edge (uses `offset`,
    /// typically a negative y so it sits above the panel).
    Above,
}

/// Portrait/face placeholder slot side. v1 draws a colored placeholder rect;
/// real image import is deferred.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum PortraitSide {
    #[default]
    None,
    Left,
    Right,
}

/// "Continue / next" indicator glyph (drawn as a baked polygon, not a font
/// glyph, so it never depends on font coverage).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum IndicatorKind {
    #[default]
    None,
    Triangle,
    Chevron,
    Diamond,
    Dots,
}

/// Per-side insets (content padding).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Insets {
    pub left: f32,
    pub top: f32,
    pub right: f32,
    pub bottom: f32,
}

impl Insets {
    pub fn uniform(v: f32) -> Self {
        Insets {
            left: v,
            top: v,
            right: v,
            bottom: v,
        }
    }
}

impl Default for Insets {
    fn default() -> Self {
        Insets::uniform(0.0)
    }
}

/// Simple offset drop shadow (no blur in v1).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct ShadowStyle {
    pub color: Rgba,
    pub offset: (f32, f32),
}

impl Default for ShadowStyle {
    fn default() -> Self {
        ShadowStyle {
            color: Rgba::new(0, 0, 0, 110),
            offset: (6.0, 6.0),
        }
    }
}

/// Speaker name plate (text + its own little panel).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NamePlate {
    #[serde(default)]
    pub mode: NamePlateMode,
    #[serde(default = "name_text")]
    pub name: TextBlock,
    #[serde(default = "np_fill")]
    pub fill: Option<Rgba>,
    #[serde(default = "np_outline")]
    pub outline: StrokeStyle,
    #[serde(default = "np_corner")]
    pub corner_px: f32,
    #[serde(default = "np_padding")]
    pub padding_px: f32,
    /// Offset of the plate's top-left from the panel's top-left (negative y puts
    /// it above the panel, Ren'Py-style).
    #[serde(default)]
    pub offset: (f32, f32),
}

fn name_text() -> TextBlock {
    TextBlock {
        size_px: 30.0,
        color: Rgba::WHITE,
        ..TextBlock::default()
    }
}
fn np_fill() -> Option<Rgba> {
    Some(Rgba::new(30, 32, 44, 255))
}
fn np_outline() -> StrokeStyle {
    StrokeStyle {
        color: Rgba::WHITE,
        width_px: 2.0,
    }
}
fn np_corner() -> f32 {
    6.0
}
fn np_padding() -> f32 {
    8.0
}

impl Default for NamePlate {
    fn default() -> Self {
        NamePlate {
            mode: NamePlateMode::None,
            name: name_text(),
            fill: np_fill(),
            outline: np_outline(),
            corner_px: np_corner(),
            padding_px: np_padding(),
            offset: (0.0, 0.0),
        }
    }
}

/// Portrait/face placeholder slot.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct PortraitSlot {
    #[serde(default)]
    pub side: PortraitSide,
    #[serde(default = "portrait_w")]
    pub width_px: f32,
    #[serde(default = "portrait_fill")]
    pub fill: Option<Rgba>,
    #[serde(default)]
    pub outline: StrokeStyle,
    #[serde(default = "portrait_margin")]
    pub margin_px: f32,
}

fn portrait_w() -> f32 {
    200.0
}
fn portrait_fill() -> Option<Rgba> {
    Some(Rgba::new(70, 74, 92, 255))
}
fn portrait_margin() -> f32 {
    12.0
}

impl Default for PortraitSlot {
    fn default() -> Self {
        PortraitSlot {
            side: PortraitSide::None,
            width_px: portrait_w(),
            fill: portrait_fill(),
            outline: StrokeStyle::default(),
            margin_px: portrait_margin(),
        }
    }
}

/// A message window = panel (fill + frame + shadow) + flowed text + optional
/// name plate / portrait slot / continue indicator.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MessageWindowObject {
    #[serde(default)]
    pub size_mode: SizeMode,
    #[serde(default)]
    pub position: WindowPosition,
    /// Panel half-extents (used by Inset; FullWidth uses `half_h` for height and
    /// derives `half_w` in the lab; AutoFitText derives both from the text).
    #[serde(default = "win_half_w")]
    pub half_w: f32,
    #[serde(default = "win_half_h")]
    pub half_h: f32,
    /// Left/right margin for FullWidth (resolved against the image by the lab).
    #[serde(default = "win_margin")]
    pub margin_px: f32,
    #[serde(default = "win_corner")]
    pub corner_px: f32,

    #[serde(default)]
    pub fill_mode: FillMode,
    #[serde(default = "win_fill")]
    pub fill: Option<Rgba>,
    #[serde(default = "one")]
    pub fill_opacity: f32,
    /// Second color for `FillMode::LinearGradient`.
    #[serde(default)]
    pub gradient_to: Option<Rgba>,
    /// Dense edge of a `GradientScrim` (default Bottom = classic下部スクリム).
    #[serde(default = "vanchor_bottom")]
    pub scrim_dense_side: VAnchor,

    #[serde(default)]
    pub frame: FrameStyle,
    #[serde(default = "win_outline")]
    pub outline: StrokeStyle,
    #[serde(default = "win_frame_gap")]
    pub frame_gap_px: f32,
    #[serde(default)]
    pub shadow: Option<ShadowStyle>,

    #[serde(default = "win_text")]
    pub text: TextBlock,
    #[serde(default = "win_padding")]
    pub padding: Insets,
    #[serde(default)]
    pub v_anchor: VAnchor,
    /// Word-wrap the body to the content width (with Japanese kinsoku). Ignored
    /// in `AutoFitText` mode (which sizes to the un-wrapped text).
    #[serde(default = "yes")]
    pub wrap: bool,

    #[serde(default)]
    pub name_plate: NamePlate,
    #[serde(default)]
    pub portrait: PortraitSlot,
    #[serde(default)]
    pub indicator: IndicatorKind,
    /// When true, the continue indicator is drawn ONLY when the body text
    /// overflows the content rect (the realistic "there's more text" cue). When
    /// false it's always drawn (if `indicator != None`).
    #[serde(default)]
    pub indicator_auto: bool,

    /// Opaque id of the window-style preset this object is linked to (same
    /// contract as `BubbleObject::shape_preset_link`).
    #[serde(default)]
    pub style_preset_link: Option<String>,
}

fn win_half_w() -> f32 {
    480.0
}
fn win_half_h() -> f32 {
    120.0
}
fn win_margin() -> f32 {
    48.0
}
fn win_corner() -> f32 {
    14.0
}
fn win_frame_gap() -> f32 {
    6.0
}
fn win_fill() -> Option<Rgba> {
    Some(Rgba::new(18, 22, 48, 235))
}
fn win_outline() -> StrokeStyle {
    StrokeStyle {
        color: Rgba::WHITE,
        width_px: 3.0,
    }
}
fn win_padding() -> Insets {
    Insets::uniform(28.0)
}
fn vanchor_bottom() -> VAnchor {
    VAnchor::Bottom
}
fn win_text() -> TextBlock {
    TextBlock {
        color: Rgba::WHITE,
        ..TextBlock::default()
    }
}

impl Default for MessageWindowObject {
    fn default() -> Self {
        MessageWindowObject {
            size_mode: SizeMode::default(),
            position: WindowPosition::default(),
            half_w: win_half_w(),
            half_h: win_half_h(),
            margin_px: win_margin(),
            corner_px: win_corner(),
            fill_mode: FillMode::default(),
            fill: win_fill(),
            fill_opacity: 1.0,
            gradient_to: None,
            scrim_dense_side: vanchor_bottom(),
            frame: FrameStyle::default(),
            outline: win_outline(),
            frame_gap_px: win_frame_gap(),
            shadow: None,
            text: win_text(),
            padding: win_padding(),
            v_anchor: VAnchor::Top,
            wrap: true,
            name_plate: NamePlate::default(),
            portrait: PortraitSlot::default(),
            indicator: IndicatorKind::None,
            indicator_auto: false,
            style_preset_link: None,
        }
    }
}

// =====================================================================
// Stamp (image sticker) — a 4th annotation kind. See docs/stamp-feature-
// design.md. comic-core stays decode-free: the LAB resolves a StampSource into
// pre-rasterized straight-alpha RGBA at display size and hands it to the baker
// via a `HashMap<object id, RgbaOverlay>`. comic-core only composites it
// (scale-to-fit, flip, opacity, sticker outline, object rotation).
// =====================================================================

/// Where a stamp's pixels come from. comic-core treats this as opaque data; the
/// lab/app resolves it into RGBA. `Emoji` is a key into a bundled emoji set
/// (e.g. a hyphen-joined codepoint string like `"1f600"` / `"1f1ef-1f1f5"`);
/// `File` is a user image path.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum StampSource {
    Emoji(String),
    File(std::path::PathBuf),
}

impl Default for StampSource {
    fn default() -> Self {
        StampSource::Emoji(String::new())
    }
}

/// An image sticker placed as an annotation. Geometry (pivot/rotation/z/enabled)
/// rides on the enclosing `AnnotationObject`; the stamp keeps its own size,
/// opacity, flips and optional sticker outline. The aspect ratio is the source
/// image's; the UI keeps `half_w`/`half_h` consistent with it (uniform scale).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StampObject {
    pub source: StampSource,
    /// On-canvas half-width (px). `half_h` follows the source aspect.
    pub half_w: f32,
    pub half_h: f32,
    #[serde(default = "one")]
    pub opacity: f32,
    #[serde(default)]
    pub flip_h: bool,
    #[serde(default)]
    pub flip_v: bool,
    /// Sticker-style outline (white border etc.): the silhouette alpha is dilated
    /// and laid behind the image in this color.
    #[serde(default)]
    pub outline: Option<StrokeStyle>,
    /// Opaque id of a stamp-style preset this is linked to (same contract as
    /// `TextBlock::preset_link`); comic-core only stores it.
    #[serde(default)]
    pub style_preset_link: Option<String>,
}

impl Default for StampObject {
    fn default() -> Self {
        StampObject {
            source: StampSource::default(),
            half_w: 96.0,
            half_h: 96.0,
            opacity: 1.0,
            flip_h: false,
            flip_v: false,
            outline: None,
            style_preset_link: None,
        }
    }
}

/// A bubble, a standalone text block, a message window, or an image stamp.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum AnnotationKind {
    Bubble(BubbleObject),
    Text(TextBlock),
    MessageWindow(MessageWindowObject),
    Stamp(StampObject),
}

/// One annotation object. Coords (pivot, tail tip) are source-image space.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AnnotationObject {
    pub id: u64,
    #[serde(default = "yes")]
    pub enabled: bool,
    #[serde(default)]
    pub z: i32,
    /// Object anchor in source-image coordinates.
    pub pivot: (f32, f32),
    #[serde(default)]
    pub rotation_rad: f32,
    pub kind: AnnotationKind,
}

fn yes() -> bool {
    true
}

impl AnnotationObject {
    pub fn new_text(id: u64, pivot: (f32, f32), text: TextBlock) -> Self {
        AnnotationObject {
            id,
            enabled: true,
            z: 0,
            pivot,
            rotation_rad: 0.0,
            kind: AnnotationKind::Text(text),
        }
    }

    pub fn new_bubble(id: u64, pivot: (f32, f32), bubble: BubbleObject) -> Self {
        AnnotationObject {
            id,
            enabled: true,
            z: 0,
            pivot,
            rotation_rad: 0.0,
            kind: AnnotationKind::Bubble(bubble),
        }
    }

    pub fn new_message_window(id: u64, pivot: (f32, f32), window: MessageWindowObject) -> Self {
        AnnotationObject {
            id,
            enabled: true,
            z: 0,
            pivot,
            rotation_rad: 0.0,
            kind: AnnotationKind::MessageWindow(window),
        }
    }

    pub fn new_stamp(id: u64, pivot: (f32, f32), stamp: StampObject) -> Self {
        AnnotationObject {
            id,
            enabled: true,
            z: 0,
            pivot,
            rotation_rad: 0.0,
            kind: AnnotationKind::Stamp(stamp),
        }
    }

    /// Convenience accessor to the embedded TextBlock (bubble / message-window
    /// body text, or standalone text). `None` for kinds without text (Stamp).
    pub fn text_block(&self) -> Option<&TextBlock> {
        match &self.kind {
            AnnotationKind::Bubble(b) => Some(&b.text),
            AnnotationKind::Text(t) => Some(t),
            AnnotationKind::MessageWindow(w) => Some(&w.text),
            AnnotationKind::Stamp(_) => None,
        }
    }

    pub fn text_block_mut(&mut self) -> Option<&mut TextBlock> {
        match &mut self.kind {
            AnnotationKind::Bubble(b) => Some(&mut b.text),
            AnnotationKind::Text(t) => Some(t),
            AnnotationKind::MessageWindow(w) => Some(&mut w.text),
            AnnotationKind::Stamp(_) => None,
        }
    }
}
