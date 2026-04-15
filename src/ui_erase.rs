//! 消しゴム (Erase) モード: フルスクリーン画像の任意領域をマスクし、
//! MI-GAN で補完 (inpaint) する。
//!
//! ツール: 囲み (Lasso), 縦線, 横線, 筆 (Brush)
//! モード: 描画 / 消去 の切り替え
//! マスクは SQLite (mask_db) に永続化される。

use eframe::egui;
use std::sync::Arc;

use crate::app::{App, EraseTool};
use crate::fs_animation::FsCacheEntry;

/// MI-GAN の固定入力サイズ。
const MIGAN_SIZE: usize = 512;

/// ツールパネルの幅。
const PANEL_W: f32 = 190.0;
/// ツールパネルの左上マージン。
const PANEL_MARGIN_X: f32 = 16.0;
const PANEL_MARGIN_Y: f32 = 60.0;

impl App {
    // ── モード開始/終了 ─────────────────────────────────────────────

    /// 消しゴムモードに入る。DB にマスクがあればロードする。
    pub(crate) fn enter_erase_mode(&mut self, fs_idx: usize) {
        // 元画像を取得: erase_base_cache (inpaint前の元画像) を優先、なければキャッシュから
        let pixels = if let Some(base) = self.erase_base_cache.get(&fs_idx) {
            Arc::clone(base)
        } else {
            let from_cache = self.ai_upscale_cache.get(&fs_idx)
                .or_else(|| self.fs_cache.get(&fs_idx))
                .and_then(|entry| match entry {
                    FsCacheEntry::Static { pixels, .. } => Some(Arc::clone(pixels)),
                    _ => None,
                });
            match from_cache {
                Some(p) => {
                    // 初回: 元画像を base_cache に保存
                    self.erase_base_cache.insert(fs_idx, Arc::clone(&p));
                    p
                }
                None => return,
            }
        };
        let [w, h] = pixels.size;
        self.erase_mode = true;
        self.erase_mask_size = [w, h];
        self.erase_mask_texture = None;
        self.erase_last_paint_pos = None;


        self.erase_lasso_points.clear();
        self.erase_line_start = None;
        self.erase_line_end = None;
        self.erase_paint_mode = true;

        // デフォルトブラシ半径: 長辺の 1/100
        if self.erase_brush_radius <= 0.0 {
            self.erase_brush_radius = (w.max(h) as f32 / 100.0).max(2.0);
        }

        // DB からマスクをロード
        let loaded_mask = self.page_path_key(fs_idx)
            .and_then(|key| self.mask_db.as_ref()?.get(&key, w, h));

        self.erase_mask = Some(loaded_mask.unwrap_or_else(|| vec![false; w * h]));
        crate::logger::log(format!("erase: enter mode, image={w}x{h}"));
    }

    /// 消しゴムモードをリセットする。
    pub(crate) fn reset_erase_mode(&mut self) {
        self.erase_mode = false;
        self.erase_mask = None;
        self.erase_mask_size = [0, 0];
        self.erase_mask_texture = None;
        self.erase_last_paint_pos = None;


        self.erase_lasso_points.clear();
        self.erase_line_start = None;
        self.erase_line_end = None;
    }

    // ── 座標変換 ──────────────────────────────────────────────────

    /// 画像レイアウト情報 (total_scale, img_rect) を計算する。
    fn erase_image_layout(
        &self,
        full_rect: egui::Rect,
        zoom_pan: Option<(f32, egui::Vec2)>,
    ) -> Option<(f32, egui::Rect)> {
        let [iw, ih] = self.erase_mask_size;
        if iw == 0 || ih == 0 { return None; }
        let display_size = egui::vec2(iw as f32, ih as f32);
        let fit_scale = (full_rect.width() / display_size.x)
            .min(full_rect.height() / display_size.y);
        let (total_scale, center) = match zoom_pan {
            Some((zoom, pan)) => (fit_scale * zoom, full_rect.center() + pan),
            None => (fit_scale, full_rect.center()),
        };
        Some((total_scale, egui::Rect::from_center_size(center, display_size * total_scale)))
    }

    /// スクリーン座標を画像ピクセル座標 (f32) に変換する。
    fn screen_to_image_f32(
        &self,
        screen_pos: egui::Pos2,
        full_rect: egui::Rect,
        zoom_pan: Option<(f32, egui::Vec2)>,
    ) -> Option<(f32, f32)> {
        let (total_scale, img_rect) = self.erase_image_layout(full_rect, zoom_pan)?;
        let [iw, ih] = self.erase_mask_size;
        let nx = (screen_pos.x - img_rect.min.x) / total_scale;
        let ny = (screen_pos.y - img_rect.min.y) / total_scale;
        if nx >= 0.0 && ny >= 0.0 && nx < iw as f32 && ny < ih as f32 {
            Some((nx, ny))
        } else {
            None
        }
    }

    /// 画像ピクセル座標をスクリーン座標に変換する。
    fn image_to_screen(
        &self,
        img_x: f32,
        img_y: f32,
        full_rect: egui::Rect,
        zoom_pan: Option<(f32, egui::Vec2)>,
    ) -> egui::Pos2 {
        let (total_scale, img_rect) = self.erase_image_layout(full_rect, zoom_pan)
            .unwrap_or((1.0, full_rect));
        egui::pos2(
            img_rect.min.x + img_x * total_scale,
            img_rect.min.y + img_y * total_scale,
        )
    }

    // ── マスク操作 ────────────────────────────────────────────────

    /// 円形ブラシで from → to を線で塗る。paint=true で描画、false で消去。
    fn paint_brush_line(&mut self, from: (f32, f32), to: (f32, f32), paint: bool) {
        let radius = self.erase_brush_radius;
        let [w, h] = self.erase_mask_size;
        let mask = match self.erase_mask.as_mut() {
            Some(m) => m,
            None => return,
        };

        let dx = to.0 - from.0;
        let dy = to.1 - from.1;
        let dist = (dx * dx + dy * dy).sqrt();
        let steps = (dist / (radius * 0.5)).ceil().max(1.0) as usize;

        for step in 0..=steps {
            let t = step as f32 / steps as f32;
            let cx = from.0 + dx * t;
            let cy = from.1 + dy * t;

            let r = radius;
            let x0 = (cx - r).floor().max(0.0) as usize;
            let y0 = (cy - r).floor().max(0.0) as usize;
            let x1 = (cx + r).ceil().min(w as f32) as usize;
            let y1 = (cy + r).ceil().min(h as f32) as usize;
            let r_sq = r * r;

            for py in y0..y1 {
                for px in x0..x1 {
                    let ddx = px as f32 + 0.5 - cx;
                    let ddy = py as f32 + 0.5 - cy;
                    if ddx * ddx + ddy * ddy <= r_sq {
                        mask[py * w + px] = paint;
                    }
                }
            }
        }
        self.erase_mask_texture = None;
    }

    /// 矩形領域を塗る。
    fn paint_rect(&mut self, x0: usize, y0: usize, x1: usize, y1: usize, paint: bool) {
        let [w, _h] = self.erase_mask_size;
        let mask = match self.erase_mask.as_mut() {
            Some(m) => m,
            None => return,
        };
        for py in y0..y1 {
            for px in x0..x1 {
                mask[py * w + px] = paint;
            }
        }
        self.erase_mask_texture = None;
    }

    /// 多角形の内部を scan-line fill で塗る。
    fn paint_polygon(&mut self, points: &[(f32, f32)], paint: bool) {
        if points.len() < 3 { return; }
        let [w, h] = self.erase_mask_size;
        let mask = match self.erase_mask.as_mut() {
            Some(m) => m,
            None => return,
        };

        // バウンディングボックス
        let min_y = points.iter().map(|p| p.1).fold(f32::MAX, f32::min).max(0.0) as usize;
        let max_y = points.iter().map(|p| p.1).fold(f32::MIN, f32::max).min(h as f32) as usize;

        let n = points.len();
        let mut intersections = Vec::new();
        for y in min_y..max_y {
            let scan_y = y as f32 + 0.5;
            intersections.clear();
            for i in 0..n {
                let (x0, y0) = points[i];
                let (x1, y1) = points[(i + 1) % n];
                if (y0 <= scan_y && y1 > scan_y) || (y1 <= scan_y && y0 > scan_y) {
                    let t = (scan_y - y0) / (y1 - y0);
                    intersections.push(x0 + t * (x1 - x0));
                }
            }

            intersections.sort_by(|a, b| a.partial_cmp(b).unwrap());

            for pair in intersections.chunks(2) {
                if pair.len() == 2 {
                    let px0 = (pair[0].max(0.0) as usize).min(w);
                    let px1 = (pair[1].max(0.0).ceil() as usize).min(w);
                    for px in px0..px1 {
                        mask[y * w + px] = paint;
                    }
                }
            }
        }
        self.erase_mask_texture = None;
    }

    // ── マスクテクスチャ ──────────────────────────────────────────

    fn ensure_mask_texture(&mut self, ctx: &egui::Context) {
        if self.erase_mask_texture.is_some() { return; }
        let mask = match &self.erase_mask {
            Some(m) => m,
            None => return,
        };
        let [w, h] = self.erase_mask_size;
        let mut rgba = vec![0u8; w * h * 4];
        for i in 0..mask.len() {
            if mask[i] {
                rgba[i * 4]     = 255;
                rgba[i * 4 + 1] = 60;
                rgba[i * 4 + 2] = 60;
                rgba[i * 4 + 3] = 140;
            }
        }
        let ci = egui::ColorImage::from_rgba_unmultiplied([w, h], &rgba);
        let tex = ctx.load_texture("erase_mask", ci, egui::TextureOptions::NEAREST);
        self.erase_mask_texture = Some(tex);
    }

    /// ツールパネルの矩形を返す。
    fn erase_panel_rect(&self, full_rect: egui::Rect) -> egui::Rect {
        let panel_pos = egui::pos2(full_rect.min.x + PANEL_MARGIN_X, full_rect.min.y + PANEL_MARGIN_Y);
        let base_h = 210.0;
        let brush_extra = if self.erase_tool == EraseTool::Brush { 40.0 } else { 0.0 };
        egui::Rect::from_min_size(panel_pos, egui::vec2(PANEL_W, base_h + brush_extra))
    }

    // ── 入力処理 ──────────────────────────────────────────────────

    /// ドラッグ入力を処理する（ツール別分岐）。
    pub(crate) fn handle_erase_paint(
        &mut self,
        ctx: &egui::Context,
        full_rect: egui::Rect,
        zoom_pan: Option<(f32, egui::Vec2)>,
    ) {
        let primary_down = ctx.input(|i| i.pointer.primary_down());
        let primary_released = ctx.input(|i| i.pointer.primary_released());
        let pointer_pos = ctx.input(|i| i.pointer.hover_pos());
        let paint = self.erase_paint_mode;

        // パネル上のクリックはツール操作に使わない
        let panel_rect = self.erase_panel_rect(full_rect);
        if let Some(pos) = pointer_pos {
            if panel_rect.contains(pos) {
                return;
            }
        }

        match self.erase_tool {
            EraseTool::Brush => {
                if primary_down {
                    if let Some(pos) = pointer_pos {
                        if let Some(img_pos) = self.screen_to_image_f32(pos, full_rect, zoom_pan) {
                            let prev = self.erase_last_paint_pos
                                .and_then(|p| self.screen_to_image_f32(p, full_rect, zoom_pan))
                                .unwrap_or(img_pos);
                            self.paint_brush_line(prev, img_pos, paint);
                        }
                        self.erase_last_paint_pos = Some(pos);
                    }
                } else {
                    self.erase_last_paint_pos = None;
                }
            }
            EraseTool::Lasso => {
                if primary_down {
                    if let Some(pos) = pointer_pos {
                        if let Some(img_pos) = self.screen_to_image_f32(pos, full_rect, zoom_pan) {
                            // サンプリング間引き
                            if self.erase_lasso_points.last()
                                .map(|&(lx, ly)| {
                                    let dx = lx - img_pos.0;
                                    let dy = ly - img_pos.1;
                                    dx * dx + dy * dy > 4.0
                                })
                                .unwrap_or(true)
                            {
                                self.erase_lasso_points.push(img_pos);
                            }
                        }
                    }
                }
                if primary_released && self.erase_lasso_points.len() >= 3 {
                    let pts: Vec<(f32, f32)> = self.erase_lasso_points.drain(..).collect();
                    self.paint_polygon(&pts, paint);
                } else if primary_released {
                    self.erase_lasso_points.clear();
                }
            }
            EraseTool::VertLine => {
                if primary_down {
                    if let Some(pos) = pointer_pos {
                        if let Some(img_pos) = self.screen_to_image_f32(pos, full_rect, zoom_pan) {
                            if self.erase_line_start.is_none() {
                                self.erase_line_start = Some(img_pos);
                            }
                            self.erase_line_end = Some(img_pos);
                        }
                    }
                }
                if primary_released {
                    if let (Some((x0, _)), Some((x1, _))) = (self.erase_line_start, self.erase_line_end) {
                        let [w, h] = self.erase_mask_size;
                        let lx = (x0.min(x1).max(0.0)) as usize;
                        let rx = (x0.max(x1).ceil().min(w as f32)) as usize;
                        self.paint_rect(lx, 0, rx, h, paint);
                    }
                    self.erase_line_start = None;
                    self.erase_line_end = None;
                }
            }
            EraseTool::HorizLine => {
                if primary_down {
                    if let Some(pos) = pointer_pos {
                        if let Some(img_pos) = self.screen_to_image_f32(pos, full_rect, zoom_pan) {
                            if self.erase_line_start.is_none() {
                                self.erase_line_start = Some(img_pos);
                            }
                            self.erase_line_end = Some(img_pos);
                        }
                    }
                }
                if primary_released {
                    if let (Some((_, y0)), Some((_, y1))) = (self.erase_line_start, self.erase_line_end) {
                        let [w, h] = self.erase_mask_size;
                        let ty = (y0.min(y1).max(0.0)) as usize;
                        let by = (y0.max(y1).ceil().min(h as f32)) as usize;
                        self.paint_rect(0, ty, w, by, paint);
                    }
                    self.erase_line_start = None;
                    self.erase_line_end = None;
                }
            }
        }
    }

    // ── 描画 ──────────────────────────────────────────────────────

    /// マスクオーバーレイ + ツールパネル + カーソルを描画する。
    pub(crate) fn draw_erase_overlay(
        &mut self,
        ui: &mut egui::Ui,
        ctx: &egui::Context,
        full_rect: egui::Rect,
        zoom_pan: Option<(f32, egui::Vec2)>,
    ) {
        // マスクオーバーレイ描画
        self.ensure_mask_texture(ctx);
        if let Some(ref tex) = self.erase_mask_texture {
            let Some((_total_scale, img_rect)) = self.erase_image_layout(full_rect, zoom_pan) else { return; };
            let painter = if zoom_pan.is_some() {
                ui.painter().with_clip_rect(full_rect)
            } else {
                ui.painter().clone()
            };
            painter.image(
                tex.id(), img_rect,
                egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
                egui::Color32::WHITE,
            );
        }

        // ドラッグ中のプレビュー
        self.draw_tool_preview(ui, full_rect, zoom_pan);

        // カーソル
        ctx.output_mut(|o| o.cursor_icon = egui::CursorIcon::Crosshair);
        self.draw_brush_cursor(ui, ctx, full_rect, zoom_pan);

        // ツールパネル
        self.draw_erase_panel(ui, ctx, full_rect);
    }

    /// ドラッグ中のプレビュー表示。
    fn draw_tool_preview(
        &self,
        ui: &mut egui::Ui,
        full_rect: egui::Rect,
        zoom_pan: Option<(f32, egui::Vec2)>,
    ) {
        let color = if self.erase_paint_mode {
            egui::Color32::from_rgba_unmultiplied(255, 100, 100, 120)
        } else {
            egui::Color32::from_rgba_unmultiplied(100, 200, 255, 120)
        };
        let stroke_color = if self.erase_paint_mode {
            egui::Color32::from_rgba_unmultiplied(255, 200, 200, 200)
        } else {
            egui::Color32::from_rgba_unmultiplied(200, 230, 255, 200)
        };

        match self.erase_tool {
            EraseTool::Lasso if !self.erase_lasso_points.is_empty() => {
                let pts: Vec<egui::Pos2> = self.erase_lasso_points.iter()
                    .map(|&(x, y)| self.image_to_screen(x, y, full_rect, zoom_pan))
                    .collect();
                if pts.len() >= 2 {
                    for i in 0..pts.len() - 1 {
                        ui.painter().line_segment(
                            [pts[i], pts[i + 1]],
                            egui::Stroke::new(2.0, stroke_color),
                        );
                    }
                    // 始点と現在位置を破線で
                    ui.painter().line_segment(
                        [*pts.last().unwrap(), pts[0]],
                        egui::Stroke::new(1.0, egui::Color32::from_rgba_unmultiplied(255, 255, 255, 100)),
                    );
                }
            }
            EraseTool::VertLine => {
                if let (Some((x0, _)), Some((x1, _))) = (self.erase_line_start, self.erase_line_end) {
                    let [_w, h] = self.erase_mask_size;
                    let p0 = self.image_to_screen(x0.min(x1), 0.0, full_rect, zoom_pan);
                    let p1 = self.image_to_screen(x0.max(x1), h as f32, full_rect, zoom_pan);
                    let rect = egui::Rect::from_min_max(p0, p1);
                    ui.painter().rect_filled(rect, 0.0, color);
                    ui.painter().rect_stroke(rect, 0.0, egui::Stroke::new(1.0, stroke_color), egui::StrokeKind::Outside);
                }
            }
            EraseTool::HorizLine => {
                if let (Some((_, y0)), Some((_, y1))) = (self.erase_line_start, self.erase_line_end) {
                    let [w, _h] = self.erase_mask_size;
                    let p0 = self.image_to_screen(0.0, y0.min(y1), full_rect, zoom_pan);
                    let p1 = self.image_to_screen(w as f32, y0.max(y1), full_rect, zoom_pan);
                    let rect = egui::Rect::from_min_max(p0, p1);
                    ui.painter().rect_filled(rect, 0.0, color);
                    ui.painter().rect_stroke(rect, 0.0, egui::Stroke::new(1.0, stroke_color), egui::StrokeKind::Outside);
                }
            }
            _ => {}
        }
    }

    /// 筆ツール時のカーソル表示。
    fn draw_brush_cursor(
        &self,
        ui: &mut egui::Ui,
        ctx: &egui::Context,
        full_rect: egui::Rect,
        zoom_pan: Option<(f32, egui::Vec2)>,
    ) {
        if self.erase_tool != EraseTool::Brush { return; }
        if let Some(pos) = ctx.input(|i| i.pointer.hover_pos()) {
            if full_rect.contains(pos) {
                let Some((total_scale, _)) = self.erase_image_layout(full_rect, zoom_pan) else { return; };
                let screen_r = self.erase_brush_radius * total_scale;
                ui.painter().circle_stroke(
                    pos, screen_r,
                    egui::Stroke::new(1.5, egui::Color32::from_rgba_unmultiplied(255, 255, 255, 200)),
                );
            }
        }
    }

    // ── ツールパネル ──────────────────────────────────────────────

    fn draw_erase_panel(&mut self, _ui: &mut egui::Ui, ctx: &egui::Context, full_rect: egui::Rect) {
        let panel_rect = self.erase_panel_rect(full_rect);

        // egui::Area (Foreground) でパネルを描画。interactable=true でクリックを受け取る。
        egui::Area::new(egui::Id::new("erase_tool_panel"))
            .order(egui::Order::Foreground)
            .fixed_pos(panel_rect.min)
            .interactable(true)
            .show(ctx, |child| {
                // 背景を描画してクリック範囲を確保
                let (_resp, painter) = child.allocate_painter(panel_rect.size(), egui::Sense::click_and_drag());
                painter.rect_filled(panel_rect, 6.0, egui::Color32::from_rgba_unmultiplied(20, 20, 20, 220));
                painter.rect_stroke(
                    panel_rect, 6.0,
                    egui::Stroke::new(1.0, egui::Color32::from_rgba_unmultiplied(255, 255, 255, 40)),
                    egui::StrokeKind::Outside,
                );

        let x0 = panel_rect.min.x + 10.0;
        let pw = panel_rect.width() - 20.0;
        let mut y = panel_rect.min.y + 8.0;

        // ── ヘッダー ──
        child.painter().text(
            egui::pos2(x0, y),
            egui::Align2::LEFT_TOP,
            "消しゴム",
            egui::FontId::proportional(15.0),
            egui::Color32::WHITE,
        );
        y += 24.0;

        // ── 描画/消去 モード切り替え ──
        let mode_labels = [("描画", true), ("消去", false)];
        let mode_w = (pw - 4.0) / 2.0;
        for (i, &(label, is_paint)) in mode_labels.iter().enumerate() {
            let btn_rect = egui::Rect::from_min_size(
                egui::pos2(x0 + i as f32 * (mode_w + 4.0), y),
                egui::vec2(mode_w, 24.0),
            );
            let is_active = self.erase_paint_mode == is_paint;
            let bg = if is_active {
                if is_paint {
                    egui::Color32::from_rgb(180, 60, 60)
                } else {
                    egui::Color32::from_rgb(60, 120, 180)
                }
            } else {
                egui::Color32::from_gray(50)
            };
            let resp = child.allocate_rect(btn_rect, egui::Sense::click());
            if resp.hovered() && !is_active {
                child.painter().rect_filled(btn_rect, 3.0, egui::Color32::from_gray(70));
            } else {
                child.painter().rect_filled(btn_rect, 3.0, bg);
            }
            child.painter().text(
                btn_rect.center(), egui::Align2::CENTER_CENTER,
                label, egui::FontId::proportional(12.0), egui::Color32::WHITE,
            );
            if resp.clicked() {
                self.erase_paint_mode = is_paint;
            }
        }
        y += 32.0;

        // ── ツール選択 ──
        let tools = [
            ("囲み", EraseTool::Lasso),
            ("筆", EraseTool::Brush),
            ("縦線", EraseTool::VertLine),
            ("横線", EraseTool::HorizLine),
        ];
        let tool_w = (pw - 8.0) / 2.0;
        for (i, &(label, tool)) in tools.iter().enumerate() {
            let col = i % 2;
            let row = i / 2;
            let btn_rect = egui::Rect::from_min_size(
                egui::pos2(x0 + col as f32 * (tool_w + 8.0), y + row as f32 * 28.0),
                egui::vec2(tool_w, 24.0),
            );
            let is_active = self.erase_tool == tool;
            let bg = if is_active {
                egui::Color32::from_rgb(60, 120, 200)
            } else {
                egui::Color32::from_gray(50)
            };
            let resp = child.allocate_rect(btn_rect, egui::Sense::click());
            if resp.hovered() && !is_active {
                child.painter().rect_filled(btn_rect, 3.0, egui::Color32::from_gray(70));
            } else {
                child.painter().rect_filled(btn_rect, 3.0, bg);
            }
            child.painter().text(
                btn_rect.center(), egui::Align2::CENTER_CENTER,
                label, egui::FontId::proportional(12.0), egui::Color32::WHITE,
            );
            if resp.clicked() {
                self.erase_tool = tool;
            }
        }
        y += 60.0;

        // ── ブラシサイズスライダー（筆ツール時のみ）──
        if self.erase_tool == EraseTool::Brush {
            child.painter().text(
                egui::pos2(x0, y),
                egui::Align2::LEFT_TOP,
                "サイズ",
                egui::FontId::proportional(11.0),
                egui::Color32::from_gray(180),
            );
            y += 16.0;
            let slider_rect = egui::Rect::from_min_size(
                egui::pos2(x0, y),
                egui::vec2(pw, 20.0),
            );
            let mut slider_child = child.new_child(egui::UiBuilder::new().max_rect(slider_rect));
            let max_r = self.erase_mask_size[0].max(self.erase_mask_size[1]) as f32 / 20.0;
            slider_child.add(
                egui::Slider::new(&mut self.erase_brush_radius, 1.0..=max_r)
                    .step_by(1.0)
                    .show_value(false),
            );
            y += 26.0;
        }

        // ── セパレーター ──
        child.painter().line_segment(
            [egui::pos2(x0, y), egui::pos2(x0 + pw, y)],
            egui::Stroke::new(1.0, egui::Color32::from_gray(60)),
        );
        y += 6.0;

        // ── マスク全削除ボタン ──
        let del_rect = egui::Rect::from_min_size(
            egui::pos2(x0, y),
            egui::vec2(pw, 22.0),
        );
        let del_resp = child.allocate_rect(del_rect, egui::Sense::click());
        let del_bg = if del_resp.hovered() {
            egui::Color32::from_rgba_unmultiplied(200, 50, 50, 200)
        } else {
            egui::Color32::from_gray(50)
        };
        child.painter().rect_filled(del_rect, 3.0, del_bg);
        child.painter().text(
            del_rect.center(), egui::Align2::CENTER_CENTER,
            "マスク全削除", egui::FontId::proportional(11.0), egui::Color32::WHITE,
        );
        if del_resp.clicked() {
            let [w, h] = self.erase_mask_size;
            self.erase_mask = Some(vec![false; w * h]);
            self.erase_mask_texture = None;
            if let Some(fs_idx) = self.fullscreen_idx {
                // DB からも削除
                if let Some(key) = self.page_path_key(fs_idx) {
                    if let Some(db) = &self.mask_db {
                        let _ = db.delete(&key);
                    }
                }
                // 表示を元画像に戻す
                if let Some(base) = self.erase_base_cache.get(&fs_idx) {
                    let tex = ctx.load_texture(
                        format!("fs_restored_{fs_idx}"),
                        base.as_ref().clone(),
                        egui::TextureOptions::LINEAR,
                    );
                    let target = if self.ai_upscale_cache.contains_key(&fs_idx) {
                        &mut self.ai_upscale_cache
                    } else {
                        &mut self.fs_cache
                    };
                    target.insert(fs_idx, FsCacheEntry::Static {
                        tex,
                        pixels: Arc::clone(base),
                    });
                }
            }
        }
        y += 28.0;

        // ── ヘルプテキスト ──
        let help = "E: 補完実行  ESC: 終了";
        child.painter().text(
            egui::pos2(x0, y),
            egui::Align2::LEFT_TOP,
            help,
            egui::FontId::proportional(10.0),
            egui::Color32::from_gray(140),
        );

        }); // egui::Area::show

        ctx.request_repaint();
    }

    // ── Inpaint 実行 ──────────────────────────────────────────────

    /// MI-GAN inpaint を実行。実行前にマスクを DB に保存する。
    pub(crate) fn execute_erase_inpaint(&mut self, ctx: &egui::Context, fs_idx: usize) {
        let mask = match self.erase_mask.take() {
            Some(m) => m,
            None => { self.reset_erase_mode(); return; }
        };
        let original = match self.erase_base_cache.get(&fs_idx) {
            Some(p) => Arc::clone(p),
            None => { self.reset_erase_mode(); return; }
        };
        let [w, h] = self.erase_mask_size;

        if !mask.iter().any(|&m| m) {
            self.reset_erase_mode();
            return;
        }

        // マスクを DB に保存
        if let Some(key) = self.page_path_key(fs_idx) {
            if let Some(db) = &self.mask_db {
                let _ = db.set(&key, &mask, w, h);
            }
        }

        let masked_count = mask.iter().filter(|&&m| m).count();
        crate::logger::log(format!("erase: inpaint start, masked pixels={masked_count}"));

        self.ensure_ai_runtime();
        let result = self.try_migan_inpaint(&original, &mask, w, h)
            .unwrap_or_else(|e| {
                crate::logger::log(format!("[erase] MI-GAN failed: {e}, falling back to diffusion"));
                inpaint_diffuse(&original, &mask, w, h)
            });

        let tex = ctx.load_texture(
            format!("fs_inpainted_{fs_idx}"),
            result.clone(),
            egui::TextureOptions::LINEAR,
        );
        if self.ai_upscale_cache.contains_key(&fs_idx) {
            self.ai_upscale_cache.insert(fs_idx, FsCacheEntry::Static {
                tex,
                pixels: Arc::new(result),
            });
        } else {
            self.fs_cache.insert(fs_idx, FsCacheEntry::Static {
                tex,
                pixels: Arc::new(result),
            });
        }
        self.reset_erase_mode();
        crate::logger::log("erase: inpaint complete".to_string());
    }

    /// 画像ロード完了後に保存済みマスクがあれば自動で inpaint を適用する。
    /// `poll_prefetch` から呼ばれる。
    pub(crate) fn auto_apply_saved_mask(&mut self, ctx: &egui::Context, idx: usize) {
        // erase mode 中は手動操作に任せる
        if self.erase_mode { return; }

        let key = match self.page_path_key(idx) {
            Some(k) => k,
            None => return,
        };

        // DB にマスクがあるか確認
        let pixels = match self.fs_cache.get(&idx) {
            Some(FsCacheEntry::Static { pixels, .. }) => Arc::clone(pixels),
            _ => return,
        };
        let [w, h] = pixels.size;

        let mask = match self.mask_db.as_ref().and_then(|db| db.get(&key, w, h)) {
            Some(m) => m,
            None => return,
        };
        if !mask.iter().any(|&m| m) { return; }

        crate::logger::log(format!("erase: auto-applying saved mask for idx={idx}"));

        // 元画像を base_cache に保存（サイズが変わった場合は更新）
        let need_update = self.erase_base_cache.get(&idx)
            .map(|old| old.size != pixels.size)
            .unwrap_or(true);
        if need_update {
            self.erase_base_cache.insert(idx, Arc::clone(&pixels));
        }

        // inpaint 実行
        self.ensure_ai_runtime();
        let result = self.try_migan_inpaint(&pixels, &mask, w, h)
            .unwrap_or_else(|e| {
                crate::logger::log(format!("[erase] auto-apply MI-GAN failed: {e}, falling back to diffusion"));
                inpaint_diffuse(&pixels, &mask, w, h)
            });

        let tex = ctx.load_texture(
            format!("fs_inpainted_{idx}"),
            result.clone(),
            egui::TextureOptions::LINEAR,
        );
        self.fs_cache.insert(idx, FsCacheEntry::Static {
            tex,
            pixels: Arc::new(result),
        });

        // AI アップスケールのキャッシュをクリアして、inpaint後の画像で再処理させる
        self.ai_upscale_cache.remove(&idx);
        self.ai_upscale_failed.remove(&idx);
        if let Some((cancel, _)) = self.ai_upscale_pending.remove(&idx) {
            cancel.store(true, std::sync::atomic::Ordering::Relaxed);
        }
        // 補正キャッシュもクリア
        self.adjustment_cache.remove(&idx);
        self.adjustment_sharpened.remove(&idx);
    }

    fn try_migan_inpaint(
        &mut self,
        original: &egui::ColorImage,
        mask: &[bool],
        w: usize,
        h: usize,
    ) -> Result<egui::ColorImage, crate::ai::AiError> {
        let runtime = self.ai_runtime.clone()
            .ok_or_else(|| crate::ai::AiError::Ort("AI runtime not available".to_string()))?;
        let manager = self.ai_model_manager.clone()
            .ok_or_else(|| crate::ai::AiError::Ort("Model manager not available".to_string()))?;

        let kind = crate::ai::ModelKind::InpaintMiGan;
        let model_path = manager.model_path(kind)
            .ok_or_else(|| crate::ai::AiError::ModelNotFound(kind))?;
        if !runtime.is_loaded(kind) {
            runtime.load_model(kind, &model_path)?;
        }

        let cancel = Arc::new(std::sync::atomic::AtomicBool::new(false));
        inpaint_migan(&runtime, original, mask, w, h, &cancel)
    }
}

// ═══════════════════════════════════════════════════════════════════════
// Free functions
// ═══════════════════════════════════════════════════════════════════════

/// タイルオーバーラップ幅（ピクセル）。
const TILE_OVERLAP: usize = 64;

/// MI-GAN によるタイル分割 inpainting。
/// マスク領域を 512×512 タイルでカバーし、オーバーラップ線形ブレンドで結合する。
fn inpaint_migan(
    runtime: &crate::ai::runtime::AiRuntime,
    original: &egui::ColorImage,
    mask: &[bool],
    w: usize,
    h: usize,
    cancel: &Arc<std::sync::atomic::AtomicBool>,
) -> Result<egui::ColorImage, crate::ai::AiError> {
    use std::sync::atomic::Ordering;

    // マスクのバウンディングボックス
    let (mut min_x, mut min_y, mut max_x, mut max_y) = (w, h, 0usize, 0usize);
    for py in 0..h {
        for px in 0..w {
            if mask[py * w + px] {
                min_x = min_x.min(px);
                min_y = min_y.min(py);
                max_x = max_x.max(px + 1);
                max_y = max_y.max(py + 1);
            }
        }
    }
    if min_x >= max_x || min_y >= max_y {
        return Err(crate::ai::AiError::ImageProcessing("No masked pixels".to_string()));
    }

    // マスク周囲にコンテキストパディングを追加（タイルが周辺情報を得るため）
    let ctx_pad = MIGAN_SIZE / 4; // 128px
    let region_x0 = min_x.saturating_sub(ctx_pad);
    let region_y0 = min_y.saturating_sub(ctx_pad);
    let region_x1 = (max_x + ctx_pad).min(w);
    let region_y1 = (max_y + ctx_pad).min(h);
    let region_w = region_x1 - region_x0;
    let region_h = region_y1 - region_y0;

    // タイル分割を計算
    let tiles = compute_inpaint_tiles(region_w, region_h, MIGAN_SIZE, TILE_OVERLAP);

    crate::logger::log(format!(
        "[erase] MI-GAN tiled: region ({region_x0},{region_y0})-({region_x1},{region_y1}) = {region_w}x{region_h}, {} tiles",
        tiles.len()
    ));

    // 累積バッファ（region 座標系、RGB float + 重み）
    let rpixels = region_w * region_h;
    let mut accum_r = vec![0.0f32; rpixels];
    let mut accum_g = vec![0.0f32; rpixels];
    let mut accum_b = vec![0.0f32; rpixels];
    let mut accum_w = vec![0.0f32; rpixels];

    // マスクされていない領域は元画像の値を初期化
    for ry in 0..region_h {
        for rx in 0..region_w {
            let src_idx = (region_y0 + ry) * w + (region_x0 + rx);
            if !mask[src_idx] {
                let c = original.pixels[src_idx];
                let ri = ry * region_w + rx;
                accum_r[ri] = c.r() as f32;
                accum_g[ri] = c.g() as f32;
                accum_b[ri] = c.b() as f32;
                accum_w[ri] = 1.0;
            }
        }
    }

    for (ti, tile) in tiles.iter().enumerate() {
        if cancel.load(Ordering::Relaxed) {
            return Err(crate::ai::AiError::Cancelled);
        }

        // タイル内にマスクピクセルがなければスキップ
        let has_mask = (tile.y..tile.y + tile.h).any(|ty| {
            (tile.x..tile.x + tile.w).any(|tx| {
                let gx = region_x0 + tx;
                let gy = region_y0 + ty;
                gx < w && gy < h && mask[gy * w + gx]
            })
        });
        if !has_mask { continue; }

        // タイル領域を切り出して 512×512 入力テンソルを構築
        let s = MIGAN_SIZE;
        let mut input_nchw = ndarray::Array4::<f32>::zeros((1, 4, s, s));

        for iy in 0..s {
            for ix in 0..s {
                // タイル座標 → region 座標 → 画像座標 (浮動小数点で精密マッピング)
                let rx = tile.x + (ix as f32 * tile.w as f32 / s as f32) as usize;
                let ry = tile.y + (iy as f32 * tile.h as f32 / s as f32) as usize;
                let gx = region_x0 + rx;
                let gy = region_y0 + ry;

                if gx < w && gy < h {
                    let src_idx = gy * w + gx;
                    let is_masked = mask[src_idx];
                    let m = if is_masked { 0.0f32 } else { 1.0f32 };
                    let c = original.pixels[src_idx];
                    let r = c.r() as f32 / 255.0 * 2.0 - 1.0;
                    let g = c.g() as f32 / 255.0 * 2.0 - 1.0;
                    let b = c.b() as f32 / 255.0 * 2.0 - 1.0;
                    input_nchw[[0, 0, iy, ix]] = m - 0.5;
                    input_nchw[[0, 1, iy, ix]] = r * m;
                    input_nchw[[0, 2, iy, ix]] = g * m;
                    input_nchw[[0, 3, iy, ix]] = b * m;
                } else {
                    input_nchw[[0, 0, iy, ix]] = -0.5; // masked
                }
            }
        }

        let input_tensor = ort::value::Tensor::from_array(input_nchw)
            .map_err(|e| crate::ai::AiError::Ort(format!("Input tensor: {e}")))?;

        // MI-GAN 推論
        let tile_rgb = runtime.with_session(crate::ai::ModelKind::InpaintMiGan, |session| {
            let outputs = session
                .run(ort::inputs!["input" => input_tensor])
                .map_err(|e| crate::ai::AiError::Ort(format!("MI-GAN run: {e}")))?;
            let (_shape, raw) = outputs[0]
                .try_extract_tensor::<f32>()
                .map_err(|e| crate::ai::AiError::Ort(format!("MI-GAN extract: {e}")))?;
            // NCHW [-1,1] → RGB [0,255]
            let mut rgb = vec![0.0f32; s * s * 3];
            for iy in 0..s {
                for ix in 0..s {
                    let dst = (iy * s + ix) * 3;
                    rgb[dst]     = ((raw.get(0 * s * s + iy * s + ix).copied().unwrap_or(0.0) * 0.5 + 0.5) * 255.0).clamp(0.0, 255.0);
                    rgb[dst + 1] = ((raw.get(1 * s * s + iy * s + ix).copied().unwrap_or(0.0) * 0.5 + 0.5) * 255.0).clamp(0.0, 255.0);
                    rgb[dst + 2] = ((raw.get(2 * s * s + iy * s + ix).copied().unwrap_or(0.0) * 0.5 + 0.5) * 255.0).clamp(0.0, 255.0);
                }
            }
            Ok(rgb)
        })?;

        // タイル出力を重み付きで累積バッファに加算
        let is_first_x = tile.x == 0;
        let is_first_y = tile.y == 0;
        let is_last_x = tile.x + tile.w >= region_w;
        let is_last_y = tile.y + tile.h >= region_h;
        let ramp = TILE_OVERLAP as f32;

        for iy in 0..s {
            for ix in 0..s {
                // 512 座標 → タイル内座標 → region 座標 (浮動小数点で精密マッピング)
                let tx = (ix as f32 * tile.w as f32 / s as f32) as usize;
                let ty = (iy as f32 * tile.h as f32 / s as f32) as usize;
                let rx = tile.x + tx;
                let ry = tile.y + ty;
                if rx >= region_w || ry >= region_h { continue; }

                let gx = region_x0 + rx;
                let gy = region_y0 + ry;
                if gx >= w || gy >= h { continue; }

                // マスクされたピクセルのみ inpaint 結果を使用
                if !mask[gy * w + gx] { continue; }

                // 辺からの距離ベースの重み
                let dist_left = if is_first_x { ramp } else { tx as f32 };
                let dist_right = if is_last_x { ramp } else { (tile.w - 1 - tx) as f32 };
                let dist_top = if is_first_y { ramp } else { ty as f32 };
                let dist_bot = if is_last_y { ramp } else { (tile.h - 1 - ty) as f32 };
                let wx = (dist_left.min(dist_right) / ramp).clamp(1e-4, 1.0);
                let wy = (dist_top.min(dist_bot) / ramp).clamp(1e-4, 1.0);
                let weight = wx * wy;

                let ri = ry * region_w + rx;
                let si = (iy * s + ix) * 3;
                accum_r[ri] += tile_rgb[si] * weight;
                accum_g[ri] += tile_rgb[si + 1] * weight;
                accum_b[ri] += tile_rgb[si + 2] * weight;
                accum_w[ri] += weight;
            }
        }

        crate::logger::log(format!("[erase] MI-GAN tile {}/{}", ti + 1, tiles.len()));
    }

    crate::logger::log("[erase] MI-GAN tiled inference done, compositing...".to_string());

    // 元画像にマスク部分のみ累積結果を合成
    let mut pixels = original.pixels.clone();
    for ry in 0..region_h {
        for rx in 0..region_w {
            let gx = region_x0 + rx;
            let gy = region_y0 + ry;
            if gx >= w || gy >= h { continue; }
            let src_idx = gy * w + gx;
            if !mask[src_idx] { continue; }

            let ri = ry * region_w + rx;
            let wt = accum_w[ri].max(1e-6);
            let r = (accum_r[ri] / wt).clamp(0.0, 255.0) as u8;
            let g = (accum_g[ri] / wt).clamp(0.0, 255.0) as u8;
            let b = (accum_b[ri] / wt).clamp(0.0, 255.0) as u8;
            pixels[src_idx] = egui::Color32::from_rgb(r, g, b);
        }
    }

    Ok(egui::ColorImage::new([w, h], pixels))
}

/// マスク領域をカバーするタイル分割を計算する。
fn compute_inpaint_tiles(
    region_w: usize,
    region_h: usize,
    tile_size: usize,
    overlap: usize,
) -> Vec<TileRect> {
    let mut tiles = Vec::new();
    let step = tile_size.saturating_sub(overlap).max(1);

    let mut y = 0usize;
    loop {
        let ty = y;
        let th = tile_size.min(region_h.saturating_sub(ty));
        if th == 0 { break; }

        let mut x = 0usize;
        loop {
            let tx = x;
            let tw = tile_size.min(region_w.saturating_sub(tx));
            if tw == 0 { break; }
            tiles.push(TileRect { x: tx, y: ty, w: tw, h: th });

            if tx + tw >= region_w { break; }
            x += step;
            if x + tile_size > region_w {
                x = region_w.saturating_sub(tile_size);
            }
        }

        if ty + th >= region_h { break; }
        y += step;
        if y + tile_size > region_h {
            y = region_h.saturating_sub(tile_size);
        }
    }

    tiles
}

struct TileRect {
    x: usize,
    y: usize,
    w: usize,
    h: usize,
}

fn inpaint_diffuse(original: &egui::ColorImage, mask: &[bool], w: usize, h: usize) -> egui::ColorImage {
    // マスクのバウンディングボックスに限定して処理
    let (mut bx0, mut by0, mut bx1, mut by1) = (w, h, 0, 0);
    for py in 0..h {
        for px in 0..w {
            if mask[py * w + px] {
                bx0 = bx0.min(px);
                by0 = by0.min(py);
                bx1 = bx1.max(px + 1);
                by1 = by1.max(py + 1);
            }
        }
    }
    // パディング（近傍参照用）
    let bx0 = bx0.saturating_sub(1);
    let by0 = by0.saturating_sub(1);
    let bx1 = (bx1 + 1).min(w);
    let by1 = (by1 + 1).min(h);

    let mut pixels: Vec<[f32; 4]> = original.pixels.iter()
        .map(|c| [c.r() as f32, c.g() as f32, c.b() as f32, c.a() as f32])
        .collect();
    let mut filled = vec![false; w * h];
    for i in 0..mask.len() {
        filled[i] = !mask[i];
    }

    // ダブルバッファで swap（clone を回避）
    let mut buf_pixels = pixels.clone();
    let mut buf_filled = filled.clone();
    let max_iters = ((bx1 - bx0).max(by1 - by0) as u32).min(2000);
    let neighbors: [(isize, isize); 4] = [(-1, 0), (1, 0), (0, -1), (0, 1)];
    for _iter in 0..max_iters {
        let mut any_filled = false;
        for py in by0..by1 {
            for px in bx0..bx1 {
                let idx = py * w + px;
                if filled[idx] { continue; }
                let mut sum = [0.0f32; 4];
                let mut count = 0u32;
                for (dx, dy) in &neighbors {
                    let nx = px as isize + dx;
                    let ny = py as isize + dy;
                    if nx >= 0 && ny >= 0 && (nx as usize) < w && (ny as usize) < h {
                        let ni = ny as usize * w + nx as usize;
                        if filled[ni] {
                            let p = pixels[ni];
                            sum[0] += p[0]; sum[1] += p[1]; sum[2] += p[2]; sum[3] += p[3];
                            count += 1;
                        }
                    }
                }
                if count > 0 {
                    buf_pixels[idx] = [sum[0]/count as f32, sum[1]/count as f32, sum[2]/count as f32, sum[3]/count as f32];
                    buf_filled[idx] = true;
                    any_filled = true;
                }
            }
        }
        std::mem::swap(&mut pixels, &mut buf_pixels);
        std::mem::swap(&mut filled, &mut buf_filled);
        // swap 後に buf を pixels からコピー（次の反復で読む値を最新にする）
        for py in by0..by1 {
            for px in bx0..bx1 {
                let idx = py * w + px;
                buf_pixels[idx] = pixels[idx];
                buf_filled[idx] = filled[idx];
            }
        }
        if !any_filled { break; }
    }
    let rgba: Vec<u8> = pixels.iter()
        .flat_map(|p| [
            p[0].round().clamp(0.0, 255.0) as u8,
            p[1].round().clamp(0.0, 255.0) as u8,
            p[2].round().clamp(0.0, 255.0) as u8,
            p[3].round().clamp(0.0, 255.0) as u8,
        ])
        .collect();
    egui::ColorImage::from_rgba_unmultiplied([w, h], &rgba)
}
