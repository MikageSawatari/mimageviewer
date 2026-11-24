use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::{capture::CaptureFormat, export_dialog::ExportScale, save_with_metadata::SrcFormat};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum WatermarkKind {
    #[default]
    Text,
    Image,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ImageWatermarkSize {
    PercentOfWidth(u32),
    Original,
}

impl Default for ImageWatermarkSize {
    fn default() -> Self {
        Self::PercentOfWidth(20)
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum WatermarkAnchor {
    TopLeft,
    TopCenter,
    TopRight,
    CenterLeft,
    Center,
    CenterRight,
    BottomLeft,
    BottomCenter,
    #[default]
    BottomRight,
}

impl WatermarkAnchor {
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

    pub fn label(self) -> &'static str {
        match self {
            Self::TopLeft => "左上",
            Self::TopCenter => "上中央",
            Self::TopRight => "右上",
            Self::CenterLeft => "左中央",
            Self::Center => "中央",
            Self::CenterRight => "右中央",
            Self::BottomLeft => "左下",
            Self::BottomCenter => "下中央",
            Self::BottomRight => "右下",
        }
    }

    pub(super) fn axes(self) -> (u32, u32) {
        let index = Self::ALL.iter().position(|anchor| *anchor == self).unwrap() as u32;
        (index % 3, index / 3)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct BatchWatermark {
    pub enabled: bool,
    pub kind: WatermarkKind,
    pub text: String,
    pub text_color: [u8; 3],
    pub text_outline: bool,
    pub text_size_percent: u32,
    pub image_path: Option<PathBuf>,
    pub image_size: ImageWatermarkSize,
    pub anchor: WatermarkAnchor,
    pub margin_percent: u32,
    pub opacity_percent: u32,
}

impl Default for BatchWatermark {
    fn default() -> Self {
        Self {
            enabled: false,
            kind: WatermarkKind::Text,
            text: String::new(),
            text_color: [255; 3],
            text_outline: true,
            text_size_percent: 4,
            image_path: None,
            image_size: ImageWatermarkSize::default(),
            anchor: WatermarkAnchor::BottomRight,
            margin_percent: 2,
            opacity_percent: 60,
        }
    }
}

pub(crate) fn supported_logo_path(path: &std::path::Path) -> bool {
    path.extension()
        .and_then(|ext| ext.to_str())
        .is_some_and(|ext| {
            ["png", "webp", "jpg", "jpeg", "bmp", "gif"]
                .iter()
                .any(|allowed| ext.eq_ignore_ascii_case(allowed))
        })
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum BatchFormatKind {
    #[default]
    Png,
    Jpeg,
    Webp,
    SameAsSource,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum SameFallback {
    #[default]
    Jpeg,
    Png,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct BatchFormat {
    pub kind: BatchFormatKind,
    pub jpeg_quality: u8,
    pub webp_quality: u8,
    pub webp_lossless: bool,
    pub same_fallback: SameFallback,
}

impl Default for BatchFormat {
    fn default() -> Self {
        Self {
            kind: BatchFormatKind::Png,
            jpeg_quality: 95,
            webp_quality: 90,
            webp_lossless: false,
            same_fallback: SameFallback::Jpeg,
        }
    }
}

impl BatchFormat {
    pub fn resolve(&self, source_ext: &str) -> SrcFormat {
        match self.kind {
            BatchFormatKind::Png => SrcFormat::Png,
            BatchFormatKind::Jpeg => SrcFormat::Jpeg,
            BatchFormatKind::Webp => SrcFormat::Webp,
            BatchFormatKind::SameAsSource => match source_ext.to_ascii_lowercase().as_str() {
                "jpg" | "jpeg" | "jfif" => SrcFormat::Jpeg,
                "png" => SrcFormat::Png,
                "webp" => SrcFormat::Webp,
                _ => match self.same_fallback {
                    SameFallback::Jpeg => SrcFormat::Jpeg,
                    SameFallback::Png => SrcFormat::Png,
                },
            },
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum BatchResizeMode {
    #[default]
    None,
    Percent(u32),
    LongEdge(u32),
    ShortEdge(u32),
    Width(u32),
    Height(u32),
    Fit {
        width: u32,
        height: u32,
    },
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum ResizeFilter {
    #[default]
    Lanczos3,
    CatmullRom,
    Bilinear,
    Nearest,
}

impl From<ResizeFilter> for crate::fast_resize::Quality {
    fn from(value: ResizeFilter) -> Self {
        match value {
            ResizeFilter::Lanczos3 => Self::Lanczos3,
            ResizeFilter::CatmullRom => Self::CatmullRom,
            ResizeFilter::Bilinear => Self::Bilinear,
            ResizeFilter::Nearest => Self::Nearest,
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct BatchResize {
    pub mode: BatchResizeMode,
    pub allow_upscale: bool,
    pub filter: ResizeFilter,
}

impl BatchResize {
    pub fn scaled_size(&self, size: [usize; 2]) -> [usize; 2] {
        let [w, h] = size.map(|n| n.max(1));
        // Use the legacy f32 calculation exactly, including round-to-nearest.
        let factor = match self.mode {
            BatchResizeMode::None => return [w, h],
            BatchResizeMode::Percent(p) => p.clamp(1, 400) as f32 / 100.0,
            BatchResizeMode::LongEdge(n) => n.max(1) as f32 / w.max(h) as f32,
            BatchResizeMode::ShortEdge(n) => n.max(1) as f32 / w.min(h) as f32,
            BatchResizeMode::Width(n) => n.max(1) as f32 / w as f32,
            BatchResizeMode::Height(n) => n.max(1) as f32 / h as f32,
            BatchResizeMode::Fit { width, height } => {
                (width.max(1) as f32 / w as f32).min(height.max(1) as f32 / h as f32)
            }
        };
        let target = [
            ((w as f32 * factor).round() as usize).max(1),
            ((h as f32 * factor).round() as usize).max(1),
        ];
        if !self.allow_upscale && (target[0] > w || target[1] > h) {
            [w, h]
        } else {
            target
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum BatchDestination {
    Folder { path: PathBuf, keep_structure: bool },
    SameAsSource { subfolder: String },
}

impl Default for BatchDestination {
    fn default() -> Self {
        Self::Folder {
            path: PathBuf::new(),
            keep_structure: false,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum OverwritePolicy {
    #[default]
    Rename,
    Skip,
    Replace,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct BatchExportOptions {
    pub destination: BatchDestination,
    pub template: String,
    pub format: BatchFormat,
    pub resize: BatchResize,
    pub overwrite: OverwritePolicy,
    pub keep_metadata: bool,
    pub keep_file_time: bool,
    #[serde(default)]
    pub watermark: BatchWatermark,
}

impl Default for BatchExportOptions {
    fn default() -> Self {
        Self {
            destination: BatchDestination::default(),
            template: crate::settings::default_export_batch_template(),
            format: BatchFormat::default(),
            resize: BatchResize::default(),
            overwrite: OverwritePolicy::Rename,
            keep_metadata: false,
            keep_file_time: false,
            watermark: BatchWatermark::default(),
        }
    }
}

impl BatchExportOptions {
    /// Keep the stored choice while disabling its effect for replacement exports.
    pub fn can_keep_file_time(&self) -> bool {
        self.overwrite != OverwritePolicy::Replace
    }

    pub fn from_legacy(
        directory: PathBuf,
        template: String,
        format: CaptureFormat,
        scale: ExportScale,
    ) -> Self {
        Self {
            destination: BatchDestination::Folder {
                path: directory,
                keep_structure: false,
            },
            template,
            format: BatchFormat {
                kind: if format.jpeg_quality().is_some() {
                    BatchFormatKind::Jpeg
                } else {
                    BatchFormatKind::Png
                },
                jpeg_quality: format.jpeg_quality().unwrap_or(95),
                ..Default::default()
            },
            resize: BatchResize {
                mode: match scale {
                    ExportScale::Full => BatchResizeMode::None,
                    ExportScale::Half => BatchResizeMode::Percent(50),
                    ExportScale::Quarter => BatchResizeMode::Percent(25),
                    ExportScale::LongEdge(n) => BatchResizeMode::LongEdge(n),
                },
                ..Default::default()
            },
            ..Default::default()
        }
    }
}
