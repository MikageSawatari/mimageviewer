//! 最後段 crop preview / export crop のページ単位設定。
//!
//! Crop は補正レイヤーや隠蔽加工の内部画像サイズを変えず、表示・コピー・書き出しの
//! 最後にだけ source image coordinate の矩形で切り出す。

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use eframe::egui;
use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq)]
pub struct CropRect {
    pub min_x: f32,
    pub min_y: f32,
    pub max_x: f32,
    pub max_y: f32,
}

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum CropAspectMode {
    #[default]
    Free,
    Keep,
    Square,
    Ratio4x3,
    Ratio3x4,
    /// 4:5。SNS フィードの縦長投稿でよく使われる。
    Ratio4x5,
    Ratio16x9,
    /// 1.91:1。SNS の横長フィード / リンクカードでよく使われる。
    Ratio191x100,
    Ratio9x16,
}

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq)]
pub struct CropSettings {
    pub rect: CropRect,
    #[serde(default)]
    pub aspect_mode: CropAspectMode,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CropHandle {
    Body,
    NorthWest,
    North,
    NorthEast,
    East,
    SouthEast,
    South,
    SouthWest,
    West,
}

impl CropAspectMode {
    /// ドロップダウン表示順。自由 / 現在比率 のあと、固定比率は横長 → 縦長の順に並べる
    /// (1.91:1 → 16:9 → 4:3 → 1:1 → 4:5 → 3:4 → 9:16)。
    pub const ALL: [Self; 9] = [
        Self::Free,
        Self::Keep,
        Self::Ratio191x100,
        Self::Ratio16x9,
        Self::Ratio4x3,
        Self::Square,
        Self::Ratio4x5,
        Self::Ratio3x4,
        Self::Ratio9x16,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Self::Free => "自由",
            Self::Keep => "現在比率",
            Self::Square => "1:1",
            Self::Ratio4x3 => "4:3",
            Self::Ratio3x4 => "3:4",
            Self::Ratio4x5 => "4:5 (SNS縦長)",
            Self::Ratio16x9 => "16:9",
            Self::Ratio191x100 => "1.91:1 (SNS横長)",
            Self::Ratio9x16 => "9:16",
        }
    }

    pub fn stable_key(self) -> &'static str {
        match self {
            Self::Free => "free",
            Self::Keep => "keep",
            Self::Square => "square",
            Self::Ratio4x3 => "4x3",
            Self::Ratio3x4 => "3x4",
            Self::Ratio4x5 => "4x5",
            Self::Ratio16x9 => "16x9",
            Self::Ratio191x100 => "191x100",
            Self::Ratio9x16 => "9x16",
        }
    }

    pub fn from_stable_key(key: &str) -> Self {
        match key {
            "keep" => Self::Keep,
            "square" => Self::Square,
            "4x3" => Self::Ratio4x3,
            "3x4" => Self::Ratio3x4,
            "4x5" => Self::Ratio4x5,
            "16x9" => Self::Ratio16x9,
            "191x100" => Self::Ratio191x100,
            "9x16" => Self::Ratio9x16,
            _ => Self::Free,
        }
    }

    pub fn aspect_ratio(self) -> Option<f32> {
        match self {
            Self::Free | Self::Keep => None,
            Self::Square => Some(1.0),
            Self::Ratio4x3 => Some(4.0 / 3.0),
            Self::Ratio3x4 => Some(3.0 / 4.0),
            Self::Ratio4x5 => Some(4.0 / 5.0),
            Self::Ratio16x9 => Some(16.0 / 9.0),
            Self::Ratio191x100 => Some(1.91),
            Self::Ratio9x16 => Some(9.0 / 16.0),
        }
    }
}

impl CropRect {
    pub fn full(width: usize, height: usize) -> Self {
        Self {
            min_x: 0.0,
            min_y: 0.0,
            max_x: width.max(1) as f32,
            max_y: height.max(1) as f32,
        }
    }

    pub fn width(self) -> f32 {
        (self.max_x - self.min_x).max(1.0)
    }

    pub fn height(self) -> f32 {
        (self.max_y - self.min_y).max(1.0)
    }

    pub fn is_full(self, width: usize, height: usize) -> bool {
        let full = Self::full(width, height);
        let crop = self.sanitized(width, height);
        (crop.min_x - full.min_x).abs() < 0.5
            && (crop.min_y - full.min_y).abs() < 0.5
            && (crop.max_x - full.max_x).abs() < 0.5
            && (crop.max_y - full.max_y).abs() < 0.5
    }

    pub fn sanitized(self, width: usize, height: usize) -> Self {
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

    pub fn fit_to_aspect_around_center(
        self,
        aspect_ratio: f32,
        width: usize,
        height: usize,
    ) -> Self {
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

    pub fn pixel_bounds(self, width: usize, height: usize) -> (usize, usize, usize, usize) {
        let crop = self.sanitized(width, height);
        let x0 = crop
            .min_x
            .floor()
            .clamp(0.0, width.saturating_sub(1) as f32) as usize;
        let y0 = crop
            .min_y
            .floor()
            .clamp(0.0, height.saturating_sub(1) as f32) as usize;
        let x1 = crop.max_x.ceil().clamp(1.0, width.max(1) as f32) as usize;
        let y1 = crop.max_y.ceil().clamp(1.0, height.max(1) as f32) as usize;
        (x0, y0, (x1 - x0).max(1), (y1 - y0).max(1))
    }

    pub fn to_screen_rect(self, image_rect: egui::Rect, width: usize, height: usize) -> egui::Rect {
        let crop = self.sanitized(width, height);
        let w = width.max(1) as f32;
        let h = height.max(1) as f32;
        egui::Rect::from_min_max(
            egui::pos2(
                image_rect.left() + image_rect.width() * crop.min_x / w,
                image_rect.top() + image_rect.height() * crop.min_y / h,
            ),
            egui::pos2(
                image_rect.left() + image_rect.width() * crop.max_x / w,
                image_rect.top() + image_rect.height() * crop.max_y / h,
            ),
        )
    }

    pub fn dragged(
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
                let crop_w = next.width().min(max_w);
                let crop_h = next.height().min(max_h);
                next.min_x = (self.min_x + delta_x).clamp(0.0, max_w - crop_w);
                next.min_y = (self.min_y + delta_y).clamp(0.0, max_h - crop_h);
                next.max_x = next.min_x + crop_w;
                next.max_y = next.min_y + crop_h;
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

impl CropSettings {
    pub fn sanitized(self, width: usize, height: usize) -> Self {
        Self {
            rect: self.rect.sanitized(width, height),
            aspect_mode: self.aspect_mode,
        }
    }

    pub fn is_full(self, width: usize, height: usize) -> bool {
        self.rect.is_full(width, height)
    }
}

pub fn crop_from_xywh_inputs(
    x: i32,
    y: i32,
    crop_w: i32,
    crop_h: i32,
    width: usize,
    height: usize,
    aspect_ratio: Option<f32>,
    prefer_height: bool,
) -> CropRect {
    let mut x = x.clamp(0, width.saturating_sub(1) as i32);
    let mut y = y.clamp(0, height.saturating_sub(1) as i32);
    let mut crop_w = crop_w.max(1).min(width.max(1) as i32);
    let mut crop_h = crop_h.max(1).min(height.max(1) as i32);
    if let Some(ratio) = aspect_ratio {
        let ratio = ratio.max(0.01);
        if prefer_height {
            crop_w = ((crop_h as f32 * ratio).round() as i32).max(1);
            if crop_w > width as i32 {
                crop_w = width as i32;
                crop_h = ((crop_w as f32 / ratio).round() as i32).max(1);
            }
        } else {
            crop_h = ((crop_w as f32 / ratio).round() as i32).max(1);
            if crop_h > height as i32 {
                crop_h = height as i32;
                crop_w = ((crop_h as f32 * ratio).round() as i32).max(1);
            }
        }
    }
    crop_w = crop_w.min(width.max(1) as i32);
    crop_h = crop_h.min(height.max(1) as i32);
    if x + crop_w > width as i32 {
        x = width as i32 - crop_w;
    }
    if y + crop_h > height as i32 {
        y = height as i32 - crop_h;
    }
    CropRect {
        min_x: x.max(0) as f32,
        min_y: y.max(0) as f32,
        max_x: (x + crop_w).max(1) as f32,
        max_y: (y + crop_h).max(1) as f32,
    }
    .sanitized(width, height)
}

pub fn crop_from_points(
    a: [f32; 2],
    b: [f32; 2],
    width: usize,
    height: usize,
    aspect_ratio: Option<f32>,
) -> CropRect {
    let crop = CropRect {
        min_x: a[0].min(b[0]),
        min_y: a[1].min(b[1]),
        max_x: a[0].max(b[0]),
        max_y: a[1].max(b[1]),
    }
    .sanitized(width, height);
    if let Some(ratio) = aspect_ratio {
        crop.fit_to_aspect_around_center(ratio, width, height)
    } else {
        crop
    }
}

pub fn crop_color_image(
    src: &egui::ColorImage,
    rect: CropRect,
) -> Result<egui::ColorImage, String> {
    let [width, height] = src.size;
    if width == 0 || height == 0 || src.pixels.len() != width * height {
        return Err("crop source image size is invalid".to_string());
    }
    let (x0, y0, crop_w, crop_h) = rect.pixel_bounds(width, height);
    let mut out = Vec::with_capacity(crop_w * crop_h);
    for y in y0..y0 + crop_h {
        let start = y * width + x0;
        out.extend_from_slice(&src.pixels[start..start + crop_w]);
    }
    Ok(egui::ColorImage::new([crop_w, crop_h], out))
}

pub fn crop_mask(mask: &[bool], width: usize, height: usize, rect: CropRect) -> Option<Vec<bool>> {
    if width == 0 || height == 0 || mask.len() != width.checked_mul(height)? {
        return None;
    }
    let (x0, y0, crop_w, crop_h) = rect.pixel_bounds(width, height);
    let mut out = Vec::with_capacity(crop_w * crop_h);
    for y in y0..y0 + crop_h {
        let start = y * width + x0;
        out.extend_from_slice(&mask[start..start + crop_w]);
    }
    Some(out)
}

pub struct CropDb {
    conn: rusqlite::Connection,
}

impl CropDb {
    pub fn open() -> Result<Self, rusqlite::Error> {
        Self::open_at(&Self::db_path())
    }

    pub fn open_at(path: &Path) -> Result<Self, rusqlite::Error> {
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let conn = rusqlite::Connection::open(path)?;
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS export_crop_pages (
                page_path   TEXT PRIMARY KEY,
                min_x       REAL NOT NULL,
                min_y       REAL NOT NULL,
                max_x       REAL NOT NULL,
                max_y       REAL NOT NULL,
                aspect_mode TEXT NOT NULL DEFAULT 'free',
                updated_at  INTEGER NOT NULL DEFAULT (unixepoch())
             );",
        )?;
        Ok(Self { conn })
    }

    fn db_path() -> PathBuf {
        crate::data_dir::get().join("export_crop.db")
    }

    pub fn get(&self, page_key: &str) -> Option<CropSettings> {
        let mut stmt = self
            .conn
            .prepare_cached(
                "SELECT min_x, min_y, max_x, max_y, aspect_mode
                 FROM export_crop_pages WHERE page_path = ?1",
            )
            .ok()?;
        stmt.query_row([page_key], |row| {
            let aspect: String = row.get(4)?;
            Ok(CropSettings {
                rect: CropRect {
                    min_x: row.get::<_, f32>(0)?,
                    min_y: row.get::<_, f32>(1)?,
                    max_x: row.get::<_, f32>(2)?,
                    max_y: row.get::<_, f32>(3)?,
                },
                aspect_mode: CropAspectMode::from_stable_key(&aspect),
            })
        })
        .ok()
    }

    pub fn set(&self, page_key: &str, settings: CropSettings) -> Result<(), rusqlite::Error> {
        self.conn.execute(
            "INSERT INTO export_crop_pages
                (page_path, min_x, min_y, max_x, max_y, aspect_mode, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, unixepoch())
             ON CONFLICT(page_path) DO UPDATE SET
                min_x = ?2,
                min_y = ?3,
                max_x = ?4,
                max_y = ?5,
                aspect_mode = ?6,
                updated_at = unixepoch()",
            rusqlite::params![
                page_key,
                settings.rect.min_x,
                settings.rect.min_y,
                settings.rect.max_x,
                settings.rect.max_y,
                settings.aspect_mode.stable_key(),
            ],
        )?;
        Ok(())
    }

    pub fn remove(&self, page_key: &str) -> Result<(), rusqlite::Error> {
        self.conn.execute(
            "DELETE FROM export_crop_pages WHERE page_path = ?1",
            [page_key],
        )?;
        Ok(())
    }

    pub fn copy_entry_key(&self, from_key: &str, to_key: &str) -> Result<(), rusqlite::Error> {
        if from_key == to_key {
            return Ok(());
        }
        self.conn.execute(
            "INSERT INTO export_crop_pages
                (page_path, min_x, min_y, max_x, max_y, aspect_mode, updated_at)
             SELECT ?2, min_x, min_y, max_x, max_y, aspect_mode, unixepoch()
             FROM export_crop_pages WHERE page_path = ?1
             ON CONFLICT(page_path) DO UPDATE SET
                min_x = excluded.min_x,
                min_y = excluded.min_y,
                max_x = excluded.max_x,
                max_y = excluded.max_y,
                aspect_mode = excluded.aspect_mode,
                updated_at = unixepoch()",
            rusqlite::params![from_key, to_key],
        )?;
        Ok(())
    }

    pub fn move_entry_key(&self, from_key: &str, to_key: &str) -> Result<(), rusqlite::Error> {
        if from_key == to_key {
            return Ok(());
        }
        self.copy_entry_key(from_key, to_key)?;
        self.remove(from_key)
    }

    pub fn load_keys(&self, prefix: &str) -> HashSet<String> {
        self.load_by_prefix(prefix).into_keys().collect()
    }

    pub fn load_by_prefix(&self, prefix: &str) -> HashMap<String, CropSettings> {
        let mut map = HashMap::new();
        let Ok(mut stmt) = self.conn.prepare_cached(
            "SELECT page_path, min_x, min_y, max_x, max_y, aspect_mode
             FROM export_crop_pages
             WHERE page_path LIKE ?1 ESCAPE '\\'",
        ) else {
            return map;
        };
        let escaped = prefix
            .replace('\\', "\\\\")
            .replace('%', "\\%")
            .replace('_', "\\_")
            .replace('[', "\\[");
        let pattern = format!("{escaped}%");
        let Ok(rows) = stmt.query_map([&pattern], |row| {
            let aspect: String = row.get(5)?;
            Ok((
                row.get::<_, String>(0)?,
                CropSettings {
                    rect: CropRect {
                        min_x: row.get::<_, f32>(1)?,
                        min_y: row.get::<_, f32>(2)?,
                        max_x: row.get::<_, f32>(3)?,
                        max_y: row.get::<_, f32>(4)?,
                    },
                    aspect_mode: CropAspectMode::from_stable_key(&aspect),
                },
            ))
        }) else {
            return map;
        };
        for (key, settings) in rows.flatten() {
            map.insert(key, settings);
        }
        map
    }

    /// 複数フォルダを横断する一覧向けに、指定キーだけを一括読込する。
    pub fn load_many(&self, page_keys: &[&str]) -> HashMap<String, CropSettings> {
        let mut map = HashMap::new();
        for chunk in page_keys.chunks(500) {
            if chunk.is_empty() {
                continue;
            }
            let placeholders = std::iter::repeat_n("?", chunk.len())
                .collect::<Vec<_>>()
                .join(",");
            let sql = format!(
                "SELECT page_path, min_x, min_y, max_x, max_y, aspect_mode
                 FROM export_crop_pages WHERE page_path IN ({placeholders})"
            );
            let Ok(mut stmt) = self.conn.prepare(&sql) else {
                continue;
            };
            let Ok(rows) =
                stmt.query_map(rusqlite::params_from_iter(chunk.iter().copied()), |row| {
                    let aspect: String = row.get(5)?;
                    Ok((
                        row.get::<_, String>(0)?,
                        CropSettings {
                            rect: CropRect {
                                min_x: row.get::<_, f32>(1)?,
                                min_y: row.get::<_, f32>(2)?,
                                max_x: row.get::<_, f32>(3)?,
                                max_y: row.get::<_, f32>(4)?,
                            },
                            aspect_mode: CropAspectMode::from_stable_key(&aspect),
                        },
                    ))
                })
            else {
                continue;
            };
            map.extend(rows.flatten());
        }
        map
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn aspect_mode_stable_key_round_trips_all_variants() {
        for mode in CropAspectMode::ALL {
            assert_eq!(
                CropAspectMode::from_stable_key(mode.stable_key()),
                mode,
                "stable_key round-trip failed for {mode:?}"
            );
        }
    }

    #[test]
    fn sns_aspect_ratios_have_expected_values() {
        assert_eq!(CropAspectMode::Ratio4x5.aspect_ratio(), Some(0.8));
        assert_eq!(CropAspectMode::Ratio191x100.aspect_ratio(), Some(1.91));
        // 縦長 (4:5) は < 1、横長 (1.91:1) は > 1。
        assert!(CropAspectMode::Ratio4x5.aspect_ratio().unwrap() < 1.0);
        assert!(CropAspectMode::Ratio191x100.aspect_ratio().unwrap() > 1.0);
    }

    #[test]
    fn crop_rect_sanitizes_to_image_bounds() {
        let rect = CropRect {
            min_x: 120.0,
            min_y: -10.0,
            max_x: -5.0,
            max_y: 200.0,
        }
        .sanitized(100, 80);

        assert_eq!(rect.min_x, 0.0);
        assert_eq!(rect.min_y, 0.0);
        assert_eq!(rect.max_x, 100.0);
        assert_eq!(rect.max_y, 80.0);
    }

    #[test]
    fn crop_color_image_returns_requested_region() {
        let pixels = (0..12)
            .map(|v| egui::Color32::from_rgba_unmultiplied(v, 0, 0, 255))
            .collect();
        let src = egui::ColorImage::new([4, 3], pixels);
        let out = crop_color_image(
            &src,
            CropRect {
                min_x: 1.0,
                min_y: 1.0,
                max_x: 4.0,
                max_y: 3.0,
            },
        )
        .unwrap();

        assert_eq!(out.size, [3, 2]);
        let values: Vec<u8> = out.pixels.iter().map(|p| p.r()).collect();
        assert_eq!(values, vec![5, 6, 7, 9, 10, 11]);
    }

    #[test]
    fn crop_db_round_trips_page_settings() {
        let dir = tempfile::tempdir().unwrap();
        let db = CropDb::open_at(&dir.path().join("crop.db")).unwrap();
        let key = "c:/imgs/a.png";
        let settings = CropSettings {
            rect: CropRect {
                min_x: 10.0,
                min_y: 12.0,
                max_x: 90.0,
                max_y: 60.0,
            },
            aspect_mode: CropAspectMode::Ratio4x3,
        };

        db.set(key, settings).unwrap();

        assert_eq!(db.get(key), Some(settings));
    }

    #[test]
    fn crop_db_load_many_returns_only_requested_exact_keys() {
        let dir = tempfile::tempdir().unwrap();
        let db = CropDb::open_at(&dir.path().join("crop.db")).unwrap();
        let settings = CropSettings {
            rect: CropRect {
                min_x: 1.0,
                min_y: 2.0,
                max_x: 30.0,
                max_y: 40.0,
            },
            aspect_mode: CropAspectMode::Keep,
        };
        db.set("c:/a.jpg", settings).unwrap();
        db.set("c:/b.jpg", settings).unwrap();
        let loaded = db.load_many(&["c:/b.jpg", "c:/missing.jpg"]);
        assert_eq!(loaded, HashMap::from([("c:/b.jpg".to_string(), settings)]));
    }

    #[test]
    fn crop_db_load_by_prefix_escapes_wildcards() {
        let dir = tempfile::tempdir().unwrap();
        let db = CropDb::open_at(&dir.path().join("crop.db")).unwrap();
        let settings = CropSettings {
            rect: CropRect {
                min_x: 0.0,
                min_y: 0.0,
                max_x: 10.0,
                max_y: 10.0,
            },
            aspect_mode: CropAspectMode::Free,
        };
        db.set("c:/imgs/a_[one].png", settings).unwrap();
        db.set("c:/imgs/a_xone].png", settings).unwrap();

        let got = db.load_by_prefix("c:/imgs/a_[");

        assert_eq!(got.len(), 1);
        assert!(got.contains_key("c:/imgs/a_[one].png"));
    }
}
