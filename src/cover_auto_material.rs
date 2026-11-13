//! Immutable, rotation-independent material carried by representative thumbnails.
use crate::folder_thumb_pins::CoverCrop;
use crate::rotation_db::Rotation;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AutoCropResults([Option<CoverCrop>; 4]);

impl AutoCropResults {
    pub(crate) fn new(crops: [Option<CoverCrop>; 4]) -> Self {
        Self(crops)
    }

    pub(crate) fn crop_for(self, angle: Rotation) -> Option<CoverCrop> {
        self.0[match angle {
            Rotation::None => 0,
            Rotation::Cw90 => 1,
            Rotation::Cw180 => 2,
            Rotation::Cw270 => 3,
        }]
    }

    pub(crate) fn crops(self) -> [Option<CoverCrop>; 4] {
        self.0
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum AutoMaterial {
    #[default]
    NotAutomatic,
    Automatic(AutoCropResults),
    /// Pixels already cropped by v5.2.0, retained only by the K2 compatibility read.
    LegacyV1,
}

impl AutoMaterial {
    /// One canonical codec for the catalog's nullable `auto_crops` column.
    pub(crate) fn encode(self) -> String {
        match self {
            Self::NotAutomatic => "not-automatic".to_owned(),
            Self::LegacyV1 => "legacy-v1".to_owned(),
            Self::Automatic(results) => {
                let values = results.crops().map(|crop| match crop {
                    None => "-".to_owned(),
                    Some(crop) => crop.values().map(|n| n.to_string()).join(","),
                });
                format!("automatic-v1:{}", values.join(";"))
            }
        }
    }

    pub(crate) fn decode(value: &str) -> Option<Self> {
        match value {
            "not-automatic" => Some(Self::NotAutomatic),
            "legacy-v1" => Some(Self::LegacyV1),
            _ => {
                let values: Vec<_> = value.strip_prefix("automatic-v1:")?.split(';').collect();
                if values.len() != 4 {
                    return None;
                }
                let mut crops = [None; 4];
                for (out, value) in crops.iter_mut().zip(values) {
                    if value != "-" {
                        let coords: Vec<_> = value.split(',').map(str::parse::<u32>).collect();
                        let [Ok(x0), Ok(y0), Ok(x1), Ok(y1)] = coords.as_slice() else {
                            return None;
                        };
                        *out = Some(CoverCrop::new([*x0, *y0, *x1, *y1]).ok()??);
                    }
                }
                let material = Self::Automatic(AutoCropResults::new(crops));
                // Reject alternate spellings as well as invalid ranges.
                (material.encode() == value).then_some(material)
            }
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct AutoCropView {
    /// Texture coordinates stay in the unrotated source coordinate system.
    pub uv: egui::Rect,
    /// Layout dimensions after selecting the crop and applying saved rotation.
    pub displayed_size: egui::Vec2,
    /// Effective unrotated dimensions for pixel-quality comparisons.
    pub source_dims: (u32, u32),
    pub layout_dims: Option<(u32, u32)>,
}

pub(crate) fn auto_crop_view(
    source_dims: (u32, u32),
    layout_dims: Option<(u32, u32)>,
    material: AutoMaterial,
    angle: Rotation,
) -> AutoCropView {
    let crop = match material {
        AutoMaterial::Automatic(results) => results.crop_for(angle),
        AutoMaterial::NotAutomatic | AutoMaterial::LegacyV1 => None,
    };
    let uv = crop.map_or_else(
        || egui::Rect::from_min_max(egui::Pos2::ZERO, egui::pos2(1.0, 1.0)),
        |crop| {
            let [x0, y0, x1, y1] = crop.values().map(|v| v as f32 / 1_000_000.0);
            egui::Rect::from_min_max(egui::pos2(x0, y0), egui::pos2(x1, y1))
        },
    );
    let effective_dims = |dims: (u32, u32)| {
        crop.map_or(dims, |crop| {
            let (_, _, width, height) = crop.pixel_bounds(dims.0, dims.1);
            (width, height)
        })
    };
    let source_dims = effective_dims(source_dims);
    let layout_dims = layout_dims.map(effective_dims);
    let (width, height) = layout_dims.unwrap_or(source_dims);
    let displayed_size = match angle {
        Rotation::None | Rotation::Cw180 => egui::vec2(width as f32, height as f32),
        Rotation::Cw90 | Rotation::Cw270 => egui::vec2(height as f32, width as f32),
    };
    AutoCropView {
        uv,
        displayed_size,
        source_dims,
        layout_dims,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn auto_material_codec_roundtrip_and_validation() {
        let crop = CoverCrop::new([100_000, 200_000, 600_000, 900_000]).unwrap();
        for material in [
            AutoMaterial::NotAutomatic,
            AutoMaterial::LegacyV1,
            AutoMaterial::Automatic(AutoCropResults::new([crop, None, crop, None])),
            AutoMaterial::Automatic(AutoCropResults::new([None; 4])),
        ] {
            assert_eq!(AutoMaterial::decode(&material.encode()), Some(material));
        }
        for value in [
            "",
            "automatic-v1:-;-;-",
            "automatic-v2:-;-;-;-",
            "automatic-v1:0,0,1000000,1000000;-;-;-",
            "automatic-v1:0,0,1000001,500000;-;-;-",
            "automatic-v1:0,0,10,500000;-;-;-",
            "automatic-v1:00,0,500000,500000;-;-;-",
        ] {
            assert_eq!(AutoMaterial::decode(value), None, "{value}");
        }
    }

    #[test]
    fn auto_crop_view_selects_rotation_without_mutating_material() {
        let crop = CoverCrop::new([500_000, 0, 1_000_000, 1_000_000]).unwrap();
        let material = AutoMaterial::Automatic(AutoCropResults::new([None, crop, None, crop]));
        for angle in [Rotation::Cw90, Rotation::Cw270] {
            let view = auto_crop_view((2000, 1200), Some((600_000, 400_000)), material, angle);
            assert_eq!(view.source_dims, (1000, 1200));
            assert_eq!(view.layout_dims, Some((300_000, 400_000)));
            assert_eq!(view.displayed_size, egui::vec2(400_000.0, 300_000.0));
            assert_eq!(view.uv.min, egui::pos2(0.5, 0.0));
        }
        for material in [material, AutoMaterial::NotAutomatic, AutoMaterial::LegacyV1] {
            let view = auto_crop_view((2000, 1200), None, material, Rotation::None);
            assert_eq!(view.source_dims, (2000, 1200));
            assert_eq!(view.displayed_size, egui::vec2(2000.0, 1200.0));
        }
    }

    #[test]
    fn auto_crop_view_maps_all_four_angles_to_unrotated_uv() {
        let coords = [
            [500_000, 0, 1_000_000, 1_000_000],
            [0, 0, 1_000_000, 500_000],
            [0, 0, 500_000, 1_000_000],
            [0, 500_000, 1_000_000, 1_000_000],
        ];
        let material = AutoMaterial::Automatic(AutoCropResults::new(
            coords.map(|value| CoverCrop::new(value).unwrap()),
        ));
        for (index, angle) in [
            Rotation::None,
            Rotation::Cw90,
            Rotation::Cw180,
            Rotation::Cw270,
        ]
        .into_iter()
        .enumerate()
        {
            let view = auto_crop_view((2000, 1200), None, material, angle);
            let [x0, y0, x1, y1] = coords[index].map(|n| n as f32 / 1_000_000.0);
            assert_eq!(
                view.uv,
                egui::Rect::from_min_max(egui::pos2(x0, y0), egui::pos2(x1, y1))
            );
            if matches!(angle, Rotation::None | Rotation::Cw180) {
                assert_eq!(view.source_dims, (1000, 1200));
                assert_eq!(view.displayed_size, egui::vec2(1000.0, 1200.0));
            } else {
                assert_eq!(view.source_dims, (2000, 600));
                assert_eq!(view.displayed_size, egui::vec2(600.0, 2000.0));
            }
        }
    }
}
