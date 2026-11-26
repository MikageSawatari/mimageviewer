//! Serializable recipe values. Output names belong to the future dialog, not presets.
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum CombineLayout {
    Row,
    Column,
    Grid,
    #[default]
    JustifiedRows,
    JustifiedColumns,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum CombineDirection {
    #[default]
    LeftToRight,
    RightToLeft,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum LineSize {
    #[default]
    Largest,
    Smallest,
    None,
}

/// Alignment on the cross axis of an unmatched row or column.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum LineAlign {
    Start,
    #[default]
    Center,
    End,
}

impl LineAlign {
    pub fn fraction(self) -> f64 {
        match self {
            Self::Start => 0.0,
            Self::Center => 0.5,
            Self::End => 1.0,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum CellRatio {
    #[default]
    FirstImage,
    Ratio(u32, u32),
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum CellFit {
    #[default]
    Fit,
    Fill,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum Anchor9 {
    TopLeft,
    TopCenter,
    TopRight,
    CenterLeft,
    #[default]
    Center,
    CenterRight,
    BottomLeft,
    BottomCenter,
    BottomRight,
}

impl Anchor9 {
    pub const ALL: [Self; 9] = [
        Self::TopLeft,
        Self::TopCenter,
        Self::TopRight,
        Self::CenterLeft,
        Self::Center,
        Self::CenterRight,
        Self::BottomLeft,
        Self::BottomCenter,
        Self::BottomRight,
    ];

    pub fn fractions(self) -> (f64, f64) {
        let index = Self::ALL.iter().position(|a| *a == self).unwrap();
        ((index % 3) as f64 / 2.0, (index / 3) as f64 / 2.0)
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum Split {
    #[default]
    Auto,
    Counts(Vec<u32>),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Background {
    Color([u8; 3]),
    Transparent,
}

impl Default for Background {
    fn default() -> Self {
        Self::Color([255; 3])
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Border {
    pub width_px: u32,
    pub color: [u8; 3],
}

impl Default for Border {
    fn default() -> Self {
        Self {
            width_px: 0,
            color: [255; 3],
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum LabelPlacement {
    #[default]
    None,
    Overlay,
    Band,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum NumberFormat {
    #[default]
    Arabic,
    ZeroPadded,
    UpperAlpha,
    LowerAlpha,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum BadgeStyle {
    #[default]
    Plain,
    Box,
    Pill,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum BandColor {
    #[default]
    Background,
    Color([u8; 3]),
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum HAlign {
    Left,
    #[default]
    Center,
    Right,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct LabelOptions {
    pub placement: LabelPlacement,
    pub number_format: NumberFormat,
    pub number_start: u32,
    pub style: BadgeStyle,
    pub text_size_px: u32,
    pub text_color: [u8; 3],
    pub outline: bool,
    pub fill_color: [u8; 3],
    pub fill_opacity: u8,
    pub overlay_anchor: Anchor9,
    pub overlay_margin_px: u32,
    pub band_color: BandColor,
    pub band_align: HAlign,
}

impl Default for LabelOptions {
    fn default() -> Self {
        Self {
            placement: LabelPlacement::None,
            number_format: NumberFormat::Arabic,
            number_start: 1,
            style: BadgeStyle::Plain,
            text_size_px: 48,
            text_color: [255; 3],
            outline: true,
            fill_color: [0; 3],
            fill_opacity: 70,
            overlay_anchor: Anchor9::TopLeft,
            overlay_margin_px: 16,
            band_color: BandColor::Background,
            band_align: HAlign::Center,
        }
    }
}

impl LabelOptions {
    pub fn band_height(&self) -> Option<u32> {
        (self.placement == LabelPlacement::Band).then(|| {
            (self.text_size_px as f64
                * if self.style == BadgeStyle::Plain {
                    1.6
                } else {
                    2.0
                })
            .ceil() as u32
        })
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum OutputSize {
    #[default]
    Original,
    Width(u32),
    Height(u32),
    LongEdge(u32),
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum CombineFormat {
    #[default]
    Png,
    Jpeg {
        #[serde(default = "jpeg_quality")]
        quality: u8,
    },
    Webp {
        #[serde(default = "webp_quality")]
        quality: u8,
        #[serde(default)]
        lossless: bool,
    },
}

fn jpeg_quality() -> u8 {
    95
}
fn webp_quality() -> u8 {
    90
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum Destination {
    #[default]
    FirstImageFolder,
    Folder(PathBuf),
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct CombineOptions {
    pub layout: CombineLayout,
    pub direction: CombineDirection,
    pub line_size: LineSize,
    pub line_align: LineAlign,
    /// None selects the automatic column count.
    pub grid_columns: Option<u32>,
    pub cell_ratio: CellRatio,
    pub cell_fit: CellFit,
    pub anchor: Anchor9,
    pub split: Split,
    pub canvas_ratio: Option<(u32, u32)>,
    pub gap_x: u32,
    pub gap_y: u32,
    pub gaps_linked: bool,
    pub margin: u32,
    pub background: Background,
    pub corner_radius: u32,
    pub border: Border,
    pub labels: LabelOptions,
    pub output_size: OutputSize,
    pub format: CombineFormat,
    pub destination: Destination,
}

impl Default for CombineOptions {
    fn default() -> Self {
        Self {
            layout: CombineLayout::default(),
            direction: CombineDirection::default(),
            line_size: LineSize::default(),
            line_align: LineAlign::default(),
            grid_columns: None,
            cell_ratio: CellRatio::default(),
            cell_fit: CellFit::default(),
            anchor: Anchor9::default(),
            split: Split::default(),
            canvas_ratio: None,
            gap_x: 0,
            gap_y: 0,
            gaps_linked: true,
            margin: 0,
            background: Background::default(),
            corner_radius: 0,
            border: Border::default(),
            labels: LabelOptions::default(),
            output_size: OutputSize::Original,
            format: CombineFormat::Png,
            destination: Destination::default(),
        }
    }
}

impl CombineOptions {
    /// Central range normalization for deserialized settings and future UI callers.
    /// Split counts are validated by layout, never silently repaired.
    pub fn normalized(&self) -> Self {
        let mut o = self.clone();
        o.grid_columns = o.grid_columns.map(|n| n.clamp(1, 20));
        if let CellRatio::Ratio(w, h) = o.cell_ratio {
            o.cell_ratio = CellRatio::Ratio(w.clamp(1, 100), h.clamp(1, 100));
        }
        o.canvas_ratio = o
            .canvas_ratio
            .map(|(w, h)| (w.clamp(1, 100), h.clamp(1, 100)));
        o.gap_x = o.gap_x.min(500);
        o.gap_y = o.gap_y.min(500);
        // Linking is an editing preference; it must not destroy persisted x/y values.
        o.margin = o.margin.min(500);
        o.corner_radius = o.corner_radius.min(500);
        o.border.width_px = o.border.width_px.min(100);
        o.labels.number_start = o.labels.number_start.min(999);
        o.labels.text_size_px = o.labels.text_size_px.clamp(8, 500);
        o.labels.fill_opacity = o.labels.fill_opacity.min(100);
        o.labels.overlay_margin_px = o.labels.overlay_margin_px.min(500);
        o.output_size = match o.output_size {
            OutputSize::Original => OutputSize::Original,
            OutputSize::Width(n) => OutputSize::Width(n.clamp(16, 65500)),
            OutputSize::Height(n) => OutputSize::Height(n.clamp(16, 65500)),
            OutputSize::LongEdge(n) => OutputSize::LongEdge(n.clamp(16, 65500)),
        };
        o.format = match o.format {
            CombineFormat::Png => CombineFormat::Png,
            CombineFormat::Jpeg { quality } => CombineFormat::Jpeg {
                quality: quality.clamp(1, 100),
            },
            CombineFormat::Webp { quality, lossless } => CombineFormat::Webp {
                quality: quality.clamp(1, 100),
                lossless,
            },
        };
        if matches!(o.format, CombineFormat::Jpeg { .. }) && o.background == Background::Transparent
        {
            o.background = Background::Color([255; 3]);
        }
        o
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum CombineSourceRef {
    File {
        path: PathBuf,
    },
    ZipEntry {
        zip_path: PathBuf,
        entry_name: String,
    },
    PdfPage {
        pdf_path: PathBuf,
        page_num: u32,
    },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CombineItemSpec {
    pub source: CombineSourceRef,
    #[serde(default)]
    pub label_override: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CombinePreset {
    pub name: String,
    pub options: CombineOptions,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CombineRecipe {
    pub v: u32,
    #[serde(default)]
    pub options: CombineOptions,
    #[serde(default)]
    pub format: CombineFormat,
    pub items: Vec<CombineItemSpec>,
}
