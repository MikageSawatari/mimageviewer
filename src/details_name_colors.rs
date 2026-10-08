//! Details-name classification and colors. No filesystem work or retained row state.

use crate::grid_item::{GridItem, SearchContainerKind};
use crate::settings::TextContrast;
use egui::{Color32, Visuals};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DetailsNameCategory {
    Folder,
    Book,
    Image,
    Raw,
    Video,
    Audio,
}

impl DetailsNameCategory {
    pub const ALL: [Self; 6] = [
        Self::Folder,
        Self::Book,
        Self::Image,
        Self::Raw,
        Self::Video,
        Self::Audio,
    ];

    pub fn index(self) -> usize {
        self as usize
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Folder => "フォルダ",
            Self::Book => "本",
            Self::Image => "画像",
            Self::Raw => "RAW",
            Self::Video => "動画",
            Self::Audio => "音声",
        }
    }
}

/// Classification uses the item itself, never its representative thumbnail.
pub fn category(item: &GridItem) -> Option<DetailsNameCategory> {
    use DetailsNameCategory as C;
    Some(match item {
        GridItem::Folder(_)
        | GridItem::ZipDir {
            is_archive: false, ..
        }
        | GridItem::SearchContainer {
            kind: SearchContainerKind::Folder,
            ..
        } => C::Folder,
        GridItem::ZipFile(_)
        | GridItem::PdfFile(_)
        | GridItem::ConvertibleArchive { .. }
        | GridItem::ZipDir {
            is_archive: true, ..
        }
        | GridItem::SearchContainer {
            kind: SearchContainerKind::Zip,
            ..
        } => C::Book,
        GridItem::Image(path) if crate::raw_format::is_raw_path(path) => C::Raw,
        GridItem::ZipImage { entry_name, .. }
            if crate::raw_format::is_raw_path(std::path::Path::new(entry_name)) =>
        {
            C::Raw
        }
        GridItem::Image(_)
        | GridItem::ZipImage { .. }
        | GridItem::PdfPage { .. }
        | GridItem::Stack { .. } => C::Image,
        GridItem::Video(_) => C::Video,
        GridItem::Audio(_) => C::Audio,
        GridItem::CollectionPlaceholder { .. } => return None,
    })
}

#[derive(serde::Serialize, serde::Deserialize, Clone, Copy, Debug, PartialEq, Eq, Default)]
#[serde(tag = "mode", rename_all = "snake_case")]
pub enum DetailsNameColor {
    Custom {
        light: [u8; 3],
        dark: [u8; 3],
    },
    #[default]
    #[serde(other)]
    Default,
}

#[derive(serde::Serialize, serde::Deserialize, Clone, Debug, PartialEq, Eq)]
#[serde(default)]
pub struct DetailsNameColors {
    pub enabled: bool,
    pub colors: [DetailsNameColor; 6],
}

impl Default for DetailsNameColors {
    fn default() -> Self {
        Self {
            enabled: true,
            colors: [DetailsNameColor::Default; 6],
        }
    }
}

impl DetailsNameColors {
    pub fn color(
        &self,
        category: DetailsNameCategory,
        visuals: &Visuals,
        contrast: TextContrast,
    ) -> Color32 {
        if !self.enabled {
            return visuals.text_color();
        }
        let strong = contrast.normalized() == TextContrast::Strong;
        if !strong {
            if let DetailsNameColor::Custom { light, dark } = self.colors[category.index()] {
                let [r, g, b] = if visuals.dark_mode { dark } else { light };
                return Color32::from_rgb(r, g, b);
            }
        }
        if category != DetailsNameCategory::Folder {
            return visuals.text_color();
        }
        let [r, g, b] = match (visuals.dark_mode, strong) {
            (false, false) => [0xA8, 0x7E, 0x00],
            (true, false) => [0xD6, 0xBA, 0x66],
            (false, true) => [0x80, 0x60, 0x00],
            (true, true) => [0xF4, 0xDF, 0xA2],
        };
        Color32::from_rgb(r, g, b)
    }

    pub fn valid_standard_custom(&self) -> bool {
        self.colors.iter().all(|color| match color {
            DetailsNameColor::Default => true,
            DetailsNameColor::Custom { light, dark } => {
                contrast_ratio(
                    Color32::from_rgb(light[0], light[1], light[2]),
                    Color32::from_gray(248),
                ) >= 3.0
                    && contrast_ratio(
                        Color32::from_rgb(dark[0], dark[1], dark[2]),
                        Color32::from_gray(27),
                    ) >= 3.0
            }
        })
    }
}

/// Selected and checked names share the existing state colors in every category.
pub fn name_color(
    settings: &DetailsNameColors,
    category: Option<DetailsNameCategory>,
    visuals: &Visuals,
    contrast: TextContrast,
    selected: bool,
    checked: bool,
    display_only: bool,
) -> Color32 {
    if selected {
        visuals.selection.stroke.color
    } else if checked || display_only {
        visuals.text_color()
    } else {
        category.map_or_else(
            || visuals.text_color(),
            |category| settings.color(category, visuals, contrast),
        )
    }
}

/// Share the real Details backgrounds with Preferences previews and validation.
pub fn row_backgrounds(visuals: &Visuals) -> [Color32; 3] {
    [
        visuals.panel_fill,
        crate::ui_main::details_alternating_row_fill(visuals),
        visuals.widgets.hovered.bg_fill,
    ]
}

pub fn contrast_ratio(foreground: Color32, background: Color32) -> f64 {
    fn luminance(color: Color32) -> f64 {
        [color.r(), color.g(), color.b()]
            .into_iter()
            .zip([0.2126, 0.7152, 0.0722])
            .map(|(channel, weight)| {
                let c = f64::from(channel) / 255.0;
                weight
                    * if c <= 0.04045 {
                        c / 12.92
                    } else {
                        ((c + 0.055) / 1.055).powf(2.4)
                    }
            })
            .sum()
    }
    let a = luminance(foreground);
    let b = luminance(background);
    (a.max(b) + 0.05) / (a.min(b) + 0.05)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::os_theme::{ResolvedTheme, app_visuals};

    #[test]
    fn defaults_and_custom_roundtrip_preserve_disabled_and_strong_values() {
        let mut colors = DetailsNameColors::default();
        assert!(colors.enabled);
        assert!(
            colors
                .colors
                .iter()
                .all(|c| *c == DetailsNameColor::Default)
        );
        colors.colors[DetailsNameCategory::Book.index()] = DetailsNameColor::Custom {
            light: [60, 90, 20],
            dark: [180, 210, 140],
        };
        colors.enabled = false;
        let restored: DetailsNameColors =
            serde_json::from_str(&serde_json::to_string(&colors).unwrap()).unwrap();
        assert_eq!(colors, restored);
        colors.enabled = true;
        let visuals = app_visuals(ResolvedTheme::Dark, TextContrast::Strong);
        assert_eq!(
            colors.color(DetailsNameCategory::Book, &visuals, TextContrast::Strong),
            visuals.text_color()
        );
        let visuals = app_visuals(ResolvedTheme::Dark, TextContrast::Standard);
        assert_eq!(
            colors.color(DetailsNameCategory::Book, &visuals, TextContrast::Standard),
            Color32::from_rgb(180, 210, 140)
        );
        assert_eq!(colors.colors, restored.colors);
    }

    #[test]
    fn classification_covers_physical_virtual_stack_and_unavailable_items() {
        use crate::collection_store::CollectionResolvedKind;
        use crate::grid_item::CollectionPlaceholderReason;
        let p = std::path::PathBuf::from("test");
        let cases = [
            (
                GridItem::Folder(p.clone()),
                Some(DetailsNameCategory::Folder),
            ),
            (
                GridItem::Image(p.with_extension("png")),
                Some(DetailsNameCategory::Image),
            ),
            (
                GridItem::Image(p.with_extension("NEF")),
                Some(DetailsNameCategory::Raw),
            ),
            (GridItem::Video(p.clone()), Some(DetailsNameCategory::Video)),
            (GridItem::Audio(p.clone()), Some(DetailsNameCategory::Audio)),
            (
                GridItem::ZipFile(p.with_extension("cbz")),
                Some(DetailsNameCategory::Book),
            ),
            (
                GridItem::PdfFile(p.with_extension("epub")),
                Some(DetailsNameCategory::Book),
            ),
            (
                GridItem::ConvertibleArchive {
                    path: p.clone(),
                    format: crate::archive_converter::ArchiveFormat::Rar,
                },
                Some(DetailsNameCategory::Book),
            ),
            (
                GridItem::ZipImage {
                    zip_path: p.clone(),
                    entry_name: "nested/P.CR3".into(),
                },
                Some(DetailsNameCategory::Raw),
            ),
            (
                GridItem::ZipImage {
                    zip_path: p.clone(),
                    entry_name: "nested/P.jpg".into(),
                },
                Some(DetailsNameCategory::Image),
            ),
            (
                GridItem::ZipDir {
                    zip_path: p.clone(),
                    dir_prefix: "dir/".into(),
                    is_archive: false,
                    representative: None,
                },
                Some(DetailsNameCategory::Folder),
            ),
            (
                GridItem::ZipDir {
                    zip_path: p.clone(),
                    dir_prefix: "book.zip/".into(),
                    is_archive: true,
                    representative: None,
                },
                Some(DetailsNameCategory::Book),
            ),
            (
                GridItem::PdfPage {
                    pdf_path: p.clone(),
                    page_num: 0,
                    content_type: None,
                },
                Some(DetailsNameCategory::Image),
            ),
            (
                GridItem::Stack {
                    key: "mixed".into(),
                    representative: p.with_extension("nef"),
                    count: 2,
                },
                Some(DetailsNameCategory::Image),
            ),
            (
                GridItem::SearchContainer {
                    path: p.clone(),
                    kind: SearchContainerKind::Folder,
                    hit_count: 1,
                    representative: None,
                },
                Some(DetailsNameCategory::Folder),
            ),
            (
                GridItem::SearchContainer {
                    path: p.clone(),
                    kind: SearchContainerKind::Zip,
                    hit_count: 1,
                    representative: None,
                },
                Some(DetailsNameCategory::Book),
            ),
            (
                GridItem::CollectionPlaceholder {
                    path: p,
                    last_known_kind: CollectionResolvedKind::Image,
                    reason: CollectionPlaceholderReason::Missing,
                },
                None,
            ),
        ];
        for (item, expected) in cases {
            assert_eq!(category(&item), expected, "{item:?}");
        }
    }

    #[test]
    fn final_palette_contrast_and_state_overrides_use_actual_visuals() {
        let colors = DetailsNameColors::default();
        for (theme, contrast, rgb, expected) in [
            (
                ResolvedTheme::Light,
                TextContrast::Standard,
                [0xA8, 0x7E, 0],
                [3.50, 3.23, 2.71, 4.83],
            ),
            (
                ResolvedTheme::Dark,
                TextContrast::Standard,
                [0xD6, 0xBA, 0x66],
                [9.08, 7.78, 4.98, 7.49],
            ),
            (
                ResolvedTheme::Light,
                TextContrast::Strong,
                [0x80, 0x60, 0],
                [5.51, 5.09, 4.26, 7.48],
            ),
            (
                ResolvedTheme::Dark,
                TextContrast::Strong,
                [0xF4, 0xDF, 0xA2],
                [13.06, 11.18, 7.16, 10.63],
            ),
        ] {
            let visuals = app_visuals(theme, contrast);
            let fg = colors.color(DetailsNameCategory::Folder, &visuals, contrast);
            assert_eq!(fg, Color32::from_rgb(rgb[0], rgb[1], rgb[2]));
            for (bg, expected) in row_backgrounds(&visuals).into_iter().zip(expected) {
                assert!((contrast_ratio(fg, bg) - expected).abs() < 0.005);
            }
            assert!(
                (contrast_ratio(visuals.text_color(), visuals.widgets.active.bg_fill)
                    - expected[3])
                    .abs()
                    < 0.005
            );
            assert!(
                contrast_ratio(visuals.selection.stroke.color, visuals.selection.bg_fill) >= 4.5
            );
            for category in DetailsNameCategory::ALL {
                assert_eq!(
                    name_color(
                        &colors,
                        Some(category),
                        &visuals,
                        contrast,
                        true,
                        true,
                        false
                    ),
                    visuals.selection.stroke.color
                );
                assert_eq!(
                    name_color(
                        &colors,
                        Some(category),
                        &visuals,
                        contrast,
                        false,
                        true,
                        false
                    ),
                    visuals.text_color()
                );
                assert_eq!(
                    name_color(
                        &colors,
                        Some(category),
                        &visuals,
                        contrast,
                        false,
                        false,
                        true
                    ),
                    visuals.text_color()
                );
                if category != DetailsNameCategory::Folder {
                    assert_eq!(
                        colors.color(category, &visuals, contrast),
                        visuals.text_color()
                    );
                }
            }
        }
    }

    #[test]
    fn standard_custom_floor_uses_normal_background_only_and_exact_three_boundary() {
        let mut settings = DetailsNameColors::default();
        settings.colors[0] = DetailsNameColor::Custom {
            light: [168, 126, 0],
            dark: [214, 186, 102],
        };
        assert!(settings.valid_standard_custom()); // hover below 3:1 is permitted
        settings.colors[0] = DetailsNameColor::Custom {
            light: [255; 3],
            dark: [214, 186, 102],
        };
        assert!(!settings.valid_standard_custom());
        settings.colors[0] = DetailsNameColor::Custom {
            light: [168, 126, 0],
            dark: [0; 3],
        };
        assert!(!settings.valid_standard_custom());
        // Adjacent grays on either side of the threshold must not be rounded before validation.
        for (dark, bg) in [
            (false, Color32::from_gray(248)),
            (true, Color32::from_gray(27)),
        ] {
            let mut found_boundary = false;
            for value in 0..255u8 {
                let a = contrast_ratio(Color32::from_gray(value), bg) >= 3.0;
                let b = contrast_ratio(Color32::from_gray(value + 1), bg) >= 3.0;
                if a != b {
                    found_boundary = true;
                    for (value, expected) in [(value, a), (value + 1, b)] {
                        settings.colors[0] = DetailsNameColor::Custom {
                            light: if dark { [0; 3] } else { [value; 3] },
                            dark: if dark { [value; 3] } else { [255; 3] },
                        };
                        assert_eq!(settings.valid_standard_custom(), expected);
                    }
                }
            }
            assert!(found_boundary);
        }
    }
}
