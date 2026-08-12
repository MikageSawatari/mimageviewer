//! 最後段 crop preview / export crop のページ単位設定。
//!
//! Crop は補正レイヤーや隠蔽加工の内部画像サイズを変えず、表示・コピー・書き出しの
//! 最後にだけ source image coordinate の矩形で切り出す。
//!
//! 矩形は、それを作成したラスタの `source_size` と組にして保存する。同じ PDF ページの
//! 再レンダや AI アップスケールで手元のラスタ寸法が変わっても、適用時はこの基準寸法から
//! 対象ラスタへ変換し、選択領域の意味を維持する。

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
    /// `rect` を作成したラスタのピクセル寸法。
    ///
    /// v2.13.0 以前の DB / sidecar / metadata transfer には存在しないため、`None` は
    /// legacy 行を表す。最初に利用可能なラスタ寸法を採用した時点で書き戻す。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_size: Option<[usize; 2]>,
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
    pub fn authored(rect: CropRect, aspect_mode: CropAspectMode, source_size: [usize; 2]) -> Self {
        let source_size = valid_size_or_one(source_size);
        Self {
            rect,
            aspect_mode,
            source_size: Some(source_size),
        }
        .sanitized(source_size[0], source_size[1])
    }

    /// legacy 設定へ最初の利用可能ラスタ寸法を採用する。既に有効な基準寸法があれば保つ。
    ///
    /// **矩形が収まらない寸法は採用しない。** 保存済みの矩形が `fallback_size` からはみ出す
    /// なら、その寸法は矩形を作ったラスタではないと確定できる (作成時に必ず clamp される
    /// ため)。ここで採用すると `sanitized` が矩形を切り落とし、それを永続化した時点で
    /// 元の選択領域が復元不能になる。PDF は低解像度ラスタが先に届いてから 4 倍の再レンダが
    /// 来るので、この順序は普通に起きる。採用を見送った設定は legacy のまま残り、次に
    /// 十分な大きさのラスタが来たときに採用される。
    pub fn with_legacy_source_size(self, fallback_size: [usize; 2]) -> Self {
        if self.valid_source_size().is_some() {
            return self;
        }
        let fallback_size = valid_size_or_one(fallback_size);
        if !self.rect_fits_within(fallback_size) {
            return self;
        }
        Self {
            rect: self.rect.sanitized(fallback_size[0], fallback_size[1]),
            aspect_mode: self.aspect_mode,
            source_size: Some(fallback_size),
        }
    }

    /// `rect` が `size` のラスタに収まるか (= その寸法が作成時の基準であり得るか)。
    fn rect_fits_within(self, size: [usize; 2]) -> bool {
        let [width, height] = valid_size_or_one(size);
        // 作成時は `sanitized` を通っているので、基準ラスタなら 0.5px の丸め以内に収まる。
        self.rect.max_x <= width as f32 + 0.5 && self.rect.max_y <= height as f32 + 0.5
    }

    pub fn valid_source_size(self) -> Option<[usize; 2]> {
        self.source_size.filter(|[width, height]| {
            *width > 0
                && *height > 0
                && i64::try_from(*width).is_ok()
                && i64::try_from(*height).is_ok()
        })
    }

    /// 保存済みの基準寸法から `target_size` のピクセル座標へ変換する。
    ///
    /// 戻り値も target 座標の自己記述設定なので、そのまま別ページへの貼り付け結果として
    /// 永続化できる。legacy 設定は target を最初の基準とみなす。
    pub fn scaled_to(self, target_size: [usize; 2]) -> Self {
        let target_size = valid_size_or_one(target_size);
        let source = self.with_legacy_source_size(target_size);
        let [source_w, source_h] = source.valid_source_size().unwrap_or(target_size);
        let source_rect = source.rect.sanitized(source_w, source_h);
        let sx = target_size[0] as f32 / source_w as f32;
        let sy = target_size[1] as f32 / source_h as f32;
        Self {
            rect: CropRect {
                min_x: source_rect.min_x * sx,
                min_y: source_rect.min_y * sy,
                max_x: source_rect.max_x * sx,
                max_y: source_rect.max_y * sy,
            }
            .sanitized(target_size[0], target_size[1]),
            aspect_mode: source.aspect_mode,
            source_size: Some(target_size),
        }
    }

    pub fn sanitized(self, width: usize, height: usize) -> Self {
        Self {
            rect: self.rect.sanitized(width, height),
            aspect_mode: self.aspect_mode,
            source_size: self.source_size,
        }
    }

    pub fn is_full(self, width: usize, height: usize) -> bool {
        self.rect.is_full(width, height)
    }
}

fn valid_size_or_one([width, height]: [usize; 2]) -> [usize; 2] {
    [width.max(1), height.max(1)]
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
        let mut conn = rusqlite::Connection::open(path)?;
        let tx = conn.transaction()?;
        tx.execute_batch(
            "CREATE TABLE IF NOT EXISTS export_crop_pages (
                page_path   TEXT PRIMARY KEY,
                min_x       REAL NOT NULL,
                min_y       REAL NOT NULL,
                max_x       REAL NOT NULL,
                max_y       REAL NOT NULL,
                aspect_mode TEXT NOT NULL DEFAULT 'free',
                source_width  INTEGER,
                source_height INTEGER,
                updated_at  INTEGER NOT NULL DEFAULT (unixepoch())
             );",
        )?;
        let columns = {
            let mut stmt = tx.prepare("PRAGMA table_info(export_crop_pages)")?;
            let rows = stmt.query_map([], |row| {
                let name: String = row.get(1)?;
                Ok(name)
            })?;
            let mut columns = HashSet::new();
            for name in rows {
                columns.insert(name?);
            }
            columns
        };
        // v2.13.0 以前のリリース済み schema は基準寸法を持たない。既存行は NULL のまま
        // 保持し、最初に利用可能なラスタを採用する遅延 migration へ渡す。
        if !columns.contains("source_width") {
            tx.execute(
                "ALTER TABLE export_crop_pages ADD COLUMN source_width INTEGER",
                [],
            )?;
        }
        if !columns.contains("source_height") {
            tx.execute(
                "ALTER TABLE export_crop_pages ADD COLUMN source_height INTEGER",
                [],
            )?;
        }
        tx.commit()?;
        Ok(Self { conn })
    }

    pub fn open_readonly(path: &Path) -> Result<Self, rusqlite::Error> {
        let conn = rusqlite::Connection::open_with_flags(
            path,
            rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY
                | rusqlite::OpenFlags::SQLITE_OPEN_URI
                | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )?;
        conn.busy_timeout(std::time::Duration::from_millis(750))?;
        Ok(Self { conn })
    }

    pub fn db_path() -> PathBuf {
        crate::data_dir::get().join("export_crop.db")
    }

    pub fn get(&self, page_key: &str) -> Option<CropSettings> {
        let mut stmt = self
            .conn
            .prepare_cached(
                "SELECT min_x, min_y, max_x, max_y, aspect_mode,
                        source_width, source_height
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
                source_size: read_source_size(row, 5, 6)?,
            })
        })
        .ok()
    }

    pub(crate) fn get_checked(&self, page_key: &str) -> Result<Option<CropSettings>, String> {
        use rusqlite::OptionalExtension as _;
        let mut stmt = self
            .conn
            .prepare_cached(
                "SELECT min_x, min_y, max_x, max_y, aspect_mode,
                        source_width, source_height
                 FROM export_crop_pages WHERE page_path = ?1",
            )
            .map_err(|error| error.to_string())?;
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
                source_size: read_source_size(row, 5, 6)?,
            })
        })
        .optional()
        .map_err(|error| error.to_string())
    }

    pub fn set(&self, page_key: &str, settings: CropSettings) -> Result<(), rusqlite::Error> {
        self.conn.execute(
            "INSERT INTO export_crop_pages
                (page_path, min_x, min_y, max_x, max_y, aspect_mode,
                 source_width, source_height, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, unixepoch())
             ON CONFLICT(page_path) DO UPDATE SET
                min_x = ?2,
                min_y = ?3,
                max_x = ?4,
                max_y = ?5,
                aspect_mode = ?6,
                source_width = ?7,
                source_height = ?8,
                updated_at = unixepoch()",
            rusqlite::params![
                page_key,
                settings.rect.min_x,
                settings.rect.min_y,
                settings.rect.max_x,
                settings.rect.max_y,
                settings.aspect_mode.stable_key(),
                settings
                    .valid_source_size()
                    .and_then(|size| i64::try_from(size[0]).ok()),
                settings
                    .valid_source_size()
                    .and_then(|size| i64::try_from(size[1]).ok()),
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
                (page_path, min_x, min_y, max_x, max_y, aspect_mode,
                 source_width, source_height, updated_at)
             SELECT ?2, min_x, min_y, max_x, max_y, aspect_mode,
                    source_width, source_height, unixepoch()
             FROM export_crop_pages WHERE page_path = ?1
             ON CONFLICT(page_path) DO UPDATE SET
                min_x = excluded.min_x,
                min_y = excluded.min_y,
                max_x = excluded.max_x,
                max_y = excluded.max_y,
                aspect_mode = excluded.aspect_mode,
                source_width = excluded.source_width,
                source_height = excluded.source_height,
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
            "SELECT page_path, min_x, min_y, max_x, max_y, aspect_mode,
                    source_width, source_height
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
                    source_size: read_source_size(row, 6, 7)?,
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
                "SELECT page_path, min_x, min_y, max_x, max_y, aspect_mode,
                        source_width, source_height
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
                            source_size: read_source_size(row, 6, 7)?,
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

pub(crate) fn read_source_size(
    row: &rusqlite::Row<'_>,
    width_index: usize,
    height_index: usize,
) -> rusqlite::Result<Option<[usize; 2]>> {
    let width = row.get::<_, Option<i64>>(width_index)?;
    let height = row.get::<_, Option<i64>>(height_index)?;
    Ok(match (width, height) {
        (Some(width), Some(height)) if width > 0 && height > 0 => {
            match (usize::try_from(width), usize::try_from(height)) {
                (Ok(width), Ok(height)) => Some([width, height]),
                _ => None,
            }
        }
        _ => None,
    })
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
            source_size: Some([100, 80]),
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
            source_size: Some([100, 100]),
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
            source_size: Some([10, 10]),
        };
        db.set("c:/imgs/a_[one].png", settings).unwrap();
        db.set("c:/imgs/a_xone].png", settings).unwrap();

        let got = db.load_by_prefix("c:/imgs/a_[");

        assert_eq!(got.len(), 1);
        assert!(got.contains_key("c:/imgs/a_[one].png"));
    }

    #[test]
    fn crop_keeps_the_same_region_across_pdf_rerender_sizes() {
        let authored = CropSettings::authored(
            CropRect {
                min_x: 113.5,
                min_y: 173.0,
                max_x: 908.0,
                max_y: 1_384.0,
            },
            CropAspectMode::Free,
            [1_135, 1_730],
        );

        let rerendered = authored.scaled_to([4_540, 6_920]);

        assert_eq!(rerendered.source_size, Some([4_540, 6_920]));
        assert_eq!(rerendered.rect.min_x, 454.0);
        assert_eq!(rerendered.rect.min_y, 692.0);
        assert_eq!(rerendered.rect.max_x, 3_632.0);
        assert_eq!(rerendered.rect.max_y, 5_536.0);
        assert_eq!(
            rerendered.scaled_to([1_135, 1_730]).rect,
            authored.rect,
            "switching between the observed 4x PDF rasters must preserve the selected region"
        );
    }

    #[test]
    fn legacy_crop_does_not_adopt_a_raster_too_small_to_hold_it() {
        // 実ログの PDF page 0 は 1135x1730 の低解像度ラスタが先に届き、その後 4540x6920 の
        // 再レンダが来る。4540 基準で作られた legacy 矩形を先着の 1135 で採用してしまうと、
        // clamp で選択領域が切り落とされ、書き戻した時点で復元できなくなる。
        let legacy = CropSettings {
            rect: CropRect {
                min_x: 454.0,
                min_y: 692.0,
                max_x: 3_632.0,
                max_y: 5_536.0,
            },
            aspect_mode: CropAspectMode::Free,
            source_size: None,
        };

        let refused = legacy.with_legacy_source_size([1_135, 1_730]);
        assert_eq!(
            refused.source_size, None,
            "a raster smaller than the stored rect cannot be the authoring basis"
        );
        assert_eq!(
            refused.rect, legacy.rect,
            "refusing adoption must leave the stored rect untouched"
        );

        let adopted = refused.with_legacy_source_size([4_540, 6_920]);
        assert_eq!(adopted.source_size, Some([4_540, 6_920]));
        assert_eq!(adopted.rect, legacy.rect);
    }

    #[test]
    fn legacy_db_row_is_preserved_then_adopts_first_raster_size() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("crop.db");
        let legacy = rusqlite::Connection::open(&path).unwrap();
        legacy
            .execute_batch(
                "CREATE TABLE export_crop_pages (
                    page_path TEXT PRIMARY KEY,
                    min_x REAL NOT NULL, min_y REAL NOT NULL,
                    max_x REAL NOT NULL, max_y REAL NOT NULL,
                    aspect_mode TEXT NOT NULL DEFAULT 'free',
                    updated_at INTEGER NOT NULL DEFAULT (unixepoch())
                 );
                 INSERT INTO export_crop_pages
                    (page_path, min_x, min_y, max_x, max_y, aspect_mode)
                 VALUES ('pdf::page_0', 100.0, 200.0, 900.0, 1400.0, 'keep');",
            )
            .unwrap();
        drop(legacy);

        let db = CropDb::open_at(&path).expect("released schema must migrate additively");
        let loaded = db.get("pdf::page_0").expect("legacy row must survive");
        assert_eq!(loaded.source_size, None);

        let adopted = loaded.with_legacy_source_size([1_135, 1_730]);
        db.set("pdf::page_0", adopted).unwrap();
        assert_eq!(
            db.get("pdf::page_0").unwrap().source_size,
            Some([1_135, 1_730]),
            "the first usable raster becomes the durable coordinate basis"
        );
    }
}
