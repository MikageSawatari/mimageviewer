use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
    mpsc,
};
use std::time::Instant;

pub const DEFAULT_QUERY_RGB: [u8; 3] = [64, 128, 255];
pub const DEFAULT_TOLERANCE: f32 = 28.0;
pub const MIN_TOLERANCE: f32 = 6.0;
pub const MAX_TOLERANCE: f32 = 60.0;
pub const RATIO_FLOOR: f32 = 0.08;

const MAX_COLORS: usize = 8;
const MAX_CANDIDATES: usize = 32;
const MAX_SAMPLE_PIXELS: usize = 16_384;
const MERGE_DELTA_E: f32 = 10.0;

pub type ColorPaletteKey = String;
pub type ScanScopeSignature = u64;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ColorInputMode {
    Hex,
    Rgb,
    Hsl,
}

impl Default for ColorInputMode {
    fn default() -> Self {
        Self::Hex
    }
}

#[derive(Clone, Debug)]
pub struct PaletteColor {
    pub rgb: [u8; 3],
    pub ratio: f32,
    pub lab: [f32; 3],
}

#[derive(Clone, Debug, Default)]
pub struct Palette {
    pub colors: Vec<PaletteColor>,
}

#[derive(Clone, Debug)]
pub struct PaletteEntry {
    pub mtime: i64,
    pub file_size: i64,
    pub palette: Palette,
}

#[derive(Debug, Default)]
pub struct ScanPalettes {
    pub map: HashMap<ColorPaletteKey, PaletteEntry>,
    pub active_scan_id: u64,
    pub last_scope_signature: Option<ScanScopeSignature>,
}

impl ScanPalettes {
    pub fn fresh_entry(&self, key: &str, mtime: i64, file_size: i64) -> Option<&PaletteEntry> {
        let entry = self.map.get(key)?;
        (entry.mtime == mtime && entry.file_size == file_size).then_some(entry)
    }

    pub fn insert(&mut self, key: ColorPaletteKey, entry: PaletteEntry) {
        self.map.insert(key, entry);
    }

    pub fn clear(&mut self) {
        self.map.clear();
        self.last_scope_signature = None;
    }
}

#[derive(Debug)]
pub struct ColorScanPending {
    pub scan_id: u64,
    pub scope_signature: ScanScopeSignature,
    pub total: usize,
    pub done: usize,
    pub cancel: Arc<AtomicBool>,
    pub rx: mpsc::Receiver<ColorScanMessage>,
    pub started_at: Instant,
}

#[derive(Clone, Debug)]
pub struct ColorScanConfirmation {
    pub scope_signature: ScanScopeSignature,
    pub missing: usize,
}

#[derive(Clone, Debug)]
pub struct ColorScanItemResult {
    pub key: ColorPaletteKey,
    pub mtime: i64,
    pub file_size: i64,
    pub palette: Palette,
}

#[derive(Clone, Debug)]
pub enum ColorScanMessage {
    Item(ColorScanItemResult),
    Done {
        scan_id: u64,
        scope_signature: ScanScopeSignature,
        cancelled: bool,
    },
}

#[derive(Debug)]
pub struct ColorFilterState {
    pub enabled: bool,
    pub query_rgb: [u8; 3],
    pub tolerance: f32,
    pub input_mode: ColorInputMode,
    pub hex_input: String,
    pub input_has_focus: bool,
    pub picker_hue_degrees: f32,
    pub palettes: ScanPalettes,
    pub pending: Option<ColorScanPending>,
    pub confirmation: Option<ColorScanConfirmation>,
    pub confirmed_large_scan_scope: Option<ScanScopeSignature>,
    pub applied_scope_signature: Option<ScanScopeSignature>,
}

impl Default for ColorFilterState {
    fn default() -> Self {
        let (hue, _, _) = rgb_to_hsv(DEFAULT_QUERY_RGB);
        Self {
            enabled: false,
            query_rgb: DEFAULT_QUERY_RGB,
            tolerance: DEFAULT_TOLERANCE,
            input_mode: ColorInputMode::default(),
            hex_input: hex_rgb(DEFAULT_QUERY_RGB),
            input_has_focus: false,
            picker_hue_degrees: hue,
            palettes: ScanPalettes::default(),
            pending: None,
            confirmation: None,
            confirmed_large_scan_scope: None,
            applied_scope_signature: None,
        }
    }
}

impl ColorFilterState {
    pub fn cancel_pending(&mut self) {
        if let Some(pending) = self.pending.take() {
            pending.cancel.store(true, Ordering::Relaxed);
        }
    }

    pub fn clear_filter(&mut self) {
        self.cancel_pending();
        self.confirmation = None;
        self.confirmed_large_scan_scope = None;
        self.enabled = false;
        self.input_has_focus = false;
        self.applied_scope_signature = None;
    }

    pub fn clear_for_new_items(&mut self) {
        self.cancel_pending();
        self.confirmation = None;
        self.confirmed_large_scan_scope = None;
        self.enabled = false;
        self.input_has_focus = false;
        self.applied_scope_signature = None;
        self.palettes.clear();
    }

    pub fn query_lab(&self) -> [f32; 3] {
        srgb_to_lab(self.query_rgb)
    }

    pub fn set_query_rgb(&mut self, rgb: [u8; 3]) {
        self.query_rgb = rgb;
        self.hex_input = hex_rgb(rgb);
        let (hue, saturation, _) = rgb_to_hsv(rgb);
        if saturation > 0.001 {
            self.picker_hue_degrees = hue;
        }
    }
}

pub fn scan_scope_signature<I, S>(view_kind: &str, items: I) -> ScanScopeSignature
where
    I: IntoIterator<Item = (S, i64, i64)>,
    S: AsRef<str>,
{
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    view_kind.hash(&mut hasher);
    let mut count = 0usize;
    for (key, mtime, file_size) in items {
        count = count.saturating_add(1);
        key.as_ref().hash(&mut hasher);
        mtime.hash(&mut hasher);
        file_size.hash(&mut hasher);
    }
    count.hash(&mut hasher);
    hasher.finish()
}

/// 完了した色スキャンの結果をどう扱うか。`poll_color_scan` の整合判定の純粋表現。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ScanDisposition {
    /// キャンセル / フィルタ無効化済み。結果を捨てて applied を解除する。
    Drop,
    /// スキャン起動時のスコープが現在も一致。visible へ一括反映する。
    Apply,
    /// スコープ / scan_id が陳腐化。候補キャッシュへ merge 済みなので、現在の items で
    /// missing/stale を再判定して再スキャンする (古いスコープで UI を確定させない)。
    Restart,
}

/// 完了した色スキャン (`scan_id` = `finished_scan_id`, 起動時スコープ = `finished_scope`) を、
/// 現在の状態 (`active_scan_id`, `current_scope`) と突き合わせてどう扱うか決める。
///
/// 候補キャッシュへの palette merge は呼び出し側が無条件に行う前提で、ここは
/// **visible 一括反映してよいか** だけを判断する。`cancelled` か `!filter_enabled` の
/// ときは無条件 `Drop`。それ以外は scan_id と scope の両方が一致したときだけ `Apply`、
/// 片方でもずれていれば `Restart`。
pub fn scan_result_disposition(
    cancelled: bool,
    filter_enabled: bool,
    finished_scan_id: u64,
    active_scan_id: u64,
    finished_scope: ScanScopeSignature,
    current_scope: Option<ScanScopeSignature>,
) -> ScanDisposition {
    if cancelled || !filter_enabled {
        return ScanDisposition::Drop;
    }
    if finished_scan_id == active_scan_id && current_scope == Some(finished_scope) {
        ScanDisposition::Apply
    } else {
        ScanDisposition::Restart
    }
}

pub fn palette_matches(palette: &Palette, query_lab: [f32; 3], tolerance: f32) -> bool {
    palette
        .colors
        .iter()
        .any(|color| color.ratio >= RATIO_FLOOR && delta_e76(query_lab, color.lab) <= tolerance)
}

pub fn delta_e76(a: [f32; 3], b: [f32; 3]) -> f32 {
    let dl = a[0] - b[0];
    let da = a[1] - b[1];
    let db = a[2] - b[2];
    (dl * dl + da * da + db * db).sqrt()
}

pub fn srgb_to_lab(rgb: [u8; 3]) -> [f32; 3] {
    fn srgb_to_linear(v: u8) -> f32 {
        let c = v as f32 / 255.0;
        if c <= 0.04045 {
            c / 12.92
        } else {
            ((c + 0.055) / 1.055).powf(2.4)
        }
    }

    fn f(t: f32) -> f32 {
        const EPSILON: f32 = 216.0 / 24389.0;
        const KAPPA: f32 = 24389.0 / 27.0;
        if t > EPSILON {
            t.cbrt()
        } else {
            (KAPPA * t + 16.0) / 116.0
        }
    }

    let r = srgb_to_linear(rgb[0]);
    let g = srgb_to_linear(rgb[1]);
    let b = srgb_to_linear(rgb[2]);

    let x = (0.4124564 * r + 0.3575761 * g + 0.1804375 * b) / 0.95047;
    let y = 0.2126729 * r + 0.7151522 * g + 0.0721750 * b;
    let z = (0.0193339 * r + 0.1191920 * g + 0.9503041 * b) / 1.08883;

    let fx = f(x);
    let fy = f(y);
    let fz = f(z);
    [116.0 * fy - 16.0, 500.0 * (fx - fy), 200.0 * (fy - fz)]
}

pub fn extract_palette_from_color_image(image: &egui::ColorImage) -> Palette {
    let [w, h] = image.size;
    if w == 0 || h == 0 || image.pixels.is_empty() {
        return Palette::default();
    }

    let total = w.saturating_mul(h).max(1);
    let stride = ((total as f64 / MAX_SAMPLE_PIXELS as f64).sqrt().ceil() as usize).max(1);
    let mut samples: Vec<[u8; 3]> = Vec::with_capacity((total / stride).min(MAX_SAMPLE_PIXELS));
    for y in (0..h).step_by(stride) {
        for x in (0..w).step_by(stride) {
            let p = image.pixels[y * w + x];
            if p.a() < 16 {
                continue;
            }
            samples.push([p.r(), p.g(), p.b()]);
        }
    }

    extract_palette_from_rgb_samples(&samples)
}

pub fn extract_palette_from_rgb_samples(samples: &[[u8; 3]]) -> Palette {
    if samples.is_empty() {
        return Palette::default();
    }

    #[derive(Clone, Debug)]
    struct Accum {
        count: u32,
        sum: [u64; 3],
    }

    let mut bins: HashMap<u16, Accum> = HashMap::new();
    for &rgb in samples {
        let key =
            (((rgb[0] >> 4) as u16) << 8) | (((rgb[1] >> 4) as u16) << 4) | ((rgb[2] >> 4) as u16);
        let entry = bins.entry(key).or_insert(Accum {
            count: 0,
            sum: [0, 0, 0],
        });
        entry.count += 1;
        entry.sum[0] += rgb[0] as u64;
        entry.sum[1] += rgb[1] as u64;
        entry.sum[2] += rgb[2] as u64;
    }

    #[derive(Clone, Debug)]
    struct Candidate {
        rgb: [u8; 3],
        lab: [f32; 3],
        weight: u32,
    }

    let mut candidates: Vec<Candidate> = bins
        .into_values()
        .map(|acc| {
            let rgb = [
                (acc.sum[0] / acc.count as u64) as u8,
                (acc.sum[1] / acc.count as u64) as u8,
                (acc.sum[2] / acc.count as u64) as u8,
            ];
            Candidate {
                rgb,
                lab: srgb_to_lab(rgb),
                weight: acc.count,
            }
        })
        .collect();
    candidates.sort_by(|a, b| b.weight.cmp(&a.weight).then_with(|| a.rgb.cmp(&b.rgb)));
    candidates.truncate(MAX_CANDIDATES);

    let mut merged: Vec<Candidate> = Vec::new();
    for cand in candidates {
        if let Some(existing) = merged
            .iter_mut()
            .find(|existing| delta_e76(existing.lab, cand.lab) <= MERGE_DELTA_E)
        {
            let total = existing.weight + cand.weight;
            for ch in 0..3 {
                existing.rgb[ch] = ((existing.rgb[ch] as u32 * existing.weight
                    + cand.rgb[ch] as u32 * cand.weight)
                    / total) as u8;
            }
            existing.weight = total;
            existing.lab = srgb_to_lab(existing.rgb);
        } else {
            merged.push(cand);
        }
    }

    if merged.is_empty() {
        return Palette::default();
    }

    let mut reassigned: Vec<Accum> = merged
        .iter()
        .map(|_| Accum {
            count: 0,
            sum: [0, 0, 0],
        })
        .collect();

    for &rgb in samples {
        let lab = srgb_to_lab(rgb);
        let nearest = merged
            .iter()
            .enumerate()
            .min_by(|(_, a), (_, b)| {
                delta_e76(lab, a.lab)
                    .partial_cmp(&delta_e76(lab, b.lab))
                    .unwrap_or(std::cmp::Ordering::Equal)
            })
            .map(|(idx, _)| idx)
            .unwrap_or(0);
        let acc = &mut reassigned[nearest];
        acc.count += 1;
        acc.sum[0] += rgb[0] as u64;
        acc.sum[1] += rgb[1] as u64;
        acc.sum[2] += rgb[2] as u64;
    }

    let total = samples.len() as f32;
    let mut colors: Vec<PaletteColor> = reassigned
        .into_iter()
        .filter(|acc| acc.count > 0)
        .map(|acc| {
            let rgb = [
                (acc.sum[0] / acc.count as u64) as u8,
                (acc.sum[1] / acc.count as u64) as u8,
                (acc.sum[2] / acc.count as u64) as u8,
            ];
            PaletteColor {
                rgb,
                ratio: acc.count as f32 / total,
                lab: srgb_to_lab(rgb),
            }
        })
        .collect();
    colors.sort_by(|a, b| {
        b.ratio
            .partial_cmp(&a.ratio)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| a.rgb.cmp(&b.rgb))
    });
    colors.truncate(MAX_COLORS);
    Palette { colors }
}

pub fn hex_rgb(rgb: [u8; 3]) -> String {
    format!("#{:02X}{:02X}{:02X}", rgb[0], rgb[1], rgb[2])
}

pub fn parse_hex_rgb(input: &str) -> Option<[u8; 3]> {
    let trimmed = input.trim();
    let hex = trimmed.strip_prefix('#').unwrap_or(trimmed);
    if hex.len() != 6 || !hex.chars().all(|c| c.is_ascii_hexdigit()) {
        return None;
    }
    let value = u32::from_str_radix(hex, 16).ok()?;
    Some([
        ((value >> 16) & 0xff) as u8,
        ((value >> 8) & 0xff) as u8,
        (value & 0xff) as u8,
    ])
}

pub fn rgb_to_hsv(rgb: [u8; 3]) -> (f32, f32, f32) {
    let r = rgb[0] as f32 / 255.0;
    let g = rgb[1] as f32 / 255.0;
    let b = rgb[2] as f32 / 255.0;
    let max = r.max(g).max(b);
    let min = r.min(g).min(b);
    let delta = max - min;
    let hue = if delta <= f32::EPSILON {
        0.0
    } else if (max - r).abs() <= f32::EPSILON {
        60.0 * ((g - b) / delta).rem_euclid(6.0)
    } else if (max - g).abs() <= f32::EPSILON {
        60.0 * (((b - r) / delta) + 2.0)
    } else {
        60.0 * (((r - g) / delta) + 4.0)
    };
    let saturation = if max <= f32::EPSILON {
        0.0
    } else {
        delta / max
    };
    (hue.rem_euclid(360.0), saturation, max)
}

pub fn hsv_to_rgb(hue_degrees: f32, saturation: f32, value: f32) -> [u8; 3] {
    let h = hue_degrees.rem_euclid(360.0) / 60.0;
    let s = saturation.clamp(0.0, 1.0);
    let v = value.clamp(0.0, 1.0);
    let c = v * s;
    let x = c * (1.0 - (h.rem_euclid(2.0) - 1.0).abs());
    let m = v - c;
    let (r1, g1, b1) = if h < 1.0 {
        (c, x, 0.0)
    } else if h < 2.0 {
        (x, c, 0.0)
    } else if h < 3.0 {
        (0.0, c, x)
    } else if h < 4.0 {
        (0.0, x, c)
    } else if h < 5.0 {
        (x, 0.0, c)
    } else {
        (c, 0.0, x)
    };
    [
        ((r1 + m) * 255.0).round().clamp(0.0, 255.0) as u8,
        ((g1 + m) * 255.0).round().clamp(0.0, 255.0) as u8,
        ((b1 + m) * 255.0).round().clamp(0.0, 255.0) as u8,
    ]
}

pub fn rgb_to_hsl(rgb: [u8; 3]) -> (f32, f32, f32) {
    let r = rgb[0] as f32 / 255.0;
    let g = rgb[1] as f32 / 255.0;
    let b = rgb[2] as f32 / 255.0;
    let max = r.max(g).max(b);
    let min = r.min(g).min(b);
    let lightness = (max + min) * 0.5;
    let delta = max - min;
    if delta <= f32::EPSILON {
        return (0.0, 0.0, lightness);
    }
    let saturation = delta / (1.0 - (2.0 * lightness - 1.0).abs());
    let hue = if (max - r).abs() <= f32::EPSILON {
        60.0 * ((g - b) / delta).rem_euclid(6.0)
    } else if (max - g).abs() <= f32::EPSILON {
        60.0 * (((b - r) / delta) + 2.0)
    } else {
        60.0 * (((r - g) / delta) + 4.0)
    };
    (hue.rem_euclid(360.0), saturation, lightness)
}

pub fn hsl_to_rgb(hue_degrees: f32, saturation: f32, lightness: f32) -> [u8; 3] {
    let h = hue_degrees.rem_euclid(360.0) / 60.0;
    let s = saturation.clamp(0.0, 1.0);
    let l = lightness.clamp(0.0, 1.0);
    let c = (1.0 - (2.0 * l - 1.0).abs()) * s;
    let x = c * (1.0 - (h.rem_euclid(2.0) - 1.0).abs());
    let m = l - c * 0.5;
    let (r1, g1, b1) = if h < 1.0 {
        (c, x, 0.0)
    } else if h < 2.0 {
        (x, c, 0.0)
    } else if h < 3.0 {
        (0.0, c, x)
    } else if h < 4.0 {
        (0.0, x, c)
    } else if h < 5.0 {
        (x, 0.0, c)
    } else {
        (c, 0.0, x)
    };
    [
        ((r1 + m) * 255.0).round().clamp(0.0, 255.0) as u8,
        ((g1 + m) * 255.0).round().clamp(0.0, 255.0) as u8,
        ((b1 + m) * 255.0).round().clamp(0.0, 255.0) as u8,
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hex_rgb_parser_accepts_hash_and_plain_forms() {
        assert_eq!(parse_hex_rgb("#5C22B3"), Some([92, 34, 179]));
        assert_eq!(parse_hex_rgb("5c22b3"), Some([92, 34, 179]));
        assert_eq!(parse_hex_rgb("#xyzxyz"), None);
        assert_eq!(parse_hex_rgb("#12345"), None);
    }

    #[test]
    fn hsv_and_hsl_round_trip_primary_color() {
        let rgb = [92, 34, 179];
        let (h, s, v) = rgb_to_hsv(rgb);
        assert_eq!(hsv_to_rgb(h, s, v), rgb);
        let (h, s, l) = rgb_to_hsl(rgb);
        assert_eq!(hsl_to_rgb(h, s, l), rgb);
    }

    #[test]
    fn solid_color_palette_is_single_dominant_color() {
        let samples = vec![[32, 64, 128]; 100];
        let palette = extract_palette_from_rgb_samples(&samples);
        assert_eq!(palette.colors.len(), 1);
        assert!(palette.colors[0].ratio > 0.99);
        assert!(palette_matches(
            &palette,
            srgb_to_lab([32, 64, 128]),
            DEFAULT_TOLERANCE
        ));
    }

    #[test]
    fn two_color_palette_keeps_ratios() {
        let mut samples = vec![[200, 20, 20]; 80];
        samples.extend(std::iter::repeat_n([20, 30, 210], 20));
        let palette = extract_palette_from_rgb_samples(&samples);
        assert!(palette.colors.len() >= 2);
        assert!((palette.colors[0].ratio - 0.8).abs() < 0.05);
        assert!((palette.colors[1].ratio - 0.2).abs() < 0.05);
    }

    #[test]
    fn similar_gradient_colors_merge() {
        let samples: Vec<[u8; 3]> = (0..120)
            .map(|i| {
                let v = 100 + (i % 16) as u8;
                [v, 120, 140]
            })
            .collect();
        let palette = extract_palette_from_rgb_samples(&samples);
        assert_eq!(palette.colors.len(), 1);
        assert!(palette.colors[0].ratio > 0.95);
    }

    #[test]
    fn scope_signature_changes_with_mtime() {
        let a = scan_scope_signature("folder", [("a.jpg", 1, 10)]);
        let b = scan_scope_signature("folder", [("a.jpg", 2, 10)]);
        assert_ne!(a, b);
    }

    #[test]
    fn scope_signature_changes_with_item_set() {
        let a = scan_scope_signature("folder", [("a.jpg", 1, 10)]);
        let b = scan_scope_signature("folder", [("a.jpg", 1, 10), ("b.jpg", 1, 10)]);
        assert_ne!(a, b);
    }

    #[test]
    fn scope_signature_changes_with_view_kind() {
        let a = scan_scope_signature("folder", [("a.jpg", 1, 10)]);
        let b = scan_scope_signature("tag", [("a.jpg", 1, 10)]);
        assert_ne!(a, b);
    }

    #[test]
    fn fresh_entry_rejects_stale_mtime_or_size() {
        let mut palettes = ScanPalettes::default();
        palettes.insert(
            "k".to_string(),
            PaletteEntry {
                mtime: 100,
                file_size: 2000,
                palette: Palette::default(),
            },
        );
        // 同名 + 同 mtime/size のときだけ再利用できる。
        assert!(palettes.fresh_entry("k", 100, 2000).is_some());
        // 同名でも mtime が変われば stale (= 同名差し替え)。
        assert!(palettes.fresh_entry("k", 101, 2000).is_none());
        // 同名でも file_size が変われば stale。
        assert!(palettes.fresh_entry("k", 100, 2001).is_none());
        // 未知キーは None。
        assert!(palettes.fresh_entry("missing", 100, 2000).is_none());
    }

    #[test]
    fn disposition_drops_on_cancel_or_disabled() {
        // cancelled なら scope が一致していても Drop。
        assert_eq!(
            scan_result_disposition(true, true, 5, 5, 9, Some(9)),
            ScanDisposition::Drop
        );
        // filter 無効化済みでも Drop。
        assert_eq!(
            scan_result_disposition(false, false, 5, 5, 9, Some(9)),
            ScanDisposition::Drop
        );
    }

    #[test]
    fn disposition_applies_only_on_scan_id_and_scope_match() {
        assert_eq!(
            scan_result_disposition(false, true, 5, 5, 9, Some(9)),
            ScanDisposition::Apply
        );
    }

    #[test]
    fn disposition_restarts_on_stale_scan_id_or_scope() {
        // 後から新しいスキャンが始まっている (active_scan_id が進んだ)。
        assert_eq!(
            scan_result_disposition(false, true, 4, 5, 9, Some(9)),
            ScanDisposition::Restart
        );
        // items 集合が変わってスコープがずれた。
        assert_eq!(
            scan_result_disposition(false, true, 5, 5, 9, Some(10)),
            ScanDisposition::Restart
        );
        // 現在スコープが算出不能 (候補ゼロ等)。
        assert_eq!(
            scan_result_disposition(false, true, 5, 5, 9, None),
            ScanDisposition::Restart
        );
    }
}
