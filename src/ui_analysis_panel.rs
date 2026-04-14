//! フルスクリーン分析パネル。
//!
//! Z キーまたはホバーバーの 🔬 ボタンで表示される。
//! 画像の右側に固定幅パネルを表示し、カーソル位置の色情報・ヒストグラムを提供する。
//!
//! ## 操作
//! - マウスホバー: 色情報取得
//! - 右クリック（画像エリア）: 比較色を固定
//! - 修飾キー＋右クリック（Shift/Ctrl/Alt/Ctrl+Alt）: 色差強調フィルター ON
//! - 修飾キー＋左ドラッグ（Shift/Ctrl/Alt）: 等間隔ガイドライン＋同心円
//! - G キー: グレースケール表示切替
//! - M キー: モザイクグリッド表示切替

use eframe::egui;

use crate::app::App;

// ── 定数 ────────────────────────────────────────────────────────────────────

const HEADER_H: f32 = 36.0;
const SECTION_FONT: f32 = 12.0;
const HIST_H: f32 = 52.0;
/// カラーホイールの外半径の上限（ウルトラワイド等で巨大にならないよう）
const MAX_WHEEL_R: f32 = 72.0;
/// SV マップの一辺の上限
const MAX_SV_MAP: f32 = 280.0;

// ── ズームコンテキスト ────────────────────────────────────────────────────────

/// 画像座標 ↔ スクリーン座標変換に必要な値を一度だけ計算してまとめる。
struct ZoomCtx {
    /// 画像左上のスクリーン座標
    origin: egui::Pos2,
    /// 1 画像ピクセルあたりのスクリーンピクセル数
    scale: f32,
    /// 表示幅・高さ（スクリーン座標）
    disp_w: f32,
    disp_h: f32,
}

impl ZoomCtx {
    fn new(image_rect: egui::Rect, img_w: usize, img_h: usize, zoom: f32, pan: egui::Vec2) -> Self {
        let fit = (image_rect.width() / img_w as f32).min(image_rect.height() / img_h as f32);
        let scale = fit * zoom;
        let dw = img_w as f32 * scale;
        let dh = img_h as f32 * scale;
        Self {
            origin: egui::pos2(
                image_rect.center().x + pan.x - dw * 0.5,
                image_rect.center().y + pan.y - dh * 0.5,
            ),
            scale,
            disp_w: dw,
            disp_h: dh,
        }
    }

    /// スクリーン座標 → 画像ピクセル座標（整数、範囲チェック付き）
    fn screen_to_pixel(&self, pos: egui::Pos2, img_w: usize, img_h: usize) -> Option<(usize, usize)> {
        let rel = pos - self.origin;
        if rel.x < 0.0 || rel.y < 0.0 || rel.x >= self.disp_w || rel.y >= self.disp_h {
            return None;
        }
        Some((
            ((rel.x / self.scale) as usize).min(img_w - 1),
            ((rel.y / self.scale) as usize).min(img_h - 1),
        ))
    }

    /// スクリーン座標 → 画像座標（f32、範囲チェックなし。ガイドライン用）
    fn screen_to_image(&self, pos: egui::Pos2) -> egui::Pos2 {
        egui::pos2(
            (pos.x - self.origin.x) / self.scale,
            (pos.y - self.origin.y) / self.scale,
        )
    }

    /// 画像座標（f32）→ スクリーン座標
    fn image_to_screen(&self, p: egui::Pos2) -> egui::Pos2 {
        egui::pos2(self.origin.x + p.x * self.scale, self.origin.y + p.y * self.scale)
    }

    /// image_rect 内に表示されている画像ピクセル範囲
    fn visible_bounds(&self, image_rect: egui::Rect, img_w: usize, img_h: usize) -> (usize, usize, usize, usize) {
        let x0 = ((image_rect.min.x - self.origin.x) / self.scale).max(0.0) as usize;
        let y0 = ((image_rect.min.y - self.origin.y) / self.scale).max(0.0) as usize;
        let x1 = ((image_rect.max.x - self.origin.x) / self.scale).min(img_w as f32) as usize;
        let y1 = ((image_rect.max.y - self.origin.y) / self.scale).min(img_h as f32) as usize;
        (x0, y0, x1, y1)
    }

    /// 表示矩形
    fn display_rect(&self) -> egui::Rect {
        egui::Rect::from_min_size(self.origin, egui::vec2(self.disp_w, self.disp_h))
    }
}

// ── 色空間変換 ───────────────────────────────────────────────────────────────

/// RGB (0-255) → HSV (h: 0-360, s: 0-1, v: 0-1)
fn rgb_to_hsv(r: u8, g: u8, b: u8) -> (f32, f32, f32) {
    let rf = r as f32 / 255.0;
    let gf = g as f32 / 255.0;
    let bf = b as f32 / 255.0;
    let max = rf.max(gf).max(bf);
    let min = rf.min(gf).min(bf);
    let delta = max - min;
    let v = max;
    let s = if max > 0.0 { delta / max } else { 0.0 };
    let h = if delta < 1e-6 {
        0.0
    } else if (max - rf).abs() < 1e-6 {
        let mut h = (gf - bf) / delta % 6.0;
        if h < 0.0 { h += 6.0; }
        h * 60.0
    } else if (max - gf).abs() < 1e-6 {
        ((bf - rf) / delta + 2.0) * 60.0
    } else {
        ((rf - gf) / delta + 4.0) * 60.0
    };
    (h, s, v)
}

/// HSV (h: 0-360, s: 0-1, v: 0-1) → Color32
fn hsv_to_color32(h: f32, s: f32, v: f32) -> egui::Color32 {
    let h = h % 360.0;
    let c = v * s;
    let x = c * (1.0 - ((h / 60.0) % 2.0 - 1.0).abs());
    let m = v - c;
    let (r1, g1, b1) = match (h / 60.0) as u32 {
        0 => (c, x, 0.0),
        1 => (x, c, 0.0),
        2 => (0.0, c, x),
        3 => (0.0, x, c),
        4 => (x, 0.0, c),
        _ => (c, 0.0, x),
    };
    egui::Color32::from_rgb(
        ((r1 + m) * 255.0) as u8,
        ((g1 + m) * 255.0) as u8,
        ((b1 + m) * 255.0) as u8,
    )
}

/// sRGB (0-255) → CIE L*a*b*
fn rgb_to_lab(r: u8, g: u8, b: u8) -> (f32, f32, f32) {
    fn linearize(c: f32) -> f32 {
        if c <= 0.04045 { c / 12.92 } else { ((c + 0.055) / 1.055).powf(2.4) }
    }
    let rl = linearize(r as f32 / 255.0);
    let gl = linearize(g as f32 / 255.0);
    let bl = linearize(b as f32 / 255.0);
    let x = rl * 0.4124564 + gl * 0.3575761 + bl * 0.1804375;
    let y = rl * 0.2126729 + gl * 0.7151522 + bl * 0.0721750;
    let z = rl * 0.0193339 + gl * 0.1191920 + bl * 0.9503041;
    fn f(t: f32) -> f32 {
        if t > 0.008856 { t.cbrt() } else { 7.787 * t + 16.0 / 116.0 }
    }
    let l = 116.0 * f(y) - 16.0;
    let a = 500.0 * (f(x / 0.95047) - f(y));
    let bb = 200.0 * (f(y) - f(z / 1.08883));
    (l, a, bb)
}

fn delta_e(lab1: (f32, f32, f32), lab2: (f32, f32, f32)) -> f32 {
    let (dl, da, db) = (lab1.0 - lab2.0, lab1.1 - lab2.1, lab1.2 - lab2.2);
    (dl * dl + da * da + db * db).sqrt()
}

fn hue_diff(h1: f32, h2: f32) -> f32 {
    let d = (h1 - h2).abs() % 360.0;
    if d > 180.0 { 360.0 - d } else { d }
}

fn sample_pixel(pixels: &egui::ColorImage, x: usize, y: usize) -> [u8; 4] {
    let c = pixels.pixels[y * pixels.size[0] + x];
    [c.r(), c.g(), c.b(), c.a()]
}

// ── ヒストグラム計算 ─────────────────────────────────────────────────────────

/// 表示範囲の画像ピクセルから H/S/V ヒストグラムを計算する。
fn compute_hsv_histograms(
    pixels: &egui::ColorImage,
    zc: &ZoomCtx,
    image_rect: egui::Rect,
) -> ([u32; 360], [u32; 256], [u32; 256]) {
    let [img_w, _img_h] = pixels.size;
    let (px_x0, px_y0, px_x1, px_y1) = zc.visible_bounds(image_rect, pixels.size[0], pixels.size[1]);
    let area = (px_x1.saturating_sub(px_x0)) * (px_y1.saturating_sub(px_y0));
    let step = if area > 2_000_000 { 3 } else if area > 500_000 { 2 } else { 1 };

    let mut h_hist = [0u32; 360];
    let mut s_hist = [0u32; 256];
    let mut v_hist = [0u32; 256];
    for py in (px_y0..px_y1).step_by(step) {
        for px in (px_x0..px_x1).step_by(step) {
            let c = pixels.pixels[py * img_w + px];
            let (h, s, v) = rgb_to_hsv(c.r(), c.g(), c.b());
            h_hist[(h as usize).min(359)] += 1;
            s_hist[(s * 255.0) as usize] += 1;
            v_hist[(v * 255.0) as usize] += 1;
        }
    }
    (h_hist, s_hist, v_hist)
}

// ── カラーホイール描画 ────────────────────────────────────────────────────────

fn draw_color_picker_widget(
    painter: &egui::Painter,
    center: egui::Pos2,
    outer_r: f32,
    ring_w: f32,
    color: Option<(f32, f32, f32)>,
) {
    let inner_r = outer_r - ring_w;
    let n = 90usize;
    for i in 0..n {
        let a0 = (i as f32 / n as f32) * std::f32::consts::TAU - std::f32::consts::FRAC_PI_2;
        let a1 = ((i + 1) as f32 / n as f32) * std::f32::consts::TAU - std::f32::consts::FRAC_PI_2;
        let col = hsv_to_color32(i as f32 / n as f32 * 360.0, 1.0, 1.0);
        let po0 = center + egui::vec2(a0.cos(), a0.sin()) * outer_r;
        let po1 = center + egui::vec2(a1.cos(), a1.sin()) * outer_r;
        let pi0 = center + egui::vec2(a0.cos(), a0.sin()) * inner_r;
        let pi1 = center + egui::vec2(a1.cos(), a1.sin()) * inner_r;
        painter.add(egui::Shape::convex_polygon(vec![pi0, po0, po1, pi1], col, egui::Stroke::NONE));
    }

    // 内接正方形: SV グラデーション
    let half_sq = inner_r * std::f32::consts::FRAC_1_SQRT_2;
    let sq = egui::Rect::from_center_size(center, egui::vec2(half_sq * 2.0, half_sq * 2.0));
    let base_hue = color.map_or(0.0, |(h, _, _)| h);
    let grid = 20usize;
    let (cw, ch) = (sq.width() / grid as f32, sq.height() / grid as f32);
    for gy in 0..grid {
        for gx in 0..grid {
            let col = hsv_to_color32(base_hue, gx as f32 / (grid - 1) as f32, 1.0 - gy as f32 / (grid - 1) as f32);
            painter.rect_filled(
                egui::Rect::from_min_size(
                    egui::pos2(sq.min.x + gx as f32 * cw, sq.min.y + gy as f32 * ch),
                    egui::vec2(cw + 0.5, ch + 0.5),
                ), 0.0, col,
            );
        }
    }

    if let Some((h, s, v)) = color {
        // リング上マーカー
        let angle = h.to_radians() - std::f32::consts::FRAC_PI_2;
        let mr = (outer_r + inner_r) * 0.5;
        let mp = center + egui::vec2(angle.cos(), angle.sin()) * mr;
        let mkr = ring_w * 0.42;
        painter.circle_stroke(mp, mkr, egui::Stroke::new(2.0, egui::Color32::WHITE));
        painter.circle_stroke(mp, mkr, egui::Stroke::new(0.8, egui::Color32::BLACK));
        // SV 四角内マーカー
        let mx = sq.min.x + s * sq.width();
        let my = sq.min.y + (1.0 - v) * sq.height();
        let mc = if v > 0.5 && s < 0.5 { egui::Color32::BLACK } else { egui::Color32::WHITE };
        painter.circle_stroke(egui::pos2(mx, my), 4.5, egui::Stroke::new(2.0, egui::Color32::WHITE));
        painter.circle_stroke(egui::pos2(mx, my), 4.5, egui::Stroke::new(1.0, mc));
    }
}

// ── ヒストグラム描画 ─────────────────────────────────────────────────────────

fn draw_hue_histogram(painter: &egui::Painter, rect: egui::Rect, hist: &[u32; 360]) {
    painter.rect_filled(rect, 2.0, egui::Color32::from_gray(20));
    let max_val = hist.iter().copied().max().unwrap_or(1).max(1);
    let log_max = (max_val as f32 + 1.0).ln();
    let bar_w = rect.width() / 360.0;
    for (i, &v) in hist.iter().enumerate() {
        if v == 0 { continue; }
        let x = rect.min.x + i as f32 * bar_w;
        let bw = bar_w.max(1.0);
        let log_h = (v as f32 + 1.0).ln() / log_max * rect.height();
        painter.rect_filled(
            egui::Rect::from_min_max(egui::pos2(x, rect.max.y - log_h), egui::pos2(x + bw, rect.max.y)),
            0.0, hsv_to_color32(i as f32, 1.0, 0.4),
        );
        let lin_h = v as f32 / max_val as f32 * rect.height();
        painter.rect_filled(
            egui::Rect::from_min_max(egui::pos2(x, rect.max.y - lin_h), egui::pos2(x + bw, rect.max.y)),
            0.0, hsv_to_color32(i as f32, 1.0, 1.0),
        );
    }
    painter.rect_stroke(rect, 2.0, egui::Stroke::new(1.0, egui::Color32::from_gray(55)), egui::StrokeKind::Outside);
}

fn draw_sv_histogram(painter: &egui::Painter, rect: egui::Rect, hist: &[u32; 256], bright: egui::Color32, dark: egui::Color32) {
    painter.rect_filled(rect, 2.0, egui::Color32::from_gray(20));
    let max_val = hist.iter().copied().max().unwrap_or(1).max(1);
    let log_max = (max_val as f32 + 1.0).ln();
    let bar_w = rect.width() / 256.0;
    for (i, &v) in hist.iter().enumerate() {
        if v == 0 { continue; }
        let x = rect.min.x + i as f32 * bar_w;
        let bw = bar_w.max(1.0);
        let log_h = (v as f32 + 1.0).ln() / log_max * rect.height();
        painter.rect_filled(
            egui::Rect::from_min_max(egui::pos2(x, rect.max.y - log_h), egui::pos2(x + bw, rect.max.y)),
            0.0, dark,
        );
        let lin_h = v as f32 / max_val as f32 * rect.height();
        painter.rect_filled(
            egui::Rect::from_min_max(egui::pos2(x, rect.max.y - lin_h), egui::pos2(x + bw, rect.max.y)),
            0.0, bright,
        );
    }
    painter.rect_stroke(rect, 2.0, egui::Stroke::new(1.0, egui::Color32::from_gray(55)), egui::StrokeKind::Outside);
}

// ── SV マップ ────────────────────────────────────────────────────────────────

fn draw_sv_map(painter: &egui::Painter, rect: egui::Rect, pixels: &egui::ColorImage, zc: &ZoomCtx, image_rect: egui::Rect) {
    painter.rect_filled(rect, 2.0, egui::Color32::from_gray(12));
    let [img_w, _] = pixels.size;
    let (px_x0, px_y0, px_x1, px_y1) = zc.visible_bounds(image_rect, pixels.size[0], pixels.size[1]);
    let area = (px_x1.saturating_sub(px_x0)) * (px_y1.saturating_sub(px_y0));
    let step = if area > 1_000_000 { 3 } else { 2 };

    const BINS: usize = 64;
    let mut sv_count = [0u32; BINS * BINS];
    for py in (px_y0..px_y1).step_by(step) {
        for px in (px_x0..px_x1).step_by(step) {
            let c = pixels.pixels[py * img_w + px];
            let (_, s, v) = rgb_to_hsv(c.r(), c.g(), c.b());
            let si = ((s * (BINS - 1) as f32) as usize).min(BINS - 1);
            let vi = (BINS - 1) - ((v * (BINS - 1) as f32) as usize).min(BINS - 1);
            sv_count[vi * BINS + si] += 1;
        }
    }
    let max_cnt = sv_count.iter().copied().max().unwrap_or(1).max(1);
    let (cw, ch) = (rect.width() / BINS as f32, rect.height() / BINS as f32);
    for yi in 0..BINS {
        for xi in 0..BINS {
            let cnt = sv_count[yi * BINS + xi];
            if cnt == 0 { continue; }
            let ratio = (cnt as f32).ln() / (max_cnt as f32).ln();
            let col = hsv_to_color32((1.0 - ratio) * 120.0, 1.0, 1.0);
            let alpha = (ratio.powf(0.45) * 235.0 + 20.0) as u8;
            painter.rect_filled(
                egui::Rect::from_min_size(
                    egui::pos2(rect.min.x + xi as f32 * cw, rect.min.y + yi as f32 * ch),
                    egui::vec2(cw + 0.5, ch + 0.5),
                ), 0.0,
                egui::Color32::from_rgba_unmultiplied(col.r(), col.g(), col.b(), alpha),
            );
        }
    }
    painter.rect_stroke(rect, 2.0, egui::Stroke::new(1.0, egui::Color32::from_gray(55)), egui::StrokeKind::Outside);
}

// ── 画像エリアオーバーレイ ────────────────────────────────────────────────────

fn generate_filter_texture(ctx: &egui::Context, pixels: &egui::ColorImage, pick: [u8; 4], mag: f32) -> egui::TextureHandle {
    let mut out = pixels.pixels.clone();
    let (pr, pg, pb) = (pick[0] as f32, pick[1] as f32, pick[2] as f32);
    for c in out.iter_mut() {
        *c = egui::Color32::from_rgb(
            (pr + (c.r() as f32 - pr) * mag).clamp(0.0, 255.0) as u8,
            (pg + (c.g() as f32 - pg) * mag).clamp(0.0, 255.0) as u8,
            (pb + (c.b() as f32 - pb) * mag).clamp(0.0, 255.0) as u8,
        );
    }
    let ci = egui::ColorImage { size: pixels.size, pixels: out, source_size: egui::Vec2::new(pixels.size[0] as f32, pixels.size[1] as f32) };
    ctx.load_texture("analysis_filter_tex", ci, egui::TextureOptions::LINEAR)
}

fn generate_grayscale_texture(ctx: &egui::Context, pixels: &egui::ColorImage) -> egui::TextureHandle {
    let mut out = pixels.pixels.clone();
    for c in out.iter_mut() {
        let g = (rgb_to_lab(c.r(), c.g(), c.b()).0.clamp(0.0, 100.0) / 100.0 * 255.0) as u8;
        *c = egui::Color32::from_rgb(g, g, g);
    }
    let ci = egui::ColorImage { size: pixels.size, pixels: out, source_size: egui::Vec2::new(pixels.size[0] as f32, pixels.size[1] as f32) };
    ctx.load_texture("analysis_gray_tex", ci, egui::TextureOptions::LINEAR)
}

fn draw_mosaic_grid(painter: &egui::Painter, image_rect: egui::Rect, zc: &ZoomCtx) {
    let pitch = zc.disp_w.max(zc.disp_h) / 100.0;
    if pitch < 2.0 { return; }
    let stroke = egui::Stroke::new(1.0, egui::Color32::from_rgba_unmultiplied(255, 0, 0, 120));
    let dash   = egui::Stroke::new(0.5, egui::Color32::from_rgba_unmultiplied(255, 0, 0, 60));
    let clip = painter.with_clip_rect(image_rect);
    let (mut x, mut i) = (zc.origin.x, 0u32);
    while x <= zc.origin.x + zc.disp_w {
        clip.line_segment([egui::pos2(x, zc.origin.y), egui::pos2(x, zc.origin.y + zc.disp_h)],
            if i % 5 == 0 { stroke } else { dash });
        x += pitch; i += 1;
    }
    let (mut y, mut i) = (zc.origin.y, 0u32);
    while y <= zc.origin.y + zc.disp_h {
        clip.line_segment([egui::pos2(zc.origin.x, y), egui::pos2(zc.origin.x + zc.disp_w, y)],
            if i % 5 == 0 { stroke } else { dash });
        y += pitch; i += 1;
    }
}

pub(crate) fn draw_guide_lines(painter: &egui::Painter, start: egui::Pos2, end: egui::Pos2, image_rect: egui::Rect, color_idx: u8) {
    let col = match color_idx {
        0 => egui::Color32::from_rgba_unmultiplied(255, 60, 60, 200),
        1 => egui::Color32::from_rgba_unmultiplied(20, 20, 20, 200),
        _ => egui::Color32::from_rgba_unmultiplied(255, 255, 255, 200),
    };
    let col_semi = egui::Color32::from_rgba_unmultiplied(col.r(), col.g(), col.b(), 100);
    let drag_stroke  = egui::Stroke::new(3.0, col);
    let unit_stroke  = egui::Stroke::new(2.0, col);
    let half_stroke  = egui::Stroke::new(1.0, col_semi);
    let circle_stroke = egui::Stroke::new(2.0, col);
    let circle_thin   = egui::Stroke::new(1.0, col_semi);

    let (dx, dy) = (end.x - start.x, end.y - start.y);
    let dist = (dx * dx + dy * dy).sqrt();
    if dist < 2.0 { return; }
    let (dir_x, dir_y) = (dx / dist, dy / dist);
    let (perp_x, perp_y) = (-dir_y, dir_x);
    let diag = (image_rect.width().powi(2) + image_rect.height().powi(2)).sqrt();
    let near = image_rect.expand(50.0);

    // 0.5 単位刻みの垂直線
    for hn in -40i32..=40 {
        let off = dist * hn as f32 * 0.5;
        let (cx, cy) = (start.x + dir_x * off, start.y + dir_y * off);
        let p0 = egui::pos2(cx - perp_x * diag, cy - perp_y * diag);
        let p1 = egui::pos2(cx + perp_x * diag, cy + perp_y * diag);
        if !near.contains(p0) && !near.contains(p1) && !near.contains(egui::pos2(cx, cy)) { continue; }
        painter.line_segment([p0, p1], if hn % 2 == 0 { unit_stroke } else { half_stroke });
    }

    // 同心円
    for n in 1..=20i32 {
        let r = dist * n as f32;
        if r > diag * 1.2 { break; }
        let segs = 72usize;
        let mut prev: Option<egui::Pos2> = None;
        for i in 0..=segs {
            let a = i as f32 / segs as f32 * std::f32::consts::TAU;
            let p = egui::pos2(start.x + a.cos() * r, start.y + a.sin() * r);
            if let Some(pv) = prev {
                if image_rect.contains(p) || image_rect.contains(pv) {
                    painter.line_segment([pv, p], if n == 1 { circle_stroke } else { circle_thin });
                }
            }
            prev = Some(p);
        }
    }

    painter.line_segment([start, end], drag_stroke);
    painter.circle_filled(start, 4.0, col);
    painter.circle_filled(end, 3.0, col);
    painter.text(end + egui::vec2(8.0, -8.0), egui::Align2::LEFT_BOTTOM,
        format!("{:.0}px", dist), egui::FontId::proportional(12.0), col);
}

// ── 3色空間表示ヘルパー ─────────────────────────────────────────────────────

/// 色空間セクション1行分のデータ
struct ColorRow {
    label: &'static str,
    hover: String,
    pinned: String,
    diff: String,
}

/// ホバー色・固定色から RGB / HSV / Lab の各行データを生成する。
fn build_color_rows(hc: Option<[u8; 4]>, pc: Option<[u8; 4]>) -> Vec<(&'static str, Vec<ColorRow>)> {
    let dash = "---".to_string();
    let mut sections = Vec::new();

    // RGB
    let (rh, gh, bh) = hc.map_or(("---".into(), "---".into(), "---".into()),
        |[r,g,b,_]| (format!("{r:3}"), format!("{g:3}"), format!("{b:3}")));
    let (rp, gp, bp) = pc.map_or(("---".into(), "---".into(), "---".into()),
        |[r,g,b,_]| (format!("{r:3}"), format!("{g:3}"), format!("{b:3}")));
    let rgb_diff = hc.zip(pc).map(|([r1,g1,b1,_],[r2,g2,b2,_])| [
        format!("{:+}", r1 as i32 - r2 as i32),
        format!("{:+}", g1 as i32 - g2 as i32),
        format!("{:+}", b1 as i32 - b2 as i32),
    ]);
    sections.push(("RGB", vec![
        ColorRow { label: "R:", hover: rh, pinned: rp, diff: rgb_diff.as_ref().map_or(dash.clone(), |d| d[0].clone()) },
        ColorRow { label: "G:", hover: gh, pinned: gp, diff: rgb_diff.as_ref().map_or(dash.clone(), |d| d[1].clone()) },
        ColorRow { label: "B:", hover: bh, pinned: bp, diff: rgb_diff.as_ref().map_or(dash.clone(), |d| d[2].clone()) },
    ]));

    // HSV
    let fmt_hsv = |c: Option<[u8;4]>| -> (String,String,String) {
        c.map_or((dash.clone(), dash.clone(), dash.clone()), |[r,g,b,_]| {
            let (h,s,v) = rgb_to_hsv(r,g,b);
            (format!("{:3}°", h as i32), format!("{:.0}%", s*100.0), format!("{:.0}%", v*100.0))
        })
    };
    let (hh,sh,vh) = fmt_hsv(hc);
    let (hp,sp,vp) = fmt_hsv(pc);
    let hsv_diff = hc.zip(pc).map(|([r1,g1,b1,_],[r2,g2,b2,_])| {
        let (h1,s1,v1) = rgb_to_hsv(r1,g1,b1);
        let (h2,s2,v2) = rgb_to_hsv(r2,g2,b2);
        [
            format!("{:+.0}°", hue_diff(h1,h2) * if h1 >= h2 { 1.0 } else { -1.0 }),
            format!("{:+.0}%", (s1-s2)*100.0),
            format!("{:+.0}%", (v1-v2)*100.0),
        ]
    });
    sections.push(("HSV", vec![
        ColorRow { label: "H:", hover: hh, pinned: hp, diff: hsv_diff.as_ref().map_or(dash.clone(), |d| d[0].clone()) },
        ColorRow { label: "S:", hover: sh, pinned: sp, diff: hsv_diff.as_ref().map_or(dash.clone(), |d| d[1].clone()) },
        ColorRow { label: "V:", hover: vh, pinned: vp, diff: hsv_diff.as_ref().map_or(dash.clone(), |d| d[2].clone()) },
    ]));

    // Lab
    let fmt_lab = |c: Option<[u8;4]>| -> (String,String,String) {
        c.map_or((dash.clone(), dash.clone(), dash.clone()), |[r,g,b,_]| {
            let (l,a,bb) = rgb_to_lab(r,g,b);
            (format!("{:.1}", l), format!("{:.1}", a), format!("{:.1}", bb))
        })
    };
    let (lh,ah,bh2) = fmt_lab(hc);
    let (lp,ap,bp2) = fmt_lab(pc);
    let lab_diff = hc.zip(pc).map(|([r1,g1,b1,_],[r2,g2,b2,_])| {
        let (l1,a1,b1_) = rgb_to_lab(r1,g1,b1);
        let (l2,a2,b2_) = rgb_to_lab(r2,g2,b2);
        [format!("{:+.1}", l1-l2), format!("{:+.1}", a1-a2), format!("{:+.1}", b1_-b2_)]
    });
    sections.push(("L*a*b*", vec![
        ColorRow { label: "L:", hover: lh, pinned: lp, diff: lab_diff.as_ref().map_or(dash.clone(), |d| d[0].clone()) },
        ColorRow { label: "a:", hover: ah, pinned: ap, diff: lab_diff.as_ref().map_or(dash.clone(), |d| d[1].clone()) },
        ColorRow { label: "b:", hover: bh2, pinned: bp2, diff: lab_diff.as_ref().map_or(dash.clone(), |d| d[2].clone()) },
    ]));

    sections
}

/// 3色空間セクションをまとめて描画し、次の y を返す。
fn draw_color_sections(painter: &egui::Painter, x0: f32, mut y: f32, pw: f32, hc: Option<[u8; 4]>, pc: Option<[u8; 4]>) -> f32 {
    let col3 = pw / 3.0;
    let line_h = 13.0;
    let font = egui::FontId::monospace(9.5);
    let text_col = egui::Color32::from_gray(200);
    let dash_col = egui::Color32::from_gray(100);

    let sections = build_color_rows(hc, pc);
    for (si, (title, rows)) in sections.iter().enumerate() {
        if si > 0 {
            thin_separator(painter, egui::pos2(x0, y), pw);
            y += 4.0;
        }
        small_label(painter, egui::pos2(x0, y), title);
        y += 11.0;
        for (i, row) in rows.iter().enumerate() {
            let ry = y + i as f32 * line_h;
            painter.text(egui::pos2(x0, ry), egui::Align2::LEFT_TOP,
                format!("{}{}", row.label, row.hover), font.clone(), text_col);
            painter.text(egui::pos2(x0 + col3 * 2.0, ry), egui::Align2::LEFT_TOP,
                format!("{}{}", row.label, row.pinned), font.clone(), text_col);
            let dc = if row.diff == "---" { dash_col } else { diff_color(&row.diff) };
            painter.text(egui::pos2(x0 + col3, ry), egui::Align2::LEFT_TOP, &row.diff, font.clone(), dc);
        }
        y += rows.len() as f32 * line_h + 2.0;
    }
    y
}

// ── メインパネル ─────────────────────────────────────────────────────────────

impl App {
    pub(crate) fn draw_analysis_panel(
        &mut self,
        ui: &mut egui::Ui,
        ctx: &egui::Context,
        full_rect: egui::Rect,
        image_rect: egui::Rect,
        pixels: Option<&egui::ColorImage>,
    ) -> bool {
        let mods = ctx.input(|i| i.modifiers);
        let is_drag_mod = mods.shift || mods.ctrl || mods.alt;
        let (iw, ih) = pixels.map_or((1usize, 1usize), |p| (p.size[0], p.size[1]));
        let zc = ZoomCtx::new(image_rect, iw, ih, self.analysis_zoom, self.analysis_pan);

        // ── 入力処理 ──
        let primary_pressed = ctx.input(|i| i.pointer.primary_pressed());
        let primary_down = ctx.input(|i| i.pointer.primary_down());
        let primary_released = ctx.input(|i| i.pointer.primary_released());

        if primary_pressed && is_drag_mod {
            if let Some(pos) = ctx.input(|i| i.pointer.press_origin()) {
                if image_rect.contains(pos) {
                    let cidx = if mods.shift { 0 } else if mods.ctrl { 1 } else { 2 };
                    let img_pos = zc.screen_to_image(pos);
                    self.analysis_guide_drag = Some((img_pos, img_pos, cidx));
                    self.analysis_mosaic_grid = false;
                }
            }
        } else if primary_down && is_drag_mod && self.analysis_guide_drag.is_some() {
            if let Some(pos) = ctx.input(|i| i.pointer.hover_pos()) {
                if let Some((start, _, cidx)) = self.analysis_guide_drag {
                    self.analysis_guide_drag = Some((start, zc.screen_to_image(pos), cidx));
                }
            }
        } else if primary_pressed && !is_drag_mod {
            if let Some(pos) = ctx.input(|i| i.pointer.press_origin()) {
                if image_rect.contains(pos) {
                    self.analysis_pan_drag_start = Some((pos, self.analysis_pan));
                }
            }
        } else if primary_down && !is_drag_mod {
            if let Some((start_pos, start_pan)) = self.analysis_pan_drag_start {
                if let Some(pos) = ctx.input(|i| i.pointer.hover_pos()) {
                    self.analysis_pan = start_pan + (pos - start_pos);
                }
            }
        }
        if primary_released { self.analysis_pan_drag_start = None; }

        if ctx.input(|i| i.pointer.secondary_pressed()) {
            if let Some(pos) = ctx.input(|i| i.pointer.interact_pos()) {
                if image_rect.contains(pos) {
                    let mag = if mods.ctrl && mods.alt { 20u8 }
                        else if mods.alt { 10 }
                        else if mods.ctrl { 5 }
                        else if mods.shift { 2 }
                        else { 0 };
                    if mag > 0 {
                        if let Some(c) = self.analysis_hover_color { self.analysis_pinned_color = Some(c); }
                        self.analysis_filter_mag = mag;
                        self.analysis_overlay_cache = None;
                    } else {
                        self.analysis_guide_drag = None;
                        self.analysis_filter_mag = 0;
                        self.analysis_overlay_cache = None;
                        if let Some(c) = self.analysis_hover_color { self.analysis_pinned_color = Some(c); }
                    }
                }
            }
        }

        // ── オーバーレイ描画（キャッシュ付き）──
        let zoom = self.analysis_zoom;
        let pan = self.analysis_pan;
        let fs_idx = self.fullscreen_idx.unwrap_or(0);
        if let Some(pix) = pixels {
            let overlay_mode = if self.analysis_filter_mag > 0 { self.analysis_filter_mag } else if self.analysis_grayscale { 255 } else { 0 };
            let overlay_pick = self.analysis_pinned_color;
            if overlay_mode > 0 {
                let cache_valid = self.analysis_overlay_cache.as_ref().map_or(false, |c|
                    c.1 == overlay_mode && c.2 == overlay_pick && c.3 == zoom && c.4 == pan && c.5 == fs_idx);
                if !cache_valid {
                    let tex = if overlay_mode == 255 { generate_grayscale_texture(ctx, pix) }
                              else { generate_filter_texture(ctx, pix, overlay_pick.unwrap_or([0,0,0,255]), overlay_mode as f32) };
                    self.analysis_overlay_cache = Some((tex, overlay_mode, overlay_pick, zoom, pan, fs_idx));
                }
                if let Some((ref tex, ..)) = self.analysis_overlay_cache {
                    ui.painter().with_clip_rect(image_rect).image(
                        tex.id(), zc.display_rect(),
                        egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
                        egui::Color32::WHITE,
                    );
                }
            } else {
                self.analysis_overlay_cache = None;
            }
            if self.analysis_mosaic_grid { draw_mosaic_grid(ui.painter(), image_rect, &zc); }
        }
        if let Some((img_start, img_end, cidx)) = self.analysis_guide_drag {
            draw_guide_lines(ui.painter(), zc.image_to_screen(img_start), zc.image_to_screen(img_end), image_rect, cidx);
        }

        // ── パネル UI ──
        let panel_rect = egui::Rect::from_min_max(egui::pos2(image_rect.max.x, full_rect.min.y), full_rect.max);
        ui.painter().rect_filled(panel_rect, 0.0, egui::Color32::from_rgba_unmultiplied(18, 18, 22, 245));
        ui.painter().line_segment([panel_rect.min, egui::pos2(panel_rect.min.x, panel_rect.max.y)],
            egui::Stroke::new(1.0, egui::Color32::from_gray(55)));

        // ヘッダー
        let header_rect = egui::Rect::from_min_size(panel_rect.min, egui::vec2(panel_rect.width(), HEADER_H));
        ui.painter().text(egui::pos2(panel_rect.min.x + 12.0, header_rect.center().y),
            egui::Align2::LEFT_CENTER, "画像分析", egui::FontId::proportional(13.0), egui::Color32::from_gray(200));

        // ステータスインジケーター
        let hints = format!("{}{}{}",
            if self.analysis_grayscale { " [G]" } else { "" },
            if self.analysis_mosaic_grid { " [M]" } else { "" },
            if self.analysis_filter_mag > 0 { format!(" [{}x]", self.analysis_filter_mag) } else { String::new() });
        if !hints.is_empty() {
            ui.painter().text(egui::pos2(panel_rect.max.x - 38.0, header_rect.center().y),
                egui::Align2::RIGHT_CENTER, &hints, egui::FontId::proportional(9.5), egui::Color32::from_rgb(140, 200, 140));
        }

        // × ボタン
        let cb_size = 22.0;
        let cb_rect = egui::Rect::from_center_size(egui::pos2(panel_rect.max.x - 16.0, header_rect.center().y), egui::vec2(cb_size, cb_size));
        let close_resp = ui.interact(cb_rect, egui::Id::new("analysis_close_btn"), egui::Sense::click());
        ui.painter().rect_filled(cb_rect, 4.0,
            if close_resp.hovered() { egui::Color32::from_rgba_unmultiplied(200, 60, 60, 220) }
            else { egui::Color32::from_rgba_unmultiplied(60, 60, 65, 200) });
        let xr = cb_size * 0.22;
        let xc = cb_rect.center();
        let xs = egui::Stroke::new(1.8, egui::Color32::WHITE);
        ui.painter().line_segment([egui::pos2(xc.x - xr, xc.y - xr), egui::pos2(xc.x + xr, xc.y + xr)], xs);
        ui.painter().line_segment([egui::pos2(xc.x + xr, xc.y - xr), egui::pos2(xc.x - xr, xc.y + xr)], xs);
        ui.painter().line_segment(
            [egui::pos2(panel_rect.min.x, header_rect.max.y), egui::pos2(panel_rect.max.x, header_rect.max.y)],
            egui::Stroke::new(1.0, egui::Color32::from_gray(45)));
        let close_resp = close_resp.on_hover_text("閉じる [Z]");
        if close_resp.clicked() { return true; }

        // ── マウス色取得 ──
        let hover_pixel: Option<[u8; 4]> = ctx.input(|i| i.pointer.hover_pos()).and_then(|pos| {
            if !image_rect.contains(pos) { return None; }
            let pix = pixels?;
            let (px, py) = zc.screen_to_pixel(pos, iw, ih)?;
            Some(sample_pixel(pix, px, py))
        });
        if let Some(c) = hover_pixel { self.analysis_hover_color = Some(c); }

        let x0 = panel_rect.min.x + 10.0;
        let pw = panel_rect.width() - 20.0;
        let mut y = header_rect.max.y + 5.0;

        // ── カラーピッカー ──
        section_label(ui.painter(), egui::pos2(x0, y), "カラー情報"); y += 14.0;
        let hover_hsv = self.analysis_hover_color.map(|[r,g,b,_]| rgb_to_hsv(r,g,b));
        let pinned_hsv = self.analysis_pinned_color.map(|[r,g,b,_]| rgb_to_hsv(r,g,b));
        let half = pw / 2.0;
        let outer_r = (half * 0.46).min(MAX_WHEEL_R);
        let ring_w = outer_r * 0.26;
        let wheel_cy = y + outer_r + 2.0;
        let (c1x, c2x) = (x0 + half * 0.50, x0 + half * 0.50 + half);
        draw_color_picker_widget(ui.painter(), egui::pos2(c1x, wheel_cy), outer_r, ring_w, hover_hsv);
        draw_color_picker_widget(ui.painter(), egui::pos2(c2x, wheel_cy), outer_r, ring_w, pinned_hsv);
        ui.painter().text(egui::pos2(c1x, wheel_cy + outer_r + 4.0), egui::Align2::CENTER_TOP,
            "ホバー", egui::FontId::proportional(9.5), egui::Color32::from_gray(140));
        ui.painter().text(egui::pos2(c2x, wheel_cy + outer_r + 4.0), egui::Align2::CENTER_TOP,
            "固定(右クリック)", egui::FontId::proportional(9.0), egui::Color32::from_gray(140));
        y = wheel_cy + outer_r + 16.0;

        // 色スウォッチ
        let sw_h = 6.0;
        if let Some([r,g,b,_]) = self.analysis_hover_color {
            ui.painter().rect_filled(egui::Rect::from_min_size(egui::pos2(x0, y), egui::vec2(half - 3.0, sw_h)),
                2.0, egui::Color32::from_rgb(r, g, b));
        }
        if let Some([r,g,b,_]) = self.analysis_pinned_color {
            ui.painter().rect_filled(egui::Rect::from_min_size(egui::pos2(x0 + half, y), egui::vec2(half - 3.0, sw_h)),
                2.0, egui::Color32::from_rgb(r, g, b));
        }
        y += sw_h + 3.0;

        // 色数値（3色空間統合ヘルパー）
        let hc = self.analysis_hover_color;
        let pc = self.analysis_pinned_color;
        y = draw_color_sections(ui.painter(), x0, y, pw, hc, pc);

        // ΔE
        if let (Some([r1,g1,b1,_]), Some([r2,g2,b2,_])) = (hc, pc) {
            let de = delta_e(rgb_to_lab(r1,g1,b1), rgb_to_lab(r2,g2,b2));
            ui.painter().text(egui::pos2(x0 + pw * 0.5, y), egui::Align2::CENTER_TOP,
                format!("ΔE = {:.2}", de), egui::FontId::proportional(11.5), egui::Color32::from_rgb(220, 200, 100));
            y += 16.0;
        }

        // ── ヒストグラム ──
        if let Some(pix) = pixels {
            thin_separator(ui.painter(), egui::pos2(x0, y), pw); y += 4.0;
            section_label(ui.painter(), egui::pos2(x0, y), "ヒストグラム（表示範囲）"); y += 13.0;

            // キャッシュ: zoom/pan/image が変わらなければ再計算しない
            let hist_valid = self.analysis_hist_cache.as_ref().map_or(false, |c| c.0 == zoom && c.1 == pan && c.2 == fs_idx);
            if !hist_valid {
                let (h, s, v) = compute_hsv_histograms(pix, &zc, image_rect);
                self.analysis_hist_cache = Some((zoom, pan, fs_idx, h, s, v));
            }
            let (_, _, _, h_hist, s_hist, v_hist) = self.analysis_hist_cache.as_ref().unwrap();

            small_label(ui.painter(), egui::pos2(x0, y), "色相 (H)"); y += 11.0;
            draw_hue_histogram(ui.painter(), egui::Rect::from_min_size(egui::pos2(x0, y), egui::vec2(pw, HIST_H)), h_hist);
            y += HIST_H + 4.0;
            small_label(ui.painter(), egui::pos2(x0, y), "彩度 (S)"); y += 11.0;
            draw_sv_histogram(ui.painter(), egui::Rect::from_min_size(egui::pos2(x0, y), egui::vec2(pw, HIST_H)), s_hist,
                egui::Color32::from_rgb(60, 140, 255), egui::Color32::from_rgb(20, 60, 140));
            y += HIST_H + 4.0;
            small_label(ui.painter(), egui::pos2(x0, y), "明度 (V)"); y += 11.0;
            draw_sv_histogram(ui.painter(), egui::Rect::from_min_size(egui::pos2(x0, y), egui::vec2(pw, HIST_H)), v_hist,
                egui::Color32::from_gray(220), egui::Color32::from_gray(80));
            y += HIST_H + 5.0;

            // SV マップ
            thin_separator(ui.painter(), egui::pos2(x0, y), pw); y += 4.0;
            section_label(ui.painter(), egui::pos2(x0, y), "彩度・明度マップ");
            ui.painter().text(egui::pos2(x0 + pw, y), egui::Align2::RIGHT_TOP,
                "→彩度  ↑明度", egui::FontId::proportional(9.0), egui::Color32::from_gray(100));
            y += 13.0;
            let map_side = pw.min(MAX_SV_MAP);
            let map_x = x0 + (pw - map_side) * 0.5;
            draw_sv_map(ui.painter(), egui::Rect::from_min_size(egui::pos2(map_x, y), egui::vec2(map_side, map_side)),
                pix, &zc, image_rect);
            y += map_side + 8.0;

            thin_separator(ui.painter(), egui::pos2(x0, y), pw); y += 5.0;
            draw_shortcut_help(ui.painter(), egui::pos2(x0, y), pw);
        }

        false
    }
}

// ── 小ヘルパー ──────────────────────────────────────────────────────────────

fn diff_color(s: &str) -> egui::Color32 {
    let t = s.trim();
    if t.starts_with('+') && t != "+0" && t != "+0°" && t != "+0%" && t != "+0.0" && t != "+0.0°" && t != "+0.0%" {
        egui::Color32::from_rgb(120, 210, 120)
    } else if t.starts_with('-') {
        egui::Color32::from_rgb(210, 120, 120)
    } else {
        egui::Color32::from_gray(140)
    }
}

fn section_label(painter: &egui::Painter, pos: egui::Pos2, label: &str) {
    painter.text(pos, egui::Align2::LEFT_TOP, label,
        egui::FontId::proportional(SECTION_FONT), egui::Color32::from_rgb(140, 180, 220));
}

fn small_label(painter: &egui::Painter, pos: egui::Pos2, label: &str) {
    painter.text(pos, egui::Align2::LEFT_TOP, label,
        egui::FontId::proportional(SECTION_FONT - 1.0), egui::Color32::from_gray(150));
}

fn thin_separator(painter: &egui::Painter, pos: egui::Pos2, width: f32) {
    painter.line_segment([pos, egui::pos2(pos.x + width, pos.y)],
        egui::Stroke::new(1.0, egui::Color32::from_gray(40)));
}

fn draw_shortcut_help(painter: &egui::Painter, origin: egui::Pos2, _pw: f32) {
    let key_col = egui::Color32::from_rgb(180, 210, 180);
    let desc_col = egui::Color32::from_gray(140);
    let font_key  = egui::FontId::monospace(9.5);
    let font_desc = egui::FontId::proportional(9.5);
    let lh = 13.5_f32;
    let items: &[(&str, &str)] = &[
        ("G",               "グレースケール 切替"),
        ("M",               "モザイクグリッド 切替"),
        ("Shift + ドラッグ", "赤 ガイドライン"),
        ("Ctrl  + ドラッグ", "黒 ガイドライン"),
        ("Alt   + ドラッグ", "白 ガイドライン"),
        ("右クリック",        "比較色を固定"),
        ("Shift + 右クリック","色差強調 ×2"),
        ("Ctrl  + 右クリック","色差強調 ×5"),
        ("Alt   + 右クリック","色差強調 ×10"),
        ("Ctrl+Alt+ 右クリック","色差強調 ×20"),
        ("Z",               "分析モード 切替"),
    ];
    let key_w = 135.0_f32;
    for (i, (key, desc)) in items.iter().enumerate() {
        let y = origin.y + i as f32 * lh;
        painter.text(egui::pos2(origin.x, y), egui::Align2::LEFT_TOP, *key, font_key.clone(), key_col);
        painter.text(egui::pos2(origin.x + key_w, y), egui::Align2::LEFT_TOP, *desc, font_desc.clone(), desc_col);
    }
}
