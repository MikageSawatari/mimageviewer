//! 消しゴム (Erase) モード: フルスクリーン画像の任意領域をマスクし、
//! MI-GAN で補完 (inpaint) する。
//!
//! - E キー 1回目: マスクモード開始（ドラッグで塗りつぶし）
//! - E キー 2回目: マスク領域を MI-GAN で復元し、モード終了

use eframe::egui;
use std::sync::Arc;

use crate::app::App;
use crate::fs_animation::FsCacheEntry;

/// MI-GAN の固定入力サイズ。
const MIGAN_SIZE: usize = 512;

impl App {
    /// 消しゴムモードに入る。現在の画像のピクセルデータを取得しマスクを初期化する。
    pub(crate) fn enter_erase_mode(&mut self, fs_idx: usize) {
        // AI アップスケール画像があればそちらを、なければ元画像を使う
        let pixels = self.ai_upscale_cache.get(&fs_idx)
            .or_else(|| self.fs_cache.get(&fs_idx))
            .and_then(|entry| match entry {
                FsCacheEntry::Static { pixels, .. } => Some(Arc::clone(pixels)),
                _ => None,
            });
        let pixels = match pixels {
            Some(p) => p,
            None => return,
        };
        let [w, h] = pixels.size;
        self.erase_mode = true;
        self.erase_mask = Some(vec![false; w * h]);
        self.erase_mask_size = [w, h];
        self.erase_mask_texture = None;
        self.erase_last_paint_pos = None;
        self.erase_original_pixels = Some(pixels);
        self.erase_inpaint_rx = None;
        crate::logger::log(format!("erase: enter mode, image={w}x{h}"));
    }

    /// 消しゴムモードをリセットする。
    pub(crate) fn reset_erase_mode(&mut self) {
        self.erase_mode = false;
        self.erase_mask = None;
        self.erase_mask_size = [0, 0];
        self.erase_mask_texture = None;
        self.erase_last_paint_pos = None;
        self.erase_original_pixels = None;
        self.erase_inpaint_rx = None;
    }

    /// 筆先サイズ (正方形の一辺) を返す。画像の長辺の 1/100。
    fn erase_brush_size(&self) -> usize {
        let [w, h] = self.erase_mask_size;
        let long_side = w.max(h);
        (long_side / 100).max(1)
    }

    /// スクリーン座標を画像ピクセル座標に変換する。
    fn screen_to_image_pos(
        &self,
        screen_pos: egui::Pos2,
        full_rect: egui::Rect,
        zoom_pan: Option<(f32, egui::Vec2)>,
    ) -> Option<(usize, usize)> {
        let [iw, ih] = self.erase_mask_size;
        if iw == 0 || ih == 0 { return None; }

        let display_size = egui::vec2(iw as f32, ih as f32);
        let fit_scale = (full_rect.width() / display_size.x)
            .min(full_rect.height() / display_size.y);
        let (total_scale, center) = match zoom_pan {
            Some((zoom, pan)) => (fit_scale * zoom, full_rect.center() + pan),
            None => (fit_scale, full_rect.center()),
        };
        let img_rect = egui::Rect::from_center_size(center, display_size * total_scale);

        let nx = (screen_pos.x - img_rect.min.x) / total_scale;
        let ny = (screen_pos.y - img_rect.min.y) / total_scale;

        if nx >= 0.0 && ny >= 0.0 && (nx as usize) < iw && (ny as usize) < ih {
            Some((nx as usize, ny as usize))
        } else {
            None
        }
    }

    /// マスクに正方形ブラシで塗る。from と to の間を線形補間してブラシを動かす。
    fn paint_mask_line(&mut self, from: (usize, usize), to: (usize, usize)) {
        let brush = self.erase_brush_size();
        let half = brush / 2;
        let [w, h] = self.erase_mask_size;
        let mask = match self.erase_mask.as_mut() {
            Some(m) => m,
            None => return,
        };

        let dx = to.0 as isize - from.0 as isize;
        let dy = to.1 as isize - from.1 as isize;
        let steps = dx.unsigned_abs().max(dy.unsigned_abs()).max(1);

        for step in 0..=steps {
            let t = step as f32 / steps as f32;
            let cx = (from.0 as f32 + dx as f32 * t) as usize;
            let cy = (from.1 as f32 + dy as f32 * t) as usize;

            let x0 = cx.saturating_sub(half);
            let y0 = cy.saturating_sub(half);
            let x1 = (cx + brush - half).min(w);
            let y1 = (cy + brush - half).min(h);

            for py in y0..y1 {
                for px in x0..x1 {
                    mask[py * w + px] = true;
                }
            }
        }
        // テクスチャキャッシュを無効化
        self.erase_mask_texture = None;
    }

    /// マスクオーバーレイテクスチャを生成/更新する。
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
                rgba[i * 4]     = 255; // R
                rgba[i * 4 + 1] = 60;  // G
                rgba[i * 4 + 2] = 60;  // B
                rgba[i * 4 + 3] = 140; // A
            }
        }
        let ci = egui::ColorImage::from_rgba_unmultiplied([w, h], &rgba);
        let tex = ctx.load_texture("erase_mask", ci, egui::TextureOptions::NEAREST);
        self.erase_mask_texture = Some(tex);
    }

    /// フルスクリーン描画中にマウスドラッグでマスクを塗る。
    pub(crate) fn handle_erase_paint(
        &mut self,
        ctx: &egui::Context,
        full_rect: egui::Rect,
        zoom_pan: Option<(f32, egui::Vec2)>,
    ) {
        let primary_down = ctx.input(|i| i.pointer.primary_down());
        let pointer_pos = ctx.input(|i| i.pointer.hover_pos());

        if primary_down {
            if let Some(pos) = pointer_pos {
                if let Some(img_pos) = self.screen_to_image_pos(pos, full_rect, zoom_pan) {
                    let prev = self.erase_last_paint_pos
                        .and_then(|p| self.screen_to_image_pos(p, full_rect, zoom_pan))
                        .unwrap_or(img_pos);
                    self.paint_mask_line(prev, img_pos);
                }
                self.erase_last_paint_pos = Some(pos);
            }
        } else {
            self.erase_last_paint_pos = None;
        }
    }

    /// マスクオーバーレイを画像の上に描画する。
    pub(crate) fn draw_erase_overlay(
        &mut self,
        ui: &mut egui::Ui,
        ctx: &egui::Context,
        full_rect: egui::Rect,
        zoom_pan: Option<(f32, egui::Vec2)>,
    ) {
        self.ensure_mask_texture(ctx);
        if let Some(ref tex) = self.erase_mask_texture {
            let [iw, ih] = self.erase_mask_size;
            let display_size = egui::vec2(iw as f32, ih as f32);
            let fit_scale = (full_rect.width() / display_size.x)
                .min(full_rect.height() / display_size.y);
            let (total_scale, center) = match zoom_pan {
                Some((zoom, pan)) => (fit_scale * zoom, full_rect.center() + pan),
                None => (fit_scale, full_rect.center()),
            };
            let img_rect = egui::Rect::from_center_size(center, display_size * total_scale);

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

        // カーソルをクロスヘアに変更
        ctx.output_mut(|o| o.cursor_icon = egui::CursorIcon::Crosshair);

        // ブラシカーソル表示
        if let Some(pos) = ctx.input(|i| i.pointer.hover_pos()) {
            if full_rect.contains(pos) {
                let brush = self.erase_brush_size();
                let [iw, ih] = self.erase_mask_size;
                let display_size = egui::vec2(iw as f32, ih as f32);
                let fit_scale = (full_rect.width() / display_size.x)
                    .min(full_rect.height() / display_size.y);
                let total_scale = match zoom_pan {
                    Some((zoom, _)) => fit_scale * zoom,
                    None => fit_scale,
                };
                let screen_brush = brush as f32 * total_scale;
                let half = screen_brush / 2.0;
                let rect = egui::Rect::from_min_size(
                    egui::pos2(pos.x - half, pos.y - half),
                    egui::vec2(screen_brush, screen_brush),
                );
                ui.painter().rect_stroke(
                    rect,
                    0.0,
                    egui::Stroke::new(1.5, egui::Color32::from_rgba_unmultiplied(255, 255, 255, 200)),
                    egui::StrokeKind::Outside,
                );
            }
        }

        // モードインジケーター
        let label = if self.erase_inpaint_rx.is_some() {
            "AI 補完処理中..."
        } else {
            "消しゴムモード — ドラッグで塗りつぶし、E で補完実行"
        };
        let font = egui::FontId::proportional(15.0);
        let pos = egui::pos2(full_rect.center().x, full_rect.max.y - 40.0);
        let galley = ui.painter().layout_no_wrap(
            label.to_string(), font.clone(), egui::Color32::WHITE,
        );
        let text_rect = egui::Align2::CENTER_BOTTOM.anchor_size(pos, galley.size());
        let bg = text_rect.expand(6.0);
        ui.painter().rect_filled(
            bg, 6.0,
            egui::Color32::from_rgba_unmultiplied(0, 0, 0, 200),
        );
        ui.painter().galley(text_rect.min, galley, egui::Color32::WHITE);
    }

    /// MI-GAN を使って inpaint を実行し、キャッシュを更新する。
    /// MI-GAN の GPU 推論は高速 (~22ms) なのでメインスレッドで同期実行する。
    pub(crate) fn execute_erase_inpaint(&mut self, ctx: &egui::Context, fs_idx: usize) {
        let mask = match self.erase_mask.take() {
            Some(m) => m,
            None => { self.reset_erase_mode(); return; }
        };
        let original = match &self.erase_original_pixels {
            Some(p) => Arc::clone(p),
            None => { self.reset_erase_mode(); return; }
        };

        let [w, h] = self.erase_mask_size;

        // マスクされたピクセルがなければ何もせず終了
        if !mask.iter().any(|&m| m) {
            self.reset_erase_mode();
            return;
        }

        let masked_count = mask.iter().filter(|&&m| m).count();
        crate::logger::log(format!("erase: inpaint start, masked pixels={masked_count}"));

        // AI ランタイムの確保とモデルロード
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
        // アップスケールキャッシュがあればそちらを更新、なければ fs_cache を更新
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

    /// MI-GAN モデルで inpaint を試みる。
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

/// MI-GAN によるマスク領域の inpainting。
/// マスクのバウンディングボックス + コンテキストを切り出し、512×512 にリサイズして推論する。
fn inpaint_migan(
    runtime: &crate::ai::runtime::AiRuntime,
    original: &egui::ColorImage,
    mask: &[bool],
    w: usize,
    h: usize,
    cancel: &Arc<std::sync::atomic::AtomicBool>,
) -> Result<egui::ColorImage, crate::ai::AiError> {
    use std::sync::atomic::Ordering;

    // マスクのバウンディングボックスを求める
    let (mut min_x, mut min_y, mut max_x, mut max_y) = (w, h, 0usize, 0usize);
    for y in 0..h {
        for x in 0..w {
            if mask[y * w + x] {
                min_x = min_x.min(x);
                min_y = min_y.min(y);
                max_x = max_x.max(x + 1);
                max_y = max_y.max(y + 1);
            }
        }
    }
    if min_x >= max_x || min_y >= max_y {
        return Err(crate::ai::AiError::ImageProcessing("No masked pixels".to_string()));
    }

    // コンテキスト領域を追加（マスク領域と同程度のパディング）
    let mask_w = max_x - min_x;
    let mask_h = max_y - min_y;
    let pad = (mask_w.max(mask_h) / 2).max(64);

    let crop_x0 = min_x.saturating_sub(pad);
    let crop_y0 = min_y.saturating_sub(pad);
    let crop_x1 = (max_x + pad).min(w);
    let crop_y1 = (max_y + pad).min(h);
    let crop_w = crop_x1 - crop_x0;
    let crop_h = crop_y1 - crop_y0;

    // 正方形にする（MI-GAN は正方形入力）
    let side = crop_w.max(crop_h);
    let sq_x0 = if side > crop_w {
        let shift = (side - crop_w) / 2;
        if crop_x0 >= shift { crop_x0 - shift } else { 0 }
    } else { crop_x0 };
    let sq_y0 = if side > crop_h {
        let shift = (side - crop_h) / 2;
        if crop_y0 >= shift { crop_y0 - shift } else { 0 }
    } else { crop_y0 };
    let sq_x1 = (sq_x0 + side).min(w);
    let sq_y1 = (sq_y0 + side).min(h);
    let sq_w = sq_x1 - sq_x0;
    let sq_h = sq_y1 - sq_y0;

    crate::logger::log(format!(
        "[erase] MI-GAN crop: ({sq_x0},{sq_y0})-({sq_x1},{sq_y1}) = {sq_w}x{sq_h}, mask bbox: ({min_x},{min_y})-({max_x},{max_y})"
    ));

    if cancel.load(Ordering::Relaxed) {
        return Err(crate::ai::AiError::Cancelled);
    }

    // 切り出し領域の RGB を [0,1] で取得
    let mut crop_rgb = vec![0.0f32; sq_w * sq_h * 3];
    let mut crop_mask = vec![false; sq_w * sq_h];
    for y in 0..sq_h {
        for x in 0..sq_w {
            let src_idx = (sq_y0 + y) * w + (sq_x0 + x);
            let dst_idx = y * sq_w + x;
            let c = original.pixels[src_idx];
            crop_rgb[dst_idx * 3]     = c.r() as f32 / 255.0;
            crop_rgb[dst_idx * 3 + 1] = c.g() as f32 / 255.0;
            crop_rgb[dst_idx * 3 + 2] = c.b() as f32 / 255.0;
            crop_mask[dst_idx] = mask[src_idx];
        }
    }

    // 512×512 にリサイズ
    let resized_rgb = resize_rgb_bilinear(&crop_rgb, sq_w, sq_h, MIGAN_SIZE, MIGAN_SIZE);
    let resized_mask = resize_mask_nearest(&crop_mask, sq_w, sq_h, MIGAN_SIZE, MIGAN_SIZE);

    // MI-GAN 入力テンソル構築: [1, 4, 512, 512]
    // ch0: mask - 0.5 (known=0.5, inpaint=-0.5)
    // ch1-3: image[-1,1] * mask
    let s = MIGAN_SIZE;
    let mut input_nchw = ndarray::Array4::<f32>::zeros((1, 4, s, s));
    for y in 0..s {
        for x in 0..s {
            let base = (y * s + x) * 3;
            let is_masked = resized_mask[y * s + x];
            let m = if is_masked { 0.0f32 } else { 1.0f32 };
            input_nchw[[0, 0, y, x]] = m - 0.5;
            input_nchw[[0, 1, y, x]] = (resized_rgb[base] * 2.0 - 1.0) * m;
            input_nchw[[0, 2, y, x]] = (resized_rgb[base + 1] * 2.0 - 1.0) * m;
            input_nchw[[0, 3, y, x]] = (resized_rgb[base + 2] * 2.0 - 1.0) * m;
        }
    }

    let input_tensor = ort::value::Tensor::from_array(input_nchw)
        .map_err(|e| crate::ai::AiError::Ort(format!("Input tensor: {e}")))?;

    if cancel.load(Ordering::Relaxed) {
        return Err(crate::ai::AiError::Cancelled);
    }

    // MI-GAN 推論
    let output_rgb = runtime.with_session(crate::ai::ModelKind::InpaintMiGan, |session| {
        let outputs = session
            .run(ort::inputs!["input" => input_tensor])
            .map_err(|e| crate::ai::AiError::Ort(format!("MI-GAN run: {e}")))?;

        let (_shape, raw) = outputs[0]
            .try_extract_tensor::<f32>()
            .map_err(|e| crate::ai::AiError::Ort(format!("MI-GAN extract: {e}")))?;

        // 出力 NCHW [1, 3, 512, 512], 範囲 [-1,1] → [0,1]
        let mut out_rgb = vec![0.0f32; s * s * 3];
        for y in 0..s {
            for x in 0..s {
                let dst = (y * s + x) * 3;
                out_rgb[dst]     = (raw.get(0 * s * s + y * s + x).copied().unwrap_or(0.0) * 0.5 + 0.5).clamp(0.0, 1.0);
                out_rgb[dst + 1] = (raw.get(1 * s * s + y * s + x).copied().unwrap_or(0.0) * 0.5 + 0.5).clamp(0.0, 1.0);
                out_rgb[dst + 2] = (raw.get(2 * s * s + y * s + x).copied().unwrap_or(0.0) * 0.5 + 0.5).clamp(0.0, 1.0);
            }
        }
        Ok(out_rgb)
    })?;

    crate::logger::log("[erase] MI-GAN inference done, compositing...".to_string());

    // 512×512 出力を元のクロップサイズに戻す
    let result_rgb = resize_rgb_bilinear(&output_rgb, MIGAN_SIZE, MIGAN_SIZE, sq_w, sq_h);

    // 元画像にマスク部分のみ合成
    let mut pixels = original.pixels.clone();
    for y in 0..sq_h {
        for x in 0..sq_w {
            let src_idx = (sq_y0 + y) * w + (sq_x0 + x);
            if mask[src_idx] {
                let base = (y * sq_w + x) * 3;
                let r = (result_rgb[base] * 255.0).clamp(0.0, 255.0) as u8;
                let g = (result_rgb[base + 1] * 255.0).clamp(0.0, 255.0) as u8;
                let b = (result_rgb[base + 2] * 255.0).clamp(0.0, 255.0) as u8;
                pixels[src_idx] = egui::Color32::from_rgb(r, g, b);
            }
        }
    }

    Ok(egui::ColorImage::new([w, h], pixels))
}

/// バイリニア補間による RGB f32 画像のリサイズ。
fn resize_rgb_bilinear(
    src: &[f32],
    src_w: usize,
    src_h: usize,
    dst_w: usize,
    dst_h: usize,
) -> Vec<f32> {
    let mut dst = vec![0.0f32; dst_w * dst_h * 3];
    let x_ratio = src_w as f32 / dst_w as f32;
    let y_ratio = src_h as f32 / dst_h as f32;

    for dy in 0..dst_h {
        let sy = dy as f32 * y_ratio;
        let y0 = (sy as usize).min(src_h.saturating_sub(1));
        let y1 = (y0 + 1).min(src_h.saturating_sub(1));
        let fy = sy - y0 as f32;

        for dx in 0..dst_w {
            let sx = dx as f32 * x_ratio;
            let x0 = (sx as usize).min(src_w.saturating_sub(1));
            let x1 = (x0 + 1).min(src_w.saturating_sub(1));
            let fx = sx - x0 as f32;

            for c in 0..3 {
                let v00 = src[(y0 * src_w + x0) * 3 + c];
                let v10 = src[(y0 * src_w + x1) * 3 + c];
                let v01 = src[(y1 * src_w + x0) * 3 + c];
                let v11 = src[(y1 * src_w + x1) * 3 + c];
                let v = v00 * (1.0 - fx) * (1.0 - fy)
                    + v10 * fx * (1.0 - fy)
                    + v01 * (1.0 - fx) * fy
                    + v11 * fx * fy;
                dst[(dy * dst_w + dx) * 3 + c] = v;
            }
        }
    }
    dst
}

/// 最近傍法によるマスクのリサイズ。
fn resize_mask_nearest(
    src: &[bool],
    src_w: usize,
    src_h: usize,
    dst_w: usize,
    dst_h: usize,
) -> Vec<bool> {
    let mut dst = vec![false; dst_w * dst_h];
    let x_ratio = src_w as f32 / dst_w as f32;
    let y_ratio = src_h as f32 / dst_h as f32;

    for dy in 0..dst_h {
        let sy = (dy as f32 * y_ratio) as usize;
        let sy = sy.min(src_h.saturating_sub(1));
        for dx in 0..dst_w {
            let sx = (dx as f32 * x_ratio) as usize;
            let sx = sx.min(src_w.saturating_sub(1));
            dst[dy * dst_w + dx] = src[sy * src_w + sx];
        }
    }
    dst
}

/// フォールバック: 簡易拡散ベース inpainting。
/// 境界から内側に向かって反復的に近傍平均を伝播させる。
fn inpaint_diffuse(
    original: &egui::ColorImage,
    mask: &[bool],
    w: usize,
    h: usize,
) -> egui::ColorImage {
    let mut pixels: Vec<[f32; 4]> = original.pixels.iter()
        .map(|c| [c.r() as f32, c.g() as f32, c.b() as f32, c.a() as f32])
        .collect();

    let mut filled = vec![false; w * h];
    for i in 0..mask.len() {
        filled[i] = !mask[i];
    }

    let max_iters = (w.max(h) as u32).min(2000);
    let neighbors: [(isize, isize); 4] = [(-1, 0), (1, 0), (0, -1), (0, 1)];

    for _iter in 0..max_iters {
        let mut any_filled = false;
        let mut new_pixels = pixels.clone();
        let mut new_filled = filled.clone();

        for y in 0..h {
            for x in 0..w {
                let idx = y * w + x;
                if filled[idx] { continue; }

                let mut sum = [0.0f32; 4];
                let mut count = 0u32;

                for (dx, dy) in &neighbors {
                    let nx = x as isize + dx;
                    let ny = y as isize + dy;
                    if nx >= 0 && ny >= 0 && (nx as usize) < w && (ny as usize) < h {
                        let ni = ny as usize * w + nx as usize;
                        if filled[ni] {
                            let p = pixels[ni];
                            sum[0] += p[0];
                            sum[1] += p[1];
                            sum[2] += p[2];
                            sum[3] += p[3];
                            count += 1;
                        }
                    }
                }

                if count > 0 {
                    new_pixels[idx] = [
                        sum[0] / count as f32,
                        sum[1] / count as f32,
                        sum[2] / count as f32,
                        sum[3] / count as f32,
                    ];
                    new_filled[idx] = true;
                    any_filled = true;
                }
            }
        }

        pixels = new_pixels;
        filled = new_filled;
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
