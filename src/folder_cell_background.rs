//! Thumbnail folder backgrounds, shared by painting, previews and validation.

use crate::details_name_colors::{DetailsNameCategory, category, contrast_ratio};
use crate::grid_item::GridItem;
use egui::Color32;

#[derive(serde::Serialize, serde::Deserialize, Clone, Debug, PartialEq, Eq)]
#[serde(default)]
pub struct FolderCellBackground {
    pub enabled: bool,
    pub light: [u8; 3],
    pub dark: [u8; 3],
}

impl Default for FolderCellBackground {
    fn default() -> Self {
        Self {
            enabled: false,
            light: [0xFF, 0xF4, 0xCC],
            dark: [0x3A, 0x34, 0x20],
        }
    }
}

/// Search folder paths are drawn directly on the cell when no image is loaded.
pub fn search_folder_path_color(dark: bool) -> Color32 {
    Color32::from_gray(if dark { 220 } else { 60 })
}

pub fn search_count_color(dark: bool) -> Color32 {
    if dark {
        Color32::from_rgb(240, 200, 100)
    } else {
        Color32::from_rgb(180, 80, 0)
    }
}

impl FolderCellBackground {
    pub fn color(&self, dark: bool) -> Color32 {
        let [r, g, b] = if dark { self.dark } else { self.light };
        Color32::from_rgb(r, g, b)
    }

    /// Validate stored colors even while disabled, as with details-name colors.
    pub fn valid(&self) -> bool {
        [false, true].into_iter().all(|dark| {
            // Add every new text color painted directly on a folder background here.
            // Icons and captions on their own plates are not direct text.
            [search_folder_path_color(dark), search_count_color(dark)]
                .into_iter()
                .all(|text| contrast_ratio(text, self.color(dark)) >= 4.5)
        })
    }
}

/// Keep the existing selection/check priority and normal colors in one owner.
pub fn cell_background(
    settings: &FolderCellBackground,
    item: &GridItem,
    is_drive_list: bool,
    dark: bool,
    selected: bool,
    checked: bool,
) -> Color32 {
    if selected || checked {
        if dark {
            Color32::from_rgb(40, 70, 110)
        } else {
            Color32::from_rgb(180, 210, 255)
        }
    } else if settings.enabled
        && !is_drive_list
        && category(item) == Some(DetailsNameCategory::Folder)
    {
        settings.color(dark)
    } else if dark {
        Color32::from_gray(28)
    } else {
        Color32::WHITE
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::grid_item::SearchContainerKind;

    #[test]
    fn folder_background_painter_and_validator_share_text_colors() {
        for dark in [false, true] {
            assert_eq!(
                search_folder_path_color(dark),
                Color32::from_gray(if dark { 220 } else { 60 })
            );
            assert_eq!(
                search_count_color(dark),
                if dark {
                    Color32::from_rgb(240, 200, 100)
                } else {
                    Color32::from_rgb(180, 80, 0)
                }
            );
            for contrast in [
                crate::settings::TextContrast::Standard,
                crate::settings::TextContrast::Strong,
            ] {
                let ctx = egui::Context::default();
                crate::ui_fonts::configure_fonts(&ctx);
                crate::os_theme::apply_resolved_with_contrast(
                    &ctx,
                    if dark {
                        crate::os_theme::ResolvedTheme::Dark
                    } else {
                        crate::os_theme::ResolvedTheme::Light
                    },
                    contrast,
                );
                let colors = FolderCellBackground {
                    enabled: true,
                    ..Default::default()
                };
                let cell =
                    egui::Rect::from_min_size(egui::pos2(20.0, 20.0), egui::vec2(240.0, 220.0));
                let mut output = None;
                for _ in 0..2 {
                    output = Some(crate::test_ui::headless_paint_output(ctx.run_pass(
                        egui::RawInput::default(),
                        |ctx| {
                            ctx.root_ui(|root| {
                                egui::CentralPanel::default().show(root, |ui| {
                                    crate::app::draw_cell(
                                        ui,
                                        cell,
                                        false,
                                        false,
                                        false,
                                        &Default::default(),
                                        &GridItem::SearchContainer {
                                            path: "G:/写真/春".into(),
                                            kind: SearchContainerKind::Folder,
                                            hit_count: 24,
                                            representative: None,
                                        },
                                        &crate::grid_item::ThumbnailState::Failed,
                                        crate::rotation_db::Rotation::None,
                                        None,
                                        false,
                                        Default::default(),
                                        Default::default(),
                                        false,
                                        None,
                                        &colors,
                                    );
                                });
                            });
                        },
                    )));
                }
                let mut path_seen = false;
                let mut count_seen = false;
                let mut background_seen = false;
                for shape in output.unwrap().shapes {
                    match shape.shape {
                        egui::Shape::Rect(rect) if rect.rect == cell => {
                            background_seen |= rect.fill == colors.color(dark);
                        }
                        egui::Shape::Text(text) => {
                            let color = text.override_text_color.unwrap_or(text.fallback_color);
                            if text.galley.text() == "24 枚" {
                                count_seen = true;
                                assert_eq!(color, search_count_color(dark));
                            } else if text.galley.text().contains("春") {
                                path_seen = true;
                                assert_eq!(color, search_folder_path_color(dark));
                            }
                        }
                        _ => {}
                    }
                }
                assert!(background_seen && path_seen && count_seen);
                assert!(colors.valid());
            }
        }
    }

    #[test]
    fn folder_background_defaults_missing_and_serde_roundtrip() {
        let defaults = FolderCellBackground::default();
        assert!(!defaults.enabled);
        assert_eq!(defaults.light, [255, 244, 204]);
        assert_eq!(defaults.dark, [58, 52, 32]);
        assert!(defaults.valid());
        assert_eq!(
            serde_json::from_str::<FolderCellBackground>("{}").unwrap(),
            defaults
        );
        let colors = FolderCellBackground {
            light: [255, 255, 230],
            ..defaults
        };
        assert_eq!(
            serde_json::from_str::<FolderCellBackground>(&serde_json::to_string(&colors).unwrap())
                .unwrap(),
            colors
        );
    }

    #[test]
    fn folder_background_classification_priority_and_disabled_colors() {
        let mut colors = FolderCellBackground {
            enabled: true,
            ..Default::default()
        };
        let path = std::path::PathBuf::from("folder");
        let cases = [
            (GridItem::Folder(path.clone()), true),
            (
                GridItem::ZipDir {
                    zip_path: path.clone(),
                    dir_prefix: "dir/".into(),
                    is_archive: false,
                    representative: None,
                },
                true,
            ),
            (
                GridItem::SearchContainer {
                    path: path.clone(),
                    kind: SearchContainerKind::Folder,
                    hit_count: 8,
                    representative: None,
                },
                true,
            ),
            (
                GridItem::SearchContainer {
                    path: path.clone(),
                    kind: SearchContainerKind::Zip,
                    hit_count: 8,
                    representative: None,
                },
                false,
            ),
            (
                GridItem::ZipDir {
                    zip_path: path.clone(),
                    dir_prefix: "book.zip/".into(),
                    is_archive: true,
                    representative: None,
                },
                false,
            ),
            (GridItem::ZipFile(path.clone()), false),
            (GridItem::PdfFile(path.clone()), false),
            (
                GridItem::ConvertibleArchive {
                    path: path.clone(),
                    format: crate::archive_converter::ArchiveFormat::Rar,
                },
                false,
            ),
            (GridItem::Image(path.with_extension("jpg")), false),
        ];
        for dark in [false, true] {
            let normal = if dark {
                Color32::from_gray(28)
            } else {
                Color32::WHITE
            };
            let blue = if dark {
                Color32::from_rgb(40, 70, 110)
            } else {
                Color32::from_rgb(180, 210, 255)
            };
            for (item, folder) in &cases {
                assert_eq!(
                    cell_background(&colors, item, false, dark, false, false),
                    if *folder { colors.color(dark) } else { normal }
                );
                assert_eq!(
                    cell_background(&colors, item, true, dark, false, false),
                    normal
                );
                for (selected, checked) in [(true, false), (false, true), (true, true)] {
                    assert_eq!(
                        cell_background(&colors, item, false, dark, selected, checked),
                        blue
                    );
                }
            }
            colors.enabled = false;
            for (item, _) in &cases {
                assert_eq!(
                    cell_background(&colors, item, false, dark, false, false),
                    normal
                );
            }
            colors.enabled = true;
        }
    }

    #[test]
    fn folder_background_validation_checks_each_text_and_theme_at_four_point_five() {
        // Adjacent 8-bit grays straddle each direct text color's 4.5:1 floor.
        // The count color is stricter in both themes; validate the entire pair too.
        for (dark, text, a, b) in [
            (false, search_folder_path_color(false), 165, 166),
            (false, search_count_color(false), 239, 240),
            (true, search_folder_path_color(true), 97, 98),
            (true, search_count_color(true), 87, 88),
        ] {
            assert_eq!(contrast_ratio(text, Color32::from_gray(a)) >= 4.5, dark);
            assert_eq!(contrast_ratio(text, Color32::from_gray(b)) >= 4.5, !dark);
            for value in [a, b] {
                let mut colors = FolderCellBackground::default();
                if dark {
                    colors.dark = [value; 3];
                } else {
                    colors.light = [value; 3];
                }
                let expected = [search_folder_path_color(dark), search_count_color(dark)]
                    .into_iter()
                    .all(|fg| contrast_ratio(fg, colors.color(dark)) >= 4.5);
                assert_eq!(colors.valid(), expected);
            }
        }
        let mut colors = FolderCellBackground::default();
        colors.light = [255, 240, 179];
        assert!(!colors.valid());
        colors.light = [255, 244, 204];
        colors.dark = [220; 3];
        assert!(!colors.valid());
    }
}
