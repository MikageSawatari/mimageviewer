//! Creative 3D LUT registration, asynchronous loading, and CPU application.
//!
//! Registered entries keep the user-selected `.cube` path. Parsing is always
//! performed on a worker thread; the UI and video-presenter threads only use
//! already parsed immutable tables.

use eframe::egui;
use local_adjust_core::CubeLutParams;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, mpsc};
use uuid::Uuid;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CreativeLutEntry {
    pub id: Uuid,
    pub name: String,
    pub path: PathBuf,
}

impl CreativeLutEntry {
    pub fn from_loaded_path(path: PathBuf, lut: &CubeLutParams) -> Self {
        let fallback = path
            .file_stem()
            .and_then(|value| value.to_str())
            .unwrap_or("LUT");
        let name = if lut.name.trim().is_empty() {
            fallback.to_owned()
        } else {
            lut.name.trim().to_owned()
        };
        Self {
            id: Uuid::new_v4(),
            name,
            path,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct CreativeLutSelection {
    pub id: Option<Uuid>,
    /// Original pixels and LUT output are linearly mixed by this value.
    pub strength: f32,
}

impl Default for CreativeLutSelection {
    fn default() -> Self {
        Self {
            id: None,
            strength: 1.0,
        }
    }
}

impl CreativeLutSelection {
    pub fn is_identity(&self) -> bool {
        self.id.is_none() || self.strength <= f32::EPSILON
    }

    pub fn sanitize(&mut self) {
        self.strength = self.strength.clamp(0.0, 1.0);
    }
}

/// Video preview adjustments. These are viewer-wide and intentionally carry
/// only the image panel's tone controls plus creative LUT selection.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct VideoAdjustments {
    pub brightness: f32,
    pub contrast: f32,
    pub gamma: f32,
    pub saturation: f32,
    pub temperature: f32,
    pub black_point: u8,
    pub white_point: u8,
    pub midtone: f32,
    pub creative_lut: CreativeLutSelection,
}

impl Default for VideoAdjustments {
    fn default() -> Self {
        Self {
            brightness: 0.0,
            contrast: 0.0,
            gamma: 1.0,
            saturation: 0.0,
            temperature: 0.0,
            black_point: 0,
            white_point: 255,
            midtone: 1.0,
            creative_lut: CreativeLutSelection::default(),
        }
    }
}

impl VideoAdjustments {
    pub fn is_color_identity(&self) -> bool {
        self.brightness == 0.0
            && self.contrast == 0.0
            && self.gamma == 1.0
            && self.saturation == 0.0
            && self.temperature == 0.0
            && self.black_point == 0
            && self.white_point == 255
            && self.midtone == 1.0
    }

    pub fn is_identity(&self) -> bool {
        self.is_color_identity() && self.creative_lut.is_identity()
    }

    pub fn sanitize(&mut self) {
        self.brightness = self.brightness.clamp(-100.0, 100.0);
        self.contrast = self.contrast.clamp(-100.0, 100.0);
        self.gamma = self.gamma.clamp(0.2, 5.0);
        self.saturation = self.saturation.clamp(-100.0, 100.0);
        self.temperature = self.temperature.clamp(-100.0, 100.0);
        if self.white_point <= self.black_point {
            self.white_point = self.black_point.saturating_add(1);
            if self.white_point == self.black_point {
                self.black_point = self.white_point.saturating_sub(1);
            }
        }
        self.midtone = self.midtone.clamp(0.1, 10.0);
        self.creative_lut.sanitize();
    }
}

pub type SharedCreativeLut = Arc<CubeLutParams>;

#[derive(Clone, Debug)]
pub struct CreativeLutChoice {
    pub id: Uuid,
    pub name: String,
    pub path: String,
    pub loaded: bool,
    pub error: Option<String>,
}

/// Immutable state sent to the native video presenter. The parsed LUT table is
/// shared; no file access or parsing occurs on the presenter thread.
#[derive(Clone, Debug)]
pub struct VideoGradeSnapshot {
    pub adjustments: VideoAdjustments,
    pub choices: Arc<[CreativeLutChoice]>,
    pub lut: Option<SharedCreativeLut>,
}

impl Default for VideoGradeSnapshot {
    fn default() -> Self {
        Self {
            adjustments: VideoAdjustments::default(),
            choices: Arc::from([]),
            lut: None,
        }
    }
}

struct LoadBatch {
    signature: Vec<(Uuid, PathBuf)>,
    rx: mpsc::Receiver<Vec<(Uuid, Result<CubeLutParams, String>)>>,
}

/// Runtime cache owned by `App`. File reads and `.cube` parsing happen in one
/// background batch whenever the registered entry signature changes.
#[derive(Default)]
pub struct CreativeLutLibrary {
    loaded: HashMap<Uuid, SharedCreativeLut>,
    errors: HashMap<Uuid, String>,
    signature: Vec<(Uuid, PathBuf)>,
    pending: Option<LoadBatch>,
}

impl CreativeLutLibrary {
    pub fn new(entries: &[CreativeLutEntry]) -> Self {
        let mut this = Self::default();
        this.reload(entries);
        this
    }

    pub fn reload(&mut self, entries: &[CreativeLutEntry]) {
        let signature = entry_signature(entries);
        if self.signature == signature
            && self
                .pending
                .as_ref()
                .is_none_or(|pending| pending.signature == signature)
        {
            return;
        }
        self.signature = signature.clone();
        self.loaded
            .retain(|id, _| signature.iter().any(|(entry_id, _)| entry_id == id));
        self.errors
            .retain(|id, _| signature.iter().any(|(entry_id, _)| entry_id == id));

        if signature.is_empty() {
            self.pending = None;
            return;
        }

        let (tx, rx) = mpsc::channel();
        let worker_signature = signature.clone();
        let spawned = std::thread::Builder::new()
            .name("creative-lut-loader".to_owned())
            .spawn(move || {
                let results = worker_signature
                    .iter()
                    .map(|(id, path)| (*id, load_cube_file(path)))
                    .collect();
                let _ = tx.send(results);
            });
        match spawned {
            Ok(_) => self.pending = Some(LoadBatch { signature, rx }),
            Err(error) => {
                self.pending = None;
                let message = format!("LUTの読み込みスレッドを開始できません: {error}");
                self.errors
                    .extend(signature.into_iter().map(|(id, _)| (id, message.clone())));
            }
        }
    }

    pub fn poll(&mut self, entries: &[CreativeLutEntry], ctx: &egui::Context) -> bool {
        self.reload(entries);
        let Some(pending) = self.pending.as_ref() else {
            return false;
        };
        let Ok(results) = pending.rx.try_recv() else {
            return false;
        };
        let pending = self.pending.take().expect("checked above");
        if pending.signature != self.signature {
            return false;
        }

        self.loaded.clear();
        self.errors.clear();
        for (id, result) in results {
            match result {
                Ok(lut) => {
                    self.loaded.insert(id, Arc::new(lut));
                }
                Err(error) => {
                    self.errors.insert(id, error);
                }
            }
        }
        ctx.request_repaint();
        true
    }

    pub fn get(&self, id: Option<Uuid>) -> Option<SharedCreativeLut> {
        id.and_then(|id| self.loaded.get(&id).cloned())
    }

    pub fn error(&self, id: Uuid) -> Option<&str> {
        self.errors.get(&id).map(String::as_str)
    }

    pub fn loaded_ids(&self) -> HashSet<Uuid> {
        self.loaded.keys().copied().collect()
    }

    pub fn video_snapshot(
        &self,
        entries: &[CreativeLutEntry],
        adjustments: &VideoAdjustments,
    ) -> VideoGradeSnapshot {
        let choices = entries
            .iter()
            .map(|entry| CreativeLutChoice {
                id: entry.id,
                name: entry.name.clone(),
                path: entry.path.display().to_string(),
                loaded: self.loaded.contains_key(&entry.id),
                error: self.errors.get(&entry.id).cloned(),
            })
            .collect::<Vec<_>>()
            .into();
        VideoGradeSnapshot {
            adjustments: adjustments.clone(),
            choices,
            lut: self.get(adjustments.creative_lut.id),
        }
    }
}

fn entry_signature(entries: &[CreativeLutEntry]) -> Vec<(Uuid, PathBuf)> {
    entries
        .iter()
        .map(|entry| (entry.id, entry.path.clone()))
        .collect()
}

pub fn load_cube_file(path: &Path) -> Result<CubeLutParams, String> {
    const MAX_CUBE_FILE_BYTES: u64 = 128 * 1024 * 1024;
    let metadata = std::fs::metadata(path)
        .map_err(|error| format!("{} の情報を取得できません: {error}", path.display()))?;
    if metadata.len() > MAX_CUBE_FILE_BYTES {
        return Err(format!(
            "LUTファイルが大きすぎます（上限 {} MiB）",
            MAX_CUBE_FILE_BYTES / 1024 / 1024
        ));
    }
    let text = std::fs::read_to_string(path)
        .map_err(|error| format!("{} を読み込めません: {error}", path.display()))?;
    let fallback = path
        .file_stem()
        .and_then(|value| value.to_str())
        .unwrap_or("LUT");
    local_adjust_core::parse_cube_lut(&text, fallback)
}

pub fn apply_to_color_image(
    source: &egui::ColorImage,
    lut: &CubeLutParams,
    strength: f32,
) -> egui::ColorImage {
    let strength = strength.clamp(0.0, 1.0);
    if strength <= f32::EPSILON || !lut.is_loaded() {
        return source.clone();
    }
    let pixels = source
        .pixels
        .iter()
        .map(|pixel| {
            let [r, g, b, a] = pixel.to_srgba_unmultiplied();
            let input = [r as f32 / 255.0, g as f32 / 255.0, b as f32 / 255.0];
            let output = local_adjust_core::sample_cube_lut(lut, input);
            let mix = |original: f32, graded: f32| {
                ((original + (graded - original) * strength).clamp(0.0, 1.0) * 255.0).round() as u8
            };
            egui::Color32::from_rgba_unmultiplied(
                mix(input[0], output[0]),
                mix(input[1], output[1]),
                mix(input[2], output[2]),
                a,
            )
        })
        .collect();
    egui::ColorImage::new(source.size, pixels)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identity_selection_does_not_enable_processing() {
        assert!(CreativeLutSelection::default().is_identity());
        let selection = CreativeLutSelection {
            id: Some(Uuid::new_v4()),
            strength: 0.0,
        };
        assert!(selection.is_identity());
    }

    #[test]
    fn video_adjustment_sanitize_preserves_valid_levels_order() {
        let mut params = VideoAdjustments {
            black_point: 240,
            white_point: 10,
            ..Default::default()
        };
        params.sanitize();
        assert!(params.white_point > params.black_point);
    }

    #[test]
    fn creative_lut_is_applied_with_strength_and_preserves_alpha() {
        let lut = local_adjust_core::parse_cube_lut(
            r#"
LUT_3D_SIZE 2
1.0 1.0 1.0
0.0 1.0 1.0
1.0 0.0 1.0
0.0 0.0 1.0
1.0 1.0 0.0
0.0 1.0 0.0
1.0 0.0 0.0
0.0 0.0 0.0
"#,
            "invert",
        )
        .expect("valid LUT");
        let source = egui::ColorImage::new(
            [1, 1],
            vec![egui::Color32::from_rgba_unmultiplied(0, 128, 255, 77)],
        );

        let output = apply_to_color_image(&source, &lut, 0.5);
        let [r, g, b, a] = output.pixels[0].to_srgba_unmultiplied();

        assert!((127..=129).contains(&r));
        assert!((127..=129).contains(&g));
        assert!((127..=129).contains(&b));
        assert_eq!(a, 77);
    }
}
