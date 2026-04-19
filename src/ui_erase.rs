//! 消しゴム (Erase) モード: フルスクリーン画像の任意領域をマスクし、
//! MI-GAN で補完 (inpaint) する。
//!
//! ツール: 囲み (Lasso), 縦線, 横線, 筆 (Brush)
//! モード: 描画 / 消去 の切り替え
//! マスクは SQLite (mask_db) に永続化される。

use eframe::egui;
use std::sync::Arc;

use crate::app::{App, EraseSnapshot, EraseTool, EraseVectorDrag, ShiftDragState};
use crate::fs_animation::FsCacheEntry;
use crate::mask_db::{LineKind, LineObject};
use crate::ui_fullscreen::FsKeyAction;

/// ベクタオブジェクトの端点ヒット判定半径 (画像ピクセル)。
const ENDPOINT_HIT_RADIUS: f32 = 12.0;
/// 矢印キー 1 回あたりの移動量 (ピクセル)。
const NUDGE_PIXELS: f32 = 1.0;
/// Ctrl+矢印の移動量 (ピクセル)。
const NUDGE_PIXELS_FAST: f32 = 10.0;
/// `[` / `]` キー 1 回あたりの回転量 (度)。
const ROTATE_DEG_STEP: f32 = 0.1;
/// Ctrl+`[` / `]` の回転量 (度)。
const ROTATE_DEG_STEP_FAST: f32 = 1.0;
/// Ctrl+ドラッグ: 縦方向 1px あたりの回転角 (度)。
const ROTATE_DEG_PER_PIXEL: f32 = 0.2;

/// MI-GAN の固定入力サイズ。
const MIGAN_SIZE: usize = 512;

/// ツールパネルの幅。
const PANEL_W: f32 = 190.0;
/// ツールパネルの左上マージン。
const PANEL_MARGIN_X: f32 = 16.0;
const PANEL_MARGIN_Y: f32 = 60.0;

/// Undo スタックの最大エントリ数。
const UNDO_MAX: usize = 20;

impl App {
    // ── モード開始/終了 ─────────────────────────────────────────────

    /// 消しゴムモードに入る。DB にマスクがあればロードする。
    pub(crate) fn enter_erase_mode(&mut self, fs_idx: usize) {
        // 元画像を取得: erase_base_cache (inpaint前の元画像) を優先、なければキャッシュから
        let pixels = if let Some(base) = self.erase_base_cache.get(&fs_idx) {
            Arc::clone(base)
        } else {
            let bg = self.effective_upscale_bg_mode();
            let from_cache = self.ai_upscale_cache.get(&(fs_idx, bg))
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
        // post-filter (CRT / 減色など) を編集中だけ一時バイパス。マスクは元画像ベースで
        // 塗るため、減色プリセットのドット表示が混ざると精密な境界操作が難しくなる。
        if !self.post_filter_bypassed {
            self.post_filter_bypassed = true;
            self.clear_adjustment_caches(fs_idx);
        }
        self.erase_mask_size = [w, h];
        self.erase_mask_texture = None;
        self.erase_last_paint_pos = None;


        self.erase_lasso_points.clear();
        self.erase_line_start = None;
        self.erase_line_end = None;
        self.erase_line_tilt = 0.0;
        self.erase_shift_drag = None;
        self.erase_paint_mode = true;
        self.erase_undo_stack.clear();
        self.erase_last_undo_at = None;
        self.erase_vectors.clear();
        self.erase_selected_vector = None;
        self.erase_vector_drag = None;

        // デフォルトブラシ半径: 長辺の 1/100
        if self.erase_brush_radius <= 0.0 {
            self.erase_brush_radius = (w.max(h) as f32 / 100.0).max(2.0);
        }
        // デフォルト直線幅: 長辺の 1/500 (細い線ノイズ除去に適した値)
        if self.erase_line_width <= 0.0 {
            self.erase_line_width = (w.max(h) as f32 / 500.0).max(2.0);
        }

        // DB からマスク (ビットマップ + ベクタ) をロード
        let (loaded_mask, loaded_vectors) = self.page_path_key(fs_idx)
            .and_then(|key| self.mask_db.as_ref()?.get_full(&key, w, h))
            .unwrap_or_else(|| (vec![false; w * h], Vec::new()));

        self.erase_mask = Some(loaded_mask);
        self.erase_vectors = loaded_vectors;
        crate::logger::log(format!(
            "erase: enter mode, image={w}x{h}, vectors={}",
            self.erase_vectors.len()
        ));
    }

    /// 消しゴムモードをリセットする。
    pub(crate) fn reset_erase_mode(&mut self) {
        let restore_idx = self.fullscreen_idx;
        self.erase_mode = false;
        // post-filter バイパスを解除し、該当ページの adjustment_cache をクリアして
        // post-filter 適用状態で再生成させる。分析モード中に誤って reset されても
        // analysis_mode が true なら post_filter_bypassed は分析モード側で保持される想定。
        if self.post_filter_bypassed && !self.analysis_mode {
            self.post_filter_bypassed = false;
            if let Some(idx) = restore_idx {
                self.clear_adjustment_caches(idx);
            }
        }
        self.erase_mask = None;
        self.erase_mask_size = [0, 0];
        self.erase_mask_texture = None;
        self.erase_last_paint_pos = None;


        self.erase_lasso_points.clear();
        self.erase_line_start = None;
        self.erase_line_end = None;
        self.erase_line_tilt = 0.0;
        self.erase_shift_drag = None;
        self.erase_undo_stack.clear();
        self.erase_last_undo_at = None;
        self.erase_vectors.clear();
        self.erase_selected_vector = None;
        self.erase_vector_drag = None;
    }

    // ── Undo / Slot ────────────────────────────────────────────────

    pub(crate) fn push_undo_snapshot(&mut self) {
        if let Some(mask) = &self.erase_mask {
            self.erase_undo_stack.push_back(EraseSnapshot {
                mask: mask.clone(),
                vectors: self.erase_vectors.clone(),
            });
            while self.erase_undo_stack.len() > UNDO_MAX {
                self.erase_undo_stack.pop_front();
            }
            self.erase_last_undo_at = Some(std::time::Instant::now());
        }
    }

    /// キーリピート連打中にスナップショットを毎フレーム取らないための版。
    /// 直前の push から閾値以内なら何もしない。
    fn push_undo_snapshot_throttled(&mut self) {
        const COALESCE_MS: u128 = 500;
        if let Some(last) = self.erase_last_undo_at {
            if last.elapsed().as_millis() < COALESCE_MS {
                return;
            }
        }
        self.push_undo_snapshot();
    }

    pub(crate) fn undo_erase(&mut self) -> bool {
        if let Some(prev) = self.erase_undo_stack.pop_back() {
            self.erase_mask = Some(prev.mask);
            self.erase_vectors = prev.vectors;
            self.erase_selected_vector = None;
            self.erase_vector_drag = None;
            self.erase_mask_texture = None;
            true
        } else {
            false
        }
    }

    /// 現在のマスク (ビットマップ + ベクタ) をスロットに保存する。
    pub(crate) fn save_mask_to_slot(&mut self, slot: usize) {
        let [w, h] = self.erase_mask_size;
        let saved = if let (Some(mask), Some(db)) = (&self.erase_mask, &self.mask_db) {
            db.set_slot(slot, mask, &self.erase_vectors, w, h).is_ok()
        } else {
            false
        };
        if saved {
            self.show_feedback_toast(format!("[スロット{}に保存]", slot));
        } else {
            self.show_feedback_toast(format!("[スロット{}保存失敗]", slot));
        }
    }

    /// スロットからマスクをロードし、現在のマスクを**差し替える**。
    /// 偶数/奇数ページを取り違えたときに旧マスクが残ると過剰マスクになるため、
    /// 追記ではなく上書き仕様。直前の状態は Ctrl+Z で戻せる。
    pub(crate) fn load_mask_from_slot(&mut self, slot: usize) {
        let [w, h] = self.erase_mask_size;
        let slot_data = self.mask_db.as_ref().and_then(|db| db.get_slot_full(slot, w, h));
        let Some((slot_mask, slot_vectors)) = slot_data else {
            self.show_feedback_toast(format!("[スロット{}は空です]", slot));
            return;
        };
        if !slot_mask.iter().any(|&m| m) && slot_vectors.is_empty() {
            self.show_feedback_toast(format!("[スロット{}は空です]", slot));
            return;
        }
        self.push_undo_snapshot();
        self.erase_mask = Some(slot_mask);
        self.erase_vectors = slot_vectors;
        self.erase_selected_vector = None;
        self.erase_mask_texture = None;
        self.show_feedback_toast(format!("[スロット{}をロード]", slot));
    }

    // ── キー入力 ──────────────────────────────────────────────────

    /// 消しゴムモード中のキー入力を処理する。
    /// 通常のフルスクリーンショートカットをブロックし、消しゴム専用キーのみ有効にする。
    pub(crate) fn handle_erase_keys(&mut self, ctx: &egui::Context, fs_idx: usize) -> FsKeyAction {
        let action = FsKeyAction { close: false, nav_delta: 0, ctrl_nav: None, jump_to: None };

        // ESC: 選択があればまず解除、無ければ消しゴムモード終了
        let esc = ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Escape));
        if esc {
            if self.erase_selected_vector.is_some() {
                self.erase_selected_vector = None;
                self.erase_vector_drag = None;
                self.erase_mask_texture = None;
                return action;
            }
            // 終了前にマスクを DB + サイドカーに保存
            let [w, h] = self.erase_mask_size;
            if let Some(mask) = self.erase_mask.clone() {
                let vectors = self.erase_vectors.clone();
                self.save_mask_with_sidecar(fs_idx, &mask, &vectors, w, h);
            }
            self.reset_erase_mode();
            return action;
        }

        // E: inpaint 実行
        let key_e = ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::E));
        if key_e {
            self.execute_erase_inpaint(ctx, fs_idx);
            return action;
        }

        // Ctrl+Z: Undo
        let ctrl_z = ctx.input_mut(|i| i.consume_key(egui::Modifiers::CTRL, egui::Key::Z));
        if ctrl_z {
            if self.undo_erase() {
                self.show_feedback_toast("[元に戻す]".to_string());
            } else {
                self.show_feedback_toast("[履歴なし]".to_string());
            }
        }

        // Delete: 選択中のベクタオブジェクトを削除
        let key_del = ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Delete));
        if key_del {
            if let Some(idx) = self.erase_selected_vector {
                if idx < self.erase_vectors.len() {
                    self.push_undo_snapshot();
                    self.erase_vectors.remove(idx);
                    self.erase_selected_vector = None;
                    self.erase_vector_drag = None;
                    self.erase_mask_texture = None;
                    self.show_feedback_toast("[ベクタ削除]".to_string());
                }
            }
        }

        // Ctrl で 10 倍 (平行移動/回転とも同じ修飾キーに揃える)。
        // Shift を使わない理由: 回転の [/] は Shift+ で論理キーが {/} に化けて
        // Key::OpenBracket/CloseBracket にマッチしないため Ctrl にせざるを得ない。
        // 揃えないと覚えにくいので矢印キーもあわせて Ctrl に統一。
        let ctrl_held = ctx.input(|i| i.modifiers.ctrl);

        // 矢印キー: 平行移動 (Ctrl で 10px)
        let step = if ctrl_held { NUDGE_PIXELS_FAST } else { NUDGE_PIXELS };
        let (mut dx, mut dy) = (0.0f32, 0.0f32);
        ctx.input_mut(|i| {
            if i.consume_key(egui::Modifiers::NONE, egui::Key::ArrowLeft)
                || i.consume_key(egui::Modifiers::CTRL, egui::Key::ArrowLeft) { dx -= step; }
            if i.consume_key(egui::Modifiers::NONE, egui::Key::ArrowRight)
                || i.consume_key(egui::Modifiers::CTRL, egui::Key::ArrowRight) { dx += step; }
            if i.consume_key(egui::Modifiers::NONE, egui::Key::ArrowUp)
                || i.consume_key(egui::Modifiers::CTRL, egui::Key::ArrowUp) { dy -= step; }
            if i.consume_key(egui::Modifiers::NONE, egui::Key::ArrowDown)
                || i.consume_key(egui::Modifiers::CTRL, egui::Key::ArrowDown) { dy += step; }
        });
        if dx != 0.0 || dy != 0.0 {
            self.nudge_mask(dx, dy);
        }

        // [ / ]: 回転 (Ctrl で 1°)
        let rot_step = if ctrl_held { ROTATE_DEG_STEP_FAST } else { ROTATE_DEG_STEP };
        let mut rot_deg = 0.0f32;
        ctx.input_mut(|i| {
            if i.consume_key(egui::Modifiers::NONE, egui::Key::OpenBracket)
                || i.consume_key(egui::Modifiers::CTRL, egui::Key::OpenBracket) { rot_deg -= rot_step; }
            if i.consume_key(egui::Modifiers::NONE, egui::Key::CloseBracket)
                || i.consume_key(egui::Modifiers::CTRL, egui::Key::CloseBracket) { rot_deg += rot_step; }
        });
        if rot_deg != 0.0 {
            self.rotate_mask(rot_deg.to_radians());
        }

        // S/B/L/V/H/I: ツール切替
        let key_s_tool = ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::S));
        let key_b = ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::B));
        let key_l = ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::L));
        let key_v = ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::V));
        let key_h = ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::H));
        let key_i = ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::I));
        if key_s_tool {
            self.erase_tool = EraseTool::Select;
            self.show_feedback_toast("[選択]".to_string());
        }
        if key_b {
            self.erase_tool = EraseTool::Brush;
            self.show_feedback_toast("[筆]".to_string());
        }
        if key_l {
            self.erase_tool = EraseTool::Lasso;
            self.show_feedback_toast("[囲み]".to_string());
        }
        if key_v {
            self.erase_tool = EraseTool::VertLine;
            self.show_feedback_toast("[縦線]".to_string());
        }
        if key_h {
            self.erase_tool = EraseTool::HorizLine;
            self.show_feedback_toast("[横線]".to_string());
        }
        if key_i {
            self.erase_tool = EraseTool::Line;
            self.show_feedback_toast("[直線]".to_string());
        }

        // D: 描画モード, F: 消去モード
        let key_d = ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::D));
        let key_f = ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::F));
        if key_d {
            self.erase_paint_mode = true;
            self.show_feedback_toast("[描画モード]".to_string());
        }
        if key_f {
            self.erase_paint_mode = false;
            self.show_feedback_toast("[消去モード]".to_string());
        }

        // erase_mode 中は通常のフルスクリーンショートカットを無効化するため、
        // ここで未使用キーを明示的に消費する (マウスイベントはペイントに必要なため除外)。
        // 矢印キー / [/] は上で既に consume 済み。
        const SINGLE_KEYS: &[egui::Key] = &[
            egui::Key::Space, egui::Key::Tab,
            egui::Key::I, egui::Key::R, egui::Key::Z,
            egui::Key::G, egui::Key::M, egui::Key::P,
            egui::Key::U, egui::Key::N,
            egui::Key::F1, egui::Key::F2, egui::Key::F3,
            egui::Key::F4, egui::Key::F5, egui::Key::F6,
        ];
        // 数字キーは全て消費 (スロット系ショートカットは廃止、誤動作を防ぐ)
        const NUM_KEYS: &[egui::Key] = &[
            egui::Key::Num0, egui::Key::Num1, egui::Key::Num2,
            egui::Key::Num3, egui::Key::Num4, egui::Key::Num5,
            egui::Key::Num6, egui::Key::Num7, egui::Key::Num8,
            egui::Key::Num9,
        ];
        ctx.input_mut(|i| {
            for &k in SINGLE_KEYS {
                let _ = i.consume_key(egui::Modifiers::NONE, k);
            }
            for &k in NUM_KEYS {
                let _ = i.consume_key(egui::Modifiers::NONE, k);
                let _ = i.consume_key(egui::Modifiers::SHIFT, k);
                let _ = i.consume_key(egui::Modifiers::CTRL, k);
                let _ = i.consume_key(egui::Modifiers::CTRL | egui::Modifiers::SHIFT, k);
            }
        });

        action
    }

    // ── 全体/個別の平行移動・回転 ─────────────────────────────────────

    /// マスクをシフトする。選択中ベクタがあればそれだけ、無ければビットマップとすべての
    /// ベクタを移動する。
    fn nudge_mask(&mut self, dx: f32, dy: f32) {
        self.push_undo_snapshot_throttled();
        match self.erase_selected_vector {
            Some(idx) if idx < self.erase_vectors.len() => {
                self.erase_vectors[idx].translate(dx, dy);
            }
            _ => {
                // 全ベクタを移動
                for v in &mut self.erase_vectors {
                    v.translate(dx, dy);
                }
                // ビットマップもシフト
                let [w, h] = self.erase_mask_size;
                if let Some(mask) = self.erase_mask.as_mut() {
                    shift_bitmap(mask, w, h, dx, dy);
                }
            }
        }
        self.erase_mask_texture = None;
    }

    /// マスクを回転する。選択中ベクタがあればそれだけ、無ければ全体を画像中心周りに回転する。
    fn rotate_mask(&mut self, angle_rad: f32) {
        self.push_undo_snapshot_throttled();
        match self.erase_selected_vector {
            Some(idx) if idx < self.erase_vectors.len() => {
                let center = self.erase_vectors[idx].center();
                self.erase_vectors[idx].rotate_around(center.0, center.1, angle_rad);
            }
            _ => {
                let [w, h] = self.erase_mask_size;
                let cx = w as f32 * 0.5;
                let cy = h as f32 * 0.5;
                for v in &mut self.erase_vectors {
                    v.rotate_around(cx, cy, angle_rad);
                }
                if let Some(mask) = self.erase_mask.as_mut() {
                    rotate_bitmap(mask, w, h, cx, cy, angle_rad);
                }
            }
        }
        self.erase_mask_texture = None;
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

    /// 多角形の内部をビットマップに塗る/消す。`mask_db::scanline_fill_polygon` の薄いラッパ。
    fn paint_polygon(&mut self, points: &[(f32, f32)], paint: bool) {
        let [w, h] = self.erase_mask_size;
        let Some(mask) = self.erase_mask.as_mut() else { return; };
        crate::mask_db::scanline_fill_polygon(mask, points, w, h, paint);
        self.erase_mask_texture = None;
    }

    // ── マスクテクスチャ ──────────────────────────────────────────

    fn ensure_mask_texture(&mut self, ctx: &egui::Context) {
        if self.erase_mask_texture.is_some() { return; }
        let Some(composite) = self.composite_mask() else { return; };
        let [w, h] = self.erase_mask_size;
        let mut rgba = vec![0u8; w * h * 4];
        for i in 0..composite.len() {
            if composite[i] {
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

    /// ビットマップとベクタ群を合成した最終マスクを返す。
    /// 表示・inpaint・保存の「真のマスク」はすべてこの合成結果を使う。
    pub(crate) fn composite_mask(&self) -> Option<Vec<bool>> {
        let mask = self.erase_mask.as_ref()?;
        let [w, h] = self.erase_mask_size;
        if w == 0 || h == 0 { return None; }
        let mut out = mask.clone();
        crate::mask_db::rasterize_vectors_into(&mut out, &self.erase_vectors, w, h);
        Some(out)
    }

    // ── ベクタオブジェクトのヒットテスト・ドラッグ編集 ──────────────

    /// 画像座標 `pos` がいずれかのベクタに当たるかを調べる。
    /// 返り値: `(index, Some(which_p1))` — 端点ヒット時は which_p1 が true=p1 / false=p0。
    /// `(index, None)` は本体 (矩形内部) ヒット。ヒットなしは None。
    fn hit_test_vector(&self, pos: (f32, f32)) -> Option<(usize, Option<bool>)> {
        // 先に端点判定を全ベクタで行う (Diagonal のみ対象、縦横線は端点操作の意味が薄い)
        for (i, v) in self.erase_vectors.iter().enumerate().rev() {
            if v.kind == LineKind::Diagonal {
                let d0 = dist_sq(pos, v.p0);
                let d1 = dist_sq(pos, v.p1);
                let r2 = ENDPOINT_HIT_RADIUS * ENDPOINT_HIT_RADIUS;
                if d0 <= r2 && d0 <= d1 {
                    return Some((i, Some(false)));
                }
                if d1 <= r2 {
                    return Some((i, Some(true)));
                }
            }
        }
        // 本体 (矩形) 判定。上に重なっている新しい方を優先するため逆順走査。
        for (i, v) in self.erase_vectors.iter().enumerate().rev() {
            let corners = v.corners(2.0); // 少し余裕を持たせる
            if point_in_polygon(pos, &corners) {
                return Some((i, None));
            }
        }
        None
    }

    /// ベクタ編集ドラッグ状態に応じてオブジェクトを更新する。
    fn update_vector_drag(
        &mut self,
        pointer_pos: Option<egui::Pos2>,
        full_rect: egui::Rect,
        zoom_pan: Option<(f32, egui::Vec2)>,
        ctrl_held: bool,
    ) {
        let Some(screen) = pointer_pos else { return; };
        let Some((total_scale, img_rect)) = self.erase_image_layout(full_rect, zoom_pan) else { return; };
        // screen_to_image を範囲外でも通るように自前で計算
        let cur = (
            (screen.x - img_rect.min.x) / total_scale,
            (screen.y - img_rect.min.y) / total_scale,
        );

        let Some(drag) = self.erase_vector_drag else { return; };

        // Ctrl キーが途中で切り替わったらモード遷移 (Pan ⇄ ModAdjust)
        let new_drag = match drag {
            EraseVectorDrag::Pan { index, base, origin } if ctrl_held => {
                EraseVectorDrag::ModAdjust { index, base, origin }
            }
            EraseVectorDrag::ModAdjust { index, base, origin } if !ctrl_held => {
                EraseVectorDrag::Pan { index, base, origin }
            }
            _ => drag,
        };
        self.erase_vector_drag = Some(new_drag);

        let idx = match new_drag {
            EraseVectorDrag::Pan { index, .. }
            | EraseVectorDrag::ModAdjust { index, .. }
            | EraseVectorDrag::Endpoint { index, .. } => index,
        };
        if idx >= self.erase_vectors.len() { return; }

        match new_drag {
            EraseVectorDrag::Pan { base, origin, .. } => {
                let dx = cur.0 - origin.0;
                let dy = cur.1 - origin.1;
                let mut v = base;
                v.translate(dx, dy);
                self.erase_vectors[idx] = v;
            }
            EraseVectorDrag::ModAdjust { base, origin, .. } => {
                let dx = cur.0 - origin.0;
                let dy = cur.1 - origin.1;
                // 縦方向: 回転、横方向: 太さ
                let angle = (dy * ROTATE_DEG_PER_PIXEL).to_radians();
                let c = base.center();
                let mut v = base;
                v.rotate_around(c.0, c.1, angle);
                let max_t = self.erase_mask_size[0].max(self.erase_mask_size[1]) as f32 * 0.5;
                v.thickness = (base.thickness + dx * 2.0).clamp(1.0, max_t);
                self.erase_vectors[idx] = v;
            }
            EraseVectorDrag::Endpoint { base, which_p1, .. } => {
                let mut v = base;
                if which_p1 {
                    v.p1 = cur;
                } else {
                    v.p0 = cur;
                }
                self.erase_vectors[idx] = v;
            }
        }
        self.erase_mask_texture = None;
    }

    /// ツールパネルの矩形を返す。
    fn erase_panel_rect(&self, full_rect: egui::Rect) -> egui::Rect {
        let panel_pos = egui::pos2(full_rect.min.x + PANEL_MARGIN_X, full_rect.min.y + PANEL_MARGIN_Y);
        // 基本高さ: ヘッダ + 描画/消去 + セパレータ + ツール 3 行 + スロット + 下部ショートカット説明 + セパレータ + マスク全削除 + ヘルプ (6行)
        let base_h = 430.0;
        let extra = if self.erase_tool == EraseTool::Brush || self.erase_tool == EraseTool::Line {
            42.0 // サイズスライダー分
        } else {
            0.0
        };
        egui::Rect::from_min_size(panel_pos, egui::vec2(PANEL_W, base_h + extra))
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
        let primary_pressed = ctx.input(|i| i.pointer.primary_pressed());
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

        // 修飾キーは Ctrl で統一: [/] キーは Shift+ が論理キー {/} に化ける制約があり
        // 回転系を Ctrl にしたため、パン・ツール時のフィット調整もすべて Ctrl に揃える。
        let ctrl_held = ctx.input(|i| i.modifiers.ctrl);

        // ── ベクタオブジェクト編集パス (選択ツール時のみ) ───────────
        // 選択ツール中はドロー系の操作を行わず、クリック=選択/ドラッグ=編集 に徹する。
        // 他ツールの描画中に近接したベクタを誤選択しないよう、意図的にモーダルにしている。
        if self.erase_tool == EraseTool::Select {
            if primary_pressed {
                if let Some(img_pos) = pointer_pos
                    .and_then(|p| self.screen_to_image_f32(p, full_rect, zoom_pan))
                {
                    if let Some((hit_idx, hit_endpoint)) = self.hit_test_vector(img_pos) {
                        self.push_undo_snapshot();
                        self.erase_selected_vector = Some(hit_idx);
                        let base = self.erase_vectors[hit_idx];
                        self.erase_vector_drag = Some(if let Some(which_p1) = hit_endpoint {
                            EraseVectorDrag::Endpoint { index: hit_idx, base, which_p1 }
                        } else if ctrl_held {
                            EraseVectorDrag::ModAdjust { index: hit_idx, base, origin: img_pos }
                        } else {
                            EraseVectorDrag::Pan { index: hit_idx, base, origin: img_pos }
                        });
                        self.erase_mask_texture = None;
                    } else {
                        // 空領域のクリック: 選択を解除
                        if self.erase_selected_vector.is_some() {
                            self.erase_selected_vector = None;
                            self.erase_mask_texture = None;
                        }
                    }
                }
            }
            if self.erase_vector_drag.is_some() {
                self.update_vector_drag(pointer_pos, full_rect, zoom_pan, ctrl_held);
                if primary_released {
                    self.erase_vector_drag = None;
                }
            }
            return;
        }

        // 他ツールに切り替わったら選択を自動解除 (ドラッグ中は最後まで完走)
        if self.erase_vector_drag.is_none() && self.erase_selected_vector.is_some() {
            self.erase_selected_vector = None;
            self.erase_mask_texture = None;
        }

        // マウスホイールによる筆/直線の太さ調整は handle_fs_wheel_and_click で処理済み。

        match self.erase_tool {
            EraseTool::Select => {
                // Select は上で処理済み。到達しないはず。
            }
            EraseTool::Brush => {
                if primary_down {
                    if let Some(pos) = pointer_pos {
                        if let Some(img_pos) = self.screen_to_image_f32(pos, full_rect, zoom_pan) {
                            if ctrl_held {
                                // 右/下方向で拡大、左/上方向で縮小
                                let base_radius = match self.erase_shift_drag {
                                    Some(ShiftDragState::BrushSize { base_radius, .. }) => base_radius,
                                    _ => {
                                        self.erase_shift_drag = Some(ShiftDragState::BrushSize {
                                            origin: img_pos,
                                            base_radius: self.erase_brush_radius,
                                        });
                                        self.erase_brush_radius
                                    }
                                };
                                if let Some(ShiftDragState::BrushSize { origin, .. }) = self.erase_shift_drag {
                                    let delta = (img_pos.0 - origin.0) + (img_pos.1 - origin.1);
                                    let max_r = self.erase_mask_size[0].max(self.erase_mask_size[1]) as f32 / 20.0;
                                    self.erase_brush_radius = (base_radius + delta).clamp(1.0, max_r);
                                }
                            } else {
                                self.erase_shift_drag = None;
                                if self.erase_last_paint_pos.is_none() {
                                    self.push_undo_snapshot();
                                }
                                let prev = self.erase_last_paint_pos
                                    .and_then(|p| self.screen_to_image_f32(p, full_rect, zoom_pan))
                                    .unwrap_or(img_pos);
                                self.paint_brush_line(prev, img_pos, paint);
                            }
                        }
                        self.erase_last_paint_pos = Some(pos);
                    }
                } else {
                    self.erase_last_paint_pos = None;
                    self.erase_shift_drag = None;
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
                    self.push_undo_snapshot();
                    let pts: Vec<(f32, f32)> = self.erase_lasso_points.drain(..).collect();
                    self.paint_polygon(&pts, paint);
                } else if primary_released {
                    self.erase_lasso_points.clear();
                }
            }
            EraseTool::VertLine => {
                self.handle_line_tool_paint(
                    primary_down, primary_released, pointer_pos, ctrl_held, paint,
                    full_rect, zoom_pan, true,
                );
            }
            EraseTool::HorizLine => {
                self.handle_line_tool_paint(
                    primary_down, primary_released, pointer_pos, ctrl_held, paint,
                    full_rect, zoom_pan, false,
                );
            }
            EraseTool::Line => {
                if primary_down {
                    if let Some(pos) = pointer_pos {
                        if let Some(img_pos) = self.screen_to_image_f32(pos, full_rect, zoom_pan) {
                            if self.erase_line_start.is_none() {
                                self.erase_line_start = Some(img_pos);
                            }
                            if ctrl_held {
                                // Ctrl+ドラッグ: カーソルから線への垂直距離で線幅を変更
                                // 線 (erase_line_start → erase_line_end) は Ctrl 開始直前に確定済み
                                if let (Some(start), Some(end)) = (self.erase_line_start, self.erase_line_end) {
                                    let dx = end.0 - start.0;
                                    let dy = end.1 - start.1;
                                    let len = (dx * dx + dy * dy).sqrt().max(1.0);
                                    let vx = img_pos.0 - start.0;
                                    let vy = img_pos.1 - start.1;
                                    // 線の法線方向成分 (符号付き) の絶対値
                                    let perp = (vx * dy - vy * dx).abs() / len;
                                    self.erase_line_width = (perp * 2.0).max(1.0);
                                }
                            } else {
                                self.erase_line_end = Some(img_pos);
                            }
                        }
                    }
                }
                if primary_released {
                    if let (Some((x0, y0)), Some((x1, y1))) = (self.erase_line_start, self.erase_line_end) {
                        let dx = x1 - x0;
                        let dy = y1 - y0;
                        let len = (dx * dx + dy * dy).sqrt();
                        if len > 1.0 {
                            self.push_undo_snapshot();
                            let obj = LineObject {
                                kind: LineKind::Diagonal,
                                p0: (x0, y0),
                                p1: (x1, y1),
                                thickness: self.erase_line_width.max(1.0),
                            };
                            self.commit_line_object(obj, paint);
                            self.erase_mask_texture = None;
                        }
                    }
                    self.erase_line_start = None;
                    self.erase_line_end = None;
                }
            }
        }
    }

    /// 縦線/横線ツール共通の入力処理。is_vertical=true で縦線、false で横線。
    /// Ctrl+ドラッグでは線の向きに沿った軸がパン、直交軸が回転になる。
    fn handle_line_tool_paint(
        &mut self,
        primary_down: bool,
        primary_released: bool,
        pointer_pos: Option<egui::Pos2>,
        ctrl_held: bool,
        paint: bool,
        full_rect: egui::Rect,
        zoom_pan: Option<(f32, egui::Vec2)>,
        is_vertical: bool,
    ) {
        if primary_down {
            if let Some(pos) = pointer_pos {
                if let Some(img_pos) = self.screen_to_image_f32(pos, full_rect, zoom_pan) {
                    if self.erase_line_start.is_none() {
                        self.erase_line_start = Some(img_pos);
                        self.erase_line_tilt = 0.0;
                    }
                    if ctrl_held {
                        let (base_tilt, base_start, base_end) = match self.erase_shift_drag {
                            Some(ShiftDragState::LineAdjust { base_tilt, base_start, base_end, .. }) => {
                                (base_tilt, base_start, base_end)
                            }
                            _ => {
                                let start = self.erase_line_start.unwrap_or(img_pos);
                                let end = self.erase_line_end.unwrap_or(img_pos);
                                self.erase_shift_drag = Some(ShiftDragState::LineAdjust {
                                    origin: img_pos,
                                    base_tilt: self.erase_line_tilt,
                                    base_start: start,
                                    base_end: end,
                                });
                                (self.erase_line_tilt, start, end)
                            }
                        };
                        if let Some(ShiftDragState::LineAdjust { origin, .. }) = self.erase_shift_drag {
                            let dx = img_pos.0 - origin.0;
                            let dy = img_pos.1 - origin.1;
                            // 縦線: 向きに沿う軸 (Y) に沿ったドラッグは幅を変えず、直交する X ドラッグでパン・Y ドラッグで回転
                            // 横線: X/Y が入れ替わる
                            let (pan_x, pan_y, tilt_delta) = if is_vertical {
                                (dx, 0.0, dy)
                            } else {
                                (0.0, dy, dx)
                            };
                            self.erase_line_start = Some((base_start.0 + pan_x, base_start.1 + pan_y));
                            self.erase_line_end = Some((base_end.0 + pan_x, base_end.1 + pan_y));
                            self.erase_line_tilt = base_tilt + tilt_delta;
                        }
                    } else {
                        self.erase_shift_drag = None;
                        self.erase_line_end = Some(img_pos);
                    }
                }
            }
        }
        if primary_released {
            if let (Some(start), Some(end)) = (self.erase_line_start, self.erase_line_end) {
                let [w, h] = self.erase_mask_size;
                let tilt = self.erase_line_tilt;
                self.push_undo_snapshot();
                if is_vertical {
                    let lx = start.0.min(end.0);
                    let rx = start.0.max(end.0);
                    let thickness = (rx - lx).max(1.0);
                    let cx = (lx + rx) * 0.5;
                    // 中心軸: 上端 (cx+tilt, 0) → 下端 (cx, h)
                    let obj = LineObject {
                        kind: LineKind::Vertical,
                        p0: (cx + tilt, 0.0),
                        p1: (cx, h as f32),
                        thickness,
                    };
                    self.commit_line_object(obj, paint);
                } else {
                    let ty = start.1.min(end.1);
                    let by = start.1.max(end.1);
                    let thickness = (by - ty).max(1.0);
                    let cy = (ty + by) * 0.5;
                    let obj = LineObject {
                        kind: LineKind::Horizontal,
                        p0: (0.0, cy),
                        p1: (w as f32, cy + tilt),
                        thickness,
                    };
                    self.commit_line_object(obj, paint);
                }
                self.erase_mask_texture = None;
            }
            self.erase_line_start = None;
            self.erase_line_end = None;
            self.erase_line_tilt = 0.0;
            self.erase_shift_drag = None;
        }
    }

    /// 描画モードならベクタオブジェクトを追加、消去モードなら重なるベクタを除去＋ビットマップも消す。
    fn commit_line_object(&mut self, obj: LineObject, paint: bool) {
        if paint {
            self.erase_vectors.push(obj);
        } else {
            let corners = obj.corners(0.0).to_vec();
            self.erase_vectors_overlapping_polygon(&corners);
            self.paint_polygon(&corners, false);
        }
    }

    /// 与えた多角形 (矩形想定) と重なるベクタを削除する (消去モード用)。
    fn erase_vectors_overlapping_polygon(&mut self, poly: &[(f32, f32)]) {
        if poly.is_empty() { return; }
        let (px_min, py_min, px_max, py_max) = bounding_box(poly);
        self.erase_vectors.retain(|v| {
            let c = v.corners(0.0);
            let (vxmin, vymin, vxmax, vymax) = bounding_box(&c);
            !(vxmax < px_min || vxmin > px_max || vymax < py_min || vymin > py_max)
        });
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
        // 選択中のベクタに黄色のアウトラインを重ねる
        if let Some(idx) = self.erase_selected_vector {
            if let Some(v) = self.erase_vectors.get(idx) {
                let pts_img = v.corners(0.0);
                let pts_screen: Vec<egui::Pos2> = pts_img.iter()
                    .map(|&(x, y)| self.image_to_screen(x, y, full_rect, zoom_pan))
                    .collect();
                let sel_stroke = egui::Stroke::new(2.0, egui::Color32::from_rgb(255, 220, 60));
                for i in 0..pts_screen.len() {
                    let a = pts_screen[i];
                    let b = pts_screen[(i + 1) % pts_screen.len()];
                    ui.painter().line_segment([a, b], sel_stroke);
                }
                // 直線は端点にハンドルを描く
                if v.kind == LineKind::Diagonal {
                    let handle = egui::Color32::from_rgb(255, 220, 60);
                    let p0s = self.image_to_screen(v.p0.0, v.p0.1, full_rect, zoom_pan);
                    let p1s = self.image_to_screen(v.p1.0, v.p1.1, full_rect, zoom_pan);
                    ui.painter().circle_filled(p0s, 4.0, handle);
                    ui.painter().circle_filled(p1s, 4.0, handle);
                }
            }
        }

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
                self.draw_line_tool_preview(ui, full_rect, zoom_pan, color, stroke_color, true);
            }
            EraseTool::HorizLine => {
                self.draw_line_tool_preview(ui, full_rect, zoom_pan, color, stroke_color, false);
            }
            EraseTool::Line => {
                if let (Some((x0, y0)), Some((x1, y1))) = (self.erase_line_start, self.erase_line_end) {
                    let dx = x1 - x0;
                    let dy = y1 - y0;
                    let len = (dx * dx + dy * dy).sqrt();
                    if len > 1.0 {
                        let nx = -dy / len;
                        let ny = dx / len;
                        let half_w = self.erase_line_width * 0.5;
                        let pts = vec![
                            self.image_to_screen(x0 + nx * half_w, y0 + ny * half_w, full_rect, zoom_pan),
                            self.image_to_screen(x1 + nx * half_w, y1 + ny * half_w, full_rect, zoom_pan),
                            self.image_to_screen(x1 - nx * half_w, y1 - ny * half_w, full_rect, zoom_pan),
                            self.image_to_screen(x0 - nx * half_w, y0 - ny * half_w, full_rect, zoom_pan),
                        ];
                        ui.painter().add(egui::Shape::convex_polygon(
                            pts,
                            color,
                            egui::Stroke::new(1.0, stroke_color),
                        ));
                        // 中心線も重ねて表示
                        let p0 = self.image_to_screen(x0, y0, full_rect, zoom_pan);
                        let p1 = self.image_to_screen(x1, y1, full_rect, zoom_pan);
                        ui.painter().line_segment(
                            [p0, p1],
                            egui::Stroke::new(1.0, stroke_color),
                        );
                    }
                }
            }
            _ => {}
        }
    }

    /// 縦線/横線ツール共通のプレビュー描画。
    fn draw_line_tool_preview(
        &self,
        ui: &mut egui::Ui,
        full_rect: egui::Rect,
        zoom_pan: Option<(f32, egui::Vec2)>,
        color: egui::Color32,
        stroke_color: egui::Color32,
        is_vertical: bool,
    ) {
        let (Some(start), Some(end)) = (self.erase_line_start, self.erase_line_end) else { return };
        let [w, h] = self.erase_mask_size;
        let tilt = self.erase_line_tilt;

        // 縦線: y を 0..h で固定し x を drag で決める。横線は X/Y 入れ替え。
        let (a0, a1, span_min, span_max) = if is_vertical {
            (start.0.min(end.0), start.0.max(end.0), 0.0f32, h as f32)
        } else {
            (start.1.min(end.1), start.1.max(end.1), 0.0f32, w as f32)
        };

        let corner = |axis: f32, span: f32, tilt_offset: f32| -> egui::Pos2 {
            if is_vertical {
                self.image_to_screen(axis + tilt_offset, span, full_rect, zoom_pan)
            } else {
                self.image_to_screen(span, axis + tilt_offset, full_rect, zoom_pan)
            }
        };

        if tilt.abs() < 0.5 {
            let p0 = corner(a0, span_min, 0.0);
            let p1 = corner(a1, span_max, 0.0);
            let rect = egui::Rect::from_min_max(p0.min(p1), p0.max(p1));
            ui.painter().rect_filled(rect, 0.0, color);
            ui.painter().rect_stroke(rect, 0.0, egui::Stroke::new(1.0, stroke_color), egui::StrokeKind::Outside);
        } else {
            // span_min 側は基準、span_max 側に tilt が加わる (is_vertical のとき上端→下端で x が tilt 分だけシフト)
            let pts = if is_vertical {
                vec![
                    corner(a0, span_min, tilt),
                    corner(a1, span_min, tilt),
                    corner(a1, span_max, 0.0),
                    corner(a0, span_max, 0.0),
                ]
            } else {
                vec![
                    corner(a0, span_min, 0.0),
                    corner(a0, span_max, tilt),
                    corner(a1, span_max, tilt),
                    corner(a1, span_min, 0.0),
                ]
            };
            ui.painter().add(egui::Shape::convex_polygon(
                pts, color, egui::Stroke::new(1.0, stroke_color),
            ));
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
                draw_dashed_circle(ui.painter(), pos, screen_r);
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
        let mode_labels = [("描画 [D]", true), ("消去 [F]", false)];
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

        // ── 区切り線 (描画/消去 と ツール選択を分ける) ──
        child.painter().line_segment(
            [egui::pos2(x0, y), egui::pos2(x0 + pw, y)],
            egui::Stroke::new(1.0, egui::Color32::from_gray(60)),
        );
        y += 8.0;

        // ── ツール選択 ──
        let tools = [
            ("選択 [S]", EraseTool::Select),
            ("筆 [B]", EraseTool::Brush),
            ("囲み [L]", EraseTool::Lasso),
            ("縦線 [V]", EraseTool::VertLine),
            ("横線 [H]", EraseTool::HorizLine),
            ("直線 [I]", EraseTool::Line),
        ];
        let tool_w = (pw - 8.0) / 2.0;
        let mut rows_used = 0usize;
        for (i, &(label, tool)) in tools.iter().enumerate() {
            let col = i % 2;
            let row = i / 2;
            rows_used = row + 1;
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
        y += rows_used as f32 * 28.0 + 4.0;

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

        // ── 直線幅スライダー (直線ツール時のみ) ──
        if self.erase_tool == EraseTool::Line {
            child.painter().text(
                egui::pos2(x0, y),
                egui::Align2::LEFT_TOP,
                "幅",
                egui::FontId::proportional(11.0),
                egui::Color32::from_gray(180),
            );
            y += 16.0;
            let slider_rect = egui::Rect::from_min_size(
                egui::pos2(x0, y),
                egui::vec2(pw, 20.0),
            );
            let mut slider_child = child.new_child(egui::UiBuilder::new().max_rect(slider_rect));
            let max_w = self.erase_mask_size[0].max(self.erase_mask_size[1]) as f32 / 20.0;
            slider_child.add(
                egui::Slider::new(&mut self.erase_line_width, 1.0..=max_w)
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

        // ── スロット保存/ロード (1/2) ──
        child.painter().text(
            egui::pos2(x0, y),
            egui::Align2::LEFT_TOP,
            "マスクスロット",
            egui::FontId::proportional(11.0),
            egui::Color32::from_gray(180),
        );
        y += 15.0;
        // 2 行 × 2 列: 上段 [保存1][保存2]、下段 [ロード1][ロード2]
        // ショートカット表記はボタンに載せない (F7/F8 はフルスクリーン全体ショートカット、
        // 消しゴムモード内ショートカットは廃止)。
        let slot_w = (pw - 4.0) / 2.0;
        for (row, action_label) in ["保存", "ロード"].iter().enumerate() {
            for slot in 1..=2u32 {
                let btn_rect = egui::Rect::from_min_size(
                    egui::pos2(x0 + (slot as f32 - 1.0) * (slot_w + 4.0), y + row as f32 * 24.0),
                    egui::vec2(slot_w, 20.0),
                );
                let resp = child.allocate_rect(btn_rect, egui::Sense::click());
                let bg = if resp.hovered() {
                    egui::Color32::from_gray(70)
                } else {
                    egui::Color32::from_gray(50)
                };
                child.painter().rect_filled(btn_rect, 3.0, bg);
                let label = format!("{}{}", action_label, slot);
                child.painter().text(
                    btn_rect.center(), egui::Align2::CENTER_CENTER,
                    &label, egui::FontId::proportional(10.0), egui::Color32::WHITE,
                );
                if resp.clicked() {
                    if row == 0 {
                        self.save_mask_to_slot(slot as usize);
                    } else {
                        self.load_mask_from_slot(slot as usize);
                    }
                }
            }
        }
        y += 52.0;

        // ── ショートカット説明 (スロットに対する全体ショートカットを文章で) ──
        child.painter().text(
            egui::pos2(x0, y),
            egui::Align2::LEFT_TOP,
            "フルスクリーン中 F7/F8 で\n保存1/2 を即適用",
            egui::FontId::proportional(10.0),
            egui::Color32::from_gray(150),
        );
        y += 26.0;

        // ── セパレーター (2) ──
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
            self.push_undo_snapshot();
            self.erase_mask = Some(vec![false; w * h]);
            self.erase_vectors.clear();
            self.erase_selected_vector = None;
            self.erase_vector_drag = None;
            self.erase_mask_texture = None;
            // 編集中の一時状態も破棄しないと、ドラッグ途中に全削除を押したあと
            // 次の release/click で描きかけの囲み・直線・シフト分だけの差分が
            // 復活してしまう。reset_erase_mode() と同じ範囲をクリアするが、
            // erase_mode 自体は維持してその場で編集を継続できるようにする。
            self.erase_last_paint_pos = None;
            self.erase_lasso_points.clear();
            self.erase_line_start = None;
            self.erase_line_end = None;
            self.erase_line_tilt = 0.0;
            self.erase_shift_drag = None;
            if let Some(fs_idx) = self.fullscreen_idx {
                // DB + サイドカーからも削除
                self.delete_mask_with_sidecar(fs_idx);
                // 表示を元画像に戻す
                if let Some(base) = self.erase_base_cache.get(&fs_idx) {
                    let tex = ctx.load_texture(
                        format!("fs_restored_{fs_idx}"),
                        base.as_ref().clone(),
                        egui::TextureOptions::LINEAR,
                    );
                    let bg = self.effective_upscale_bg_mode();
                    // fs_cache を上書きするケースでは、既存エントリに保存されている
                    // 原寸 (source_dims) をそのまま引き継ぐ。ai_upscale_cache 側は
                    // 派生キャッシュなので None で良い。
                    let prev_source_dims = self.fs_cache.get(&fs_idx).and_then(|e| {
                        if let FsCacheEntry::Static { source_dims, .. } = e {
                            *source_dims
                        } else {
                            None
                        }
                    });
                    let write_to_ai = self.ai_upscale_cache.contains_key(&(fs_idx, bg));
                    let entry = FsCacheEntry::Static {
                        tex,
                        pixels: Arc::clone(base),
                        source_dims: if write_to_ai { None } else { prev_source_dims },
                        load_seq: self.input_seq,
                    };
                    if write_to_ai {
                        self.ai_upscale_cache.insert((fs_idx, bg), entry);
                    } else {
                        self.fs_cache.insert(fs_idx, entry);
                    }
                }
            }
        }
        y += 28.0;

        // ── ヘルプテキスト ──
        let help = "E:補完 ESC:終了/選択解除 Ctrl+Z:戻す\n\
                    矢印:シフト [/]:回転 (Ctrl:10倍)\n\
                    Ctrl+ドラッグ: 筆/直線→太さ\n\
                    \u{00A0}縦横線→パン/傾き 選択→回転/太さ\n\
                    選択ツール+クリック=選択  Del:削除";
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
        let composite = match self.composite_mask() {
            Some(c) if c.iter().any(|&m| m) => c,
            _ => { self.reset_erase_mode(); return; }
        };
        let Some(bitmap) = self.erase_mask.clone() else {
            self.reset_erase_mode();
            return;
        };
        let original = match self.erase_base_cache.get(&fs_idx) {
            Some(p) => Arc::clone(p),
            None => { self.reset_erase_mode(); return; }
        };
        let [w, h] = self.erase_mask_size;

        // ビットマップとベクタを別々に永続化することで、再編集時に両者を分離して読み直せる。
        let vectors_snapshot = self.erase_vectors.clone();
        self.save_mask_with_sidecar(fs_idx, &bitmap, &vectors_snapshot, w, h);

        let masked_count = composite.iter().filter(|&&m| m).count();
        crate::logger::log(format!("erase: inpaint start, masked pixels={masked_count}"));
        self.run_inpaint_and_cache(ctx, fs_idx, &original, &composite, w, h, "exec");
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

        let (bitmap, vectors) = match self.mask_db.as_ref().and_then(|db| db.get_full(&key, w, h)) {
            Some(m) => m,
            None => return,
        };
        let mut composite = bitmap.clone();
        crate::mask_db::rasterize_vectors_into(&mut composite, &vectors, w, h);
        if !composite.iter().any(|&m| m) { return; }

        crate::logger::log(format!("erase: auto-applying saved mask for idx={idx}"));

        // 元画像を base_cache に保存（サイズが変わった場合は更新）
        let need_update = self.erase_base_cache.get(&idx)
            .map(|old| old.size != pixels.size)
            .unwrap_or(true);
        if need_update {
            self.erase_base_cache.insert(idx, Arc::clone(&pixels));
        }

        self.run_inpaint_and_cache(ctx, idx, &pixels, &composite, w, h, "auto-apply");
    }

    /// フルスクリーン表示中 (消しゴムモード外) で F7/F8 から呼ばれる。
    /// スロット N のマスクを現ページに**差し替えて**保存し、inpaint を実行する。
    /// 消しゴムモードに入る必要なく 1 キーでマスクを適用できる。
    /// 偶数/奇数ページ取り違えで旧マスクと合成されないよう上書き仕様。
    pub(crate) fn apply_slot_in_viewing_mode(&mut self, ctx: &egui::Context, slot: usize) {
        if self.erase_mode {
            return;
        }
        let Some(fs_idx) = self.fullscreen_idx else { return; };

        let pixels = match self.fs_cache.get(&fs_idx) {
            Some(FsCacheEntry::Static { pixels, .. }) => Arc::clone(pixels),
            _ => {
                self.show_feedback_toast("[画像読込中]".to_string());
                return;
            }
        };
        let [w, h] = pixels.size;

        // スロットのマスクとベクタを取得
        let slot_data = self.mask_db.as_ref().and_then(|db| db.get_slot_full(slot, w, h));
        let Some((new_mask, new_vectors)) = slot_data else {
            self.show_feedback_toast(format!("[スロット{slot}は空です]"));
            return;
        };
        if !new_mask.iter().any(|&m| m) && new_vectors.is_empty() {
            self.show_feedback_toast(format!("[スロット{slot}は空です]"));
            return;
        }

        // 元画像を base_cache に保存 (サイズが変わっていれば更新)
        let need_update = self.erase_base_cache.get(&fs_idx)
            .map(|old| old.size != pixels.size)
            .unwrap_or(true);
        if need_update {
            self.erase_base_cache.insert(fs_idx, Arc::clone(&pixels));
        }
        let original = Arc::clone(self.erase_base_cache.get(&fs_idx).unwrap());

        let mut composite = new_mask.clone();
        crate::mask_db::rasterize_vectors_into(&mut composite, &new_vectors, w, h);
        if !composite.iter().any(|&m| m) {
            return;
        }

        self.save_mask_with_sidecar(fs_idx, &new_mask, &new_vectors, w, h);

        crate::logger::log(format!(
            "erase: apply_slot_in_viewing_mode slot={slot} idx={fs_idx}"
        ));

        self.run_inpaint_and_cache(ctx, fs_idx, &original, &composite, w, h, "viewing-mode");
        self.show_feedback_toast(format!("[スロット{slot}適用]"));
    }

    /// MI-GAN (失敗時は拡散フォールバック) で inpaint を走らせ、結果を `fs_cache` に
    /// 差し込んで上位レイヤ (AI upscale / 補正) のキャッシュを無効化する共通経路。
    /// E キーの確定・保存済みマスクの自動適用・F7/F8 の 3 つから呼ばれる。
    /// `log_prefix` はログ行を区別するための識別子。
    fn run_inpaint_and_cache(
        &mut self,
        ctx: &egui::Context,
        idx: usize,
        original: &egui::ColorImage,
        composite: &[bool],
        w: usize,
        h: usize,
        log_prefix: &str,
    ) {
        self.ensure_ai_runtime();
        let result = self.try_migan_inpaint(original, composite, w, h)
            .unwrap_or_else(|e| {
                crate::logger::log(format!(
                    "[erase] {log_prefix} MI-GAN failed: {e}, falling back to diffusion"
                ));
                inpaint_diffuse(original, composite, w, h)
            });
        let tex = ctx.load_texture(
            format!("fs_inpainted_{idx}"),
            result.clone(),
            egui::TextureOptions::LINEAR,
        );
        // Inpaint で fs_cache を上書きするので、原寸 (source_dims) は既存エントリから
        // 引き継ぐ (inpaint はサイズ保存のため)。
        let prev_source_dims = self.fs_cache.get(&idx).and_then(|e| {
            if let FsCacheEntry::Static { source_dims, .. } = e {
                *source_dims
            } else {
                None
            }
        });
        self.fs_cache.insert(idx, FsCacheEntry::Static {
            tex,
            pixels: Arc::new(result),
            source_dims: prev_source_dims,
            load_seq: self.input_seq,
        });
        self.invalidate_derived_fs_caches(idx);
    }

    /// `fs_cache` を差し替えたあとに呼ぶ。上位レイヤ (AI アップスケール / 補正) の
    /// キャッシュを無効化して、新しい元画像で再処理させる。
    /// 処理中の AI タスクがあればキャンセル。
    pub(crate) fn invalidate_derived_fs_caches(&mut self, idx: usize) {
        self.purge_upscale_for_idx(idx);
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
        let manager = self.ai_model_manager.clone();

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

/// ビットマップマスクを (dx, dy) ピクセル平行移動する (はみ出た部分は false)。
/// 小さなシフトのみを想定 (ゴミ位置補正)。
pub(crate) fn shift_bitmap(mask: &mut [bool], w: usize, h: usize, dx: f32, dy: f32) {
    let shift_x = dx.round() as isize;
    let shift_y = dy.round() as isize;
    if shift_x == 0 && shift_y == 0 { return; }
    let src = mask.to_vec();
    for y in 0..h {
        for x in 0..w {
            let sx = x as isize - shift_x;
            let sy = y as isize - shift_y;
            mask[y * w + x] = if sx >= 0 && sy >= 0 && (sx as usize) < w && (sy as usize) < h {
                src[sy as usize * w + sx as usize]
            } else {
                false
            };
        }
    }
}

/// ビットマップマスクを中心 (cx, cy) 周りに angle [rad] 回転する (nearest-neighbor)。
/// 1bit マスクなので累積回転で劣化する。ユーザ向けには small-angle 前提。
pub(crate) fn rotate_bitmap(
    mask: &mut [bool],
    w: usize,
    h: usize,
    cx: f32,
    cy: f32,
    angle: f32,
) {
    let (s, c) = (-angle).sin_cos(); // 逆変換で src 座標を取る
    let src = mask.to_vec();
    for y in 0..h {
        for x in 0..w {
            let rx = x as f32 - cx;
            let ry = y as f32 - cy;
            let sxf = cx + rx * c - ry * s;
            let syf = cy + rx * s + ry * c;
            let sx = sxf.round();
            let sy = syf.round();
            mask[y * w + x] = if sx >= 0.0 && sy >= 0.0 && sx < w as f32 && sy < h as f32 {
                src[sy as usize * w + sx as usize]
            } else {
                false
            };
        }
    }
}

pub(crate) fn dist_sq(a: (f32, f32), b: (f32, f32)) -> f32 {
    let dx = a.0 - b.0;
    let dy = a.1 - b.1;
    dx * dx + dy * dy
}

pub(crate) fn bounding_box(pts: &[(f32, f32)]) -> (f32, f32, f32, f32) {
    let mut xmin = f32::MAX;
    let mut ymin = f32::MAX;
    let mut xmax = f32::MIN;
    let mut ymax = f32::MIN;
    for &(x, y) in pts {
        if x < xmin { xmin = x; }
        if y < ymin { ymin = y; }
        if x > xmax { xmax = x; }
        if y > ymax { ymax = y; }
    }
    (xmin, ymin, xmax, ymax)
}

/// 偶奇規則の点多角形判定。
pub(crate) fn point_in_polygon(pt: (f32, f32), poly: &[(f32, f32)]) -> bool {
    let n = poly.len();
    if n < 3 { return false; }
    let mut inside = false;
    let mut j = n - 1;
    for i in 0..n {
        let (xi, yi) = poly[i];
        let (xj, yj) = poly[j];
        if (yi > pt.1) != (yj > pt.1) {
            let x_intersect = (xj - xi) * (pt.1 - yi) / (yj - yi + 1e-12) + xi;
            if pt.0 < x_intersect { inside = !inside; }
        }
        j = i;
    }
    inside
}

/// 円周に沿って白黒の破線を交互に描画する。
/// どの背景色 (白/黒/中間色) でも視認できるブラシカーソル用。
/// 内側を黒線、外側を白線で 1px ずつずらして描くことで
/// 単色背景でも必ず片方が見える。
fn draw_dashed_circle(painter: &egui::Painter, center: egui::Pos2, radius: f32) {
    if radius < 1.5 {
        // 小さい場合はシンプルに十字で表示
        let s = 4.0;
        let outer = egui::Stroke::new(3.0, egui::Color32::BLACK);
        let inner = egui::Stroke::new(1.0, egui::Color32::WHITE);
        painter.line_segment([center - egui::vec2(s, 0.0), center + egui::vec2(s, 0.0)], outer);
        painter.line_segment([center - egui::vec2(0.0, s), center + egui::vec2(0.0, s)], outer);
        painter.line_segment([center - egui::vec2(s, 0.0), center + egui::vec2(s, 0.0)], inner);
        painter.line_segment([center - egui::vec2(0.0, s), center + egui::vec2(0.0, s)], inner);
        return;
    }

    // 円周を N セグメントに分割し、交互に白/黒で描画。
    // セグメント数は半径に比例 (最小 32、最大 128)。
    let circumference = 2.0 * std::f32::consts::PI * radius;
    let seg_len = 8.0f32; // 1 セグメントあたりの円弧長 (screen px)
    let n = ((circumference / seg_len).round() as usize).clamp(32, 128);
    // 偶数にして黒/白を均等に
    let n = if n % 2 == 0 { n } else { n + 1 };

    let black = egui::Stroke::new(2.5, egui::Color32::BLACK);
    let white = egui::Stroke::new(1.5, egui::Color32::WHITE);

    let mut points: Vec<egui::Pos2> = Vec::with_capacity(n + 1);
    for i in 0..=n {
        let t = (i as f32 / n as f32) * std::f32::consts::TAU;
        points.push(center + egui::vec2(t.cos() * radius, t.sin() * radius));
    }

    // 黒い太めの線でベースを全周描画
    for i in 0..n {
        painter.line_segment([points[i], points[i + 1]], black);
    }
    // その上に白い細めの線を破線状に (偶数番目のセグメントだけ) 描画
    for i in 0..n {
        if i % 2 == 0 {
            painter.line_segment([points[i], points[i + 1]], white);
        }
    }
}
