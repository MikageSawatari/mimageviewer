//! 隠蔽加工 (Conceal) モード: フルスクリーン画像の任意領域に
//! モザイク / 白塗り / 黒塗り / ぼかし を適用する。
//!
//! 詳細仕様: [docs/conceal-feature-plan.md](../../docs/conceal-feature-plan.md)
//!
//! # Phase 進捗 (Phase 2 時点)
//!
//! - 8 ツールパレット (Select / Brush / Lasso / Line / VertLine / HorizLine /
//!   Rect / Ellipse) と `D` / `F` (描画 / 消去) 切替を実装
//! - マスクオーバーレイ (紫半透明、消しゴムの赤系と区別)
//! - Select ツールは [`crate::vector_edit`] 経由で handle 編集 (角・辺中点・
//!   回転ハンドル + 端点)。Shift で軸拘束 / 等比 / 角度スナップ、Alt で中心固定
//! - Brush / Lasso はビットマップ塗り (消しゴムと同じ scanline ロジック)
//! - Line / VertLine / HorizLine / Rect / Ellipse はベクタ Shape を新規作成
//! - Undo: `ConcealSnapshot` で mask + shapes をまとめて記録
//!
//! Phase 3 以降で合成 (Mosaic / Fill / Blur) と DB 永続化を追加する。

use eframe::egui;
use std::sync::Arc;

use crate::app::{App, ConcealSnapshot, EraseSpreadCtx};
use crate::conceal::ConcealTool;
use crate::fs_animation::FsCacheEntry;
use crate::mask_db::{LineKind, Shape};
use crate::ui_fullscreen::FsKeyAction;
use crate::vector_edit;

// ── 定数 ────────────────────────────────────────────────────────────────

/// ツールパネルの幅。消しゴム ([`crate::ui_erase::PANEL_W`] = 190px) より少し広い
/// 220px (タイル比率スライダー + 境界処理のラジオ 3 行を持つため)。
const PANEL_W: f32 = 220.0;
/// ツールパネルの左上マージン。
const PANEL_MARGIN_X: f32 = 16.0;
const PANEL_MARGIN_Y: f32 = 60.0;

/// Undo スタックの最大エントリ数。
const UNDO_MAX: usize = 20;

/// Undo throttle (キーリピート連打中のスナップショット間引き)。
const UNDO_COALESCE_MS: u128 = 500;

/// マスクオーバーレイの色 (紫半透明、消しゴムの赤系 [255, 60, 60, 140] と区別)。
const MASK_OVERLAY_R: u8 = 180;
const MASK_OVERLAY_G: u8 = 80;
const MASK_OVERLAY_B: u8 = 220;
const MASK_OVERLAY_A: u8 = 140;

/// 矢印キー 1 回あたりの平行移動量 (画像 px)。
const NUDGE_PIXELS: f32 = 1.0;
const NUDGE_PIXELS_FAST: f32 = 10.0;

impl App {
    /// 「フルスクリーン上の主要 UI (メタデータパネル / 上部ホバーバー / カーソル自動隠し /
    /// マウスホイールでのページ送り 等) を抑制すべき編集モード」が現在 active か判定する
    /// (= 消しゴム or 隠蔽加工)。Phase 4 で追加。
    ///
    /// 既存コード (ui_fullscreen.rs) に散らばっている `!self.erase_mode` 判定の多くは
    /// 「ペイント中の UI を邪魔するな」という同じ意図なので、本ヘルパーで統一する。
    /// 将来別の overlay edit mode (例: 切り抜き / ワイヤフレーム) を追加するときも
    /// この 1 箇所を拡張するだけで済む。
    pub(crate) fn is_overlay_edit_mode_active(&self) -> bool {
        self.erase_mode || self.conceal_mode
    }

    // ── モード入退場 ────────────────────────────────────────────────

    /// 隠蔽加工モードに入る。
    ///
    /// 動作は [`App::enter_erase_mode`] をベースに、Conceal 専用フィールド
    /// (`conceal_*`) を初期化する。`conceal_db` から該当ページのマスクを
    /// hydrate し、ベクタ群 (`conceal_shapes`) も復元する。
    ///
    /// 見開き表示中に呼ばれた場合は spread_mode を一時的に Single へ落とし
    /// (消しゴムと同じ振る舞い、`conceal_spread_ctx` に元状態を保存)、
    /// `reset_conceal_mode` で復元する。
    pub(crate) fn enter_conceal_mode(&mut self, fs_idx: usize) {
        // 見開き → Single ピボット (消しゴムの enter_erase_mode と同じ作法)
        let spread_pair = match self.resolve_spread_pair(fs_idx) {
            crate::ui_fullscreen::SpreadPair::Double { left, right } => Some((left, right)),
            crate::ui_fullscreen::SpreadPair::Single => None,
        };
        let target_idx = spread_pair.map(|(l, _)| l).unwrap_or(fs_idx);

        // 元画像取得 (state mutation 前)。conceal_base_cache を優先、なければ
        // adjustment_cache / ai_upscale_cache / fs_cache の順で探す。
        let pixels = if let Some(base) = self.conceal_base_cache.get(&target_idx) {
            Arc::clone(base)
        } else {
            let bg = self.effective_upscale_bg_mode();
            let from_cache = self
                .ai_upscale_cache
                .get(&(target_idx, bg))
                .or_else(|| self.fs_cache.get(&target_idx))
                .and_then(|entry| match entry {
                    FsCacheEntry::Static { pixels, .. } => Some(Arc::clone(pixels)),
                    _ => None,
                });
            match from_cache {
                Some(p) => {
                    self.conceal_base_cache.insert(target_idx, Arc::clone(&p));
                    p
                }
                None => {
                    crate::logger::log("conceal: enter aborted (no base pixels)".to_string());
                    return;
                }
            }
        };

        // 見開き状態をスナップショット
        if let Some(pair) = spread_pair {
            self.conceal_spread_ctx = Some(EraseSpreadCtx {
                saved_mode: self.spread_mode,
                pair,
            });
            self.spread_mode = crate::settings::SpreadMode::Single;
            self.fullscreen_idx = Some(target_idx);
            self.fs_zoom = 1.0;
            self.fs_pan = egui::Vec2::ZERO;
        }
        let fs_idx = target_idx;
        let [w, h] = pixels.size;

        self.conceal_mode = true;
        self.clear_meta_undo();
        if !self.post_filter_bypassed {
            self.post_filter_bypassed = true;
            self.clear_adjustment_caches(fs_idx);
        }

        self.conceal_mask_size = [w, h];
        self.conceal_mask_texture = None;
        self.conceal_paint_mode = true;
        self.conceal_undo_stack.clear();
        self.conceal_last_undo_at = None;
        self.conceal_shapes.clear();
        self.conceal_selected_shape = None;
        self.conceal_drag = None;
        self.conceal_last_paint_pos = None;
        self.conceal_lasso_points.clear();
        self.conceal_line_start = None;
        self.conceal_line_end = None;
        self.conceal_shape_drag_start = None;
        self.conceal_shape_drag_end = None;

        // デフォルトブラシ半径 / 直線幅 (まだ未初期化のときだけ)
        if self.settings.conceal_brush_radius <= 0.0 {
            self.settings.conceal_brush_radius = (w.max(h) as f32 / 100.0).max(2.0);
        }
        if self.settings.conceal_line_width <= 0.0 {
            self.settings.conceal_line_width = (w.max(h) as f32 / 500.0).max(2.0);
        }

        // DB からマスク (ビットマップ + ベクタ) をロード
        let (loaded_mask, loaded_shapes) = self
            .page_path_key(fs_idx)
            .and_then(|key| self.conceal_db.as_ref()?.get_full(&key, w, h))
            .unwrap_or_else(|| (vec![false; w * h], Vec::new()));

        self.conceal_mask = Some(loaded_mask);
        self.conceal_shapes = loaded_shapes;
        crate::logger::log(format!(
            "conceal: enter mode, image={w}x{h}, shapes={}, type={:?}, tool={:?}",
            self.conceal_shapes.len(),
            self.settings.conceal_type,
            self.conceal_tool,
        ));
    }

    /// 隠蔽加工モードを終了する (Esc / Ctrl+M 再押下)。
    ///
    /// Phase 4 から: 現在ページの編集中マスク (ビットマップ + Shape) を
    /// `conceal_db` + サイドカーに保存する。空マスクなら DB エントリを削除し
    /// `conceal_pages` バッジ集合からも除外する (`save_conceal_with_sidecar` の
    /// 空マスク経路に揃える)。表示パイプライン側の `conceal_cache` は次回の
    /// composite で自然に再生成されるため、ここでは触らない。
    pub(crate) fn reset_conceal_mode(&mut self) {
        let restore_idx = self.fullscreen_idx;
        let was_conceal_mode = self.conceal_mode;

        // DB / サイドカー書き込みは state mutation の前に行う (page_path_key は
        // fullscreen_idx を必要とするため、`conceal_mode = false` 後でも値は残るが
        // 順序保証のためここで実施する)。
        if was_conceal_mode {
            if let Some(idx) = self.fullscreen_idx {
                let [w, h] = self.conceal_mask_size;
                if let Some(mask) = self.conceal_mask.clone() {
                    let shapes = self.conceal_shapes.clone();
                    self.save_conceal_with_sidecar(idx, &mask, &shapes, w, h);
                }
                // 編集中のマスクが新しい形状になったので、`conceal_cache[idx]` を
                // 破棄して退場後の最初の表示パスで再合成させる (Phase 4)。
                self.clear_conceal_caches(idx);
            }
        }

        self.conceal_mode = false;

        if was_conceal_mode {
            self.clear_meta_undo();
        }

        // post-filter バイパス解除 (analysis_mode が同時にアクティブでない場合)
        if self.post_filter_bypassed && !self.analysis_mode {
            self.post_filter_bypassed = false;
            if let Some(idx) = restore_idx {
                self.clear_adjustment_caches(idx);
            }
        }

        self.conceal_mask = None;
        self.conceal_mask_size = [0, 0];
        self.conceal_mask_texture = None;
        self.conceal_shapes.clear();
        self.conceal_selected_shape = None;
        self.conceal_undo_stack.clear();
        self.conceal_last_undo_at = None;
        self.conceal_drag = None;
        self.conceal_last_paint_pos = None;
        self.conceal_lasso_points.clear();
        self.conceal_line_start = None;
        self.conceal_line_end = None;
        self.conceal_shape_drag_start = None;
        self.conceal_shape_drag_end = None;
        self.fs_pan_drag_start = None;

        // 見開きから入っていた場合は spread_mode と表示ページを復元
        if let Some(ctx) = self.conceal_spread_ctx.take() {
            self.spread_mode = ctx.saved_mode;
            self.fullscreen_idx = Some(ctx.pair.0);
            self.fs_zoom = 1.0;
            self.fs_pan = egui::Vec2::ZERO;
        }
        crate::logger::log("conceal: reset mode".to_string());
    }

    // ── Undo ────────────────────────────────────────────────────────

    pub(crate) fn push_conceal_undo(&mut self) {
        if let Some(mask) = &self.conceal_mask {
            self.conceal_undo_stack.push_back(ConcealSnapshot {
                mask: mask.clone(),
                shapes: self.conceal_shapes.clone(),
            });
            while self.conceal_undo_stack.len() > UNDO_MAX {
                self.conceal_undo_stack.pop_front();
            }
            self.conceal_last_undo_at = Some(std::time::Instant::now());
        }
    }

    /// キーリピート連打中にスナップショットを毎フレーム取らないための版。
    fn push_conceal_undo_throttled(&mut self) {
        if let Some(last) = self.conceal_last_undo_at {
            if last.elapsed().as_millis() < UNDO_COALESCE_MS {
                return;
            }
        }
        self.push_conceal_undo();
    }

    pub(crate) fn undo_conceal(&mut self) -> bool {
        if let Some(prev) = self.conceal_undo_stack.pop_back() {
            self.conceal_mask = Some(prev.mask);
            self.conceal_shapes = prev.shapes;
            self.conceal_selected_shape = None;
            self.conceal_drag = None;
            self.conceal_mask_texture = None;
            true
        } else {
            false
        }
    }

    // ── マスクスロット (差分画像生成サポート、消しゴム save_mask_to_slot と対称) ──

    /// 現在のマスク (ビットマップ + Shape) をスロットに保存する。
    /// スロット番号は 1 か 2 (`conceal_db::slot_key` 経由で `__slot_N` キーへ)。
    pub(crate) fn save_conceal_mask_to_slot(&mut self, slot: usize) {
        let [w, h] = self.conceal_mask_size;
        let saved = if let (Some(mask), Some(db)) = (&self.conceal_mask, &self.conceal_db) {
            db.set_slot(slot, mask, &self.conceal_shapes, w, h).is_ok()
        } else {
            false
        };
        if saved {
            self.show_feedback_toast(format!("[隠蔽スロット{}に保存]", slot));
        } else {
            self.show_feedback_toast(format!("[隠蔽スロット{}保存失敗]", slot));
        }
    }

    /// スロットからマスクをロードし、現在のマスクを **差し替える** (消しゴムと同じ仕様)。
    /// 直前の状態は Ctrl+Z で戻せるので、取り違えロードを安全に巻き戻せる。
    pub(crate) fn load_conceal_mask_from_slot(&mut self, slot: usize) {
        let [w, h] = self.conceal_mask_size;
        let slot_data = self
            .conceal_db
            .as_ref()
            .and_then(|db| db.get_slot_full(slot, w, h));
        let Some((slot_mask, slot_shapes)) = slot_data else {
            self.show_feedback_toast(format!("[隠蔽スロット{}は空です]", slot));
            return;
        };
        if !slot_mask.iter().any(|&m| m) && slot_shapes.is_empty() {
            self.show_feedback_toast(format!("[隠蔽スロット{}は空です]", slot));
            return;
        }
        self.push_conceal_undo();
        self.conceal_mask = Some(slot_mask);
        self.conceal_shapes = slot_shapes;
        self.conceal_selected_shape = None;
        self.conceal_mask_texture = None;
        self.show_feedback_toast(format!("[隠蔽スロット{}をロード]", slot));
    }

    // ── マスク全削除 (パネルボタンから呼ぶ) ──────────────────────────────

    /// 現在ページの隠蔽マスクを全消去する (消しゴムの delete-all と対称)。
    /// DB / サイドカーからも削除し、`conceal_pages` バッジ集合からも除外する。
    /// `conceal_mode` 自体は維持 (= モードに留まり、そのまま続きの編集ができる)。
    pub(crate) fn delete_all_conceal_mask(&mut self) {
        let [w, h] = self.conceal_mask_size;
        if w == 0 || h == 0 {
            return;
        }
        self.push_conceal_undo();
        self.conceal_mask = Some(vec![false; w * h]);
        self.conceal_shapes.clear();
        self.conceal_selected_shape = None;
        self.conceal_drag = None;
        self.conceal_mask_texture = None;
        // 編集中の一時状態も破棄
        self.conceal_last_paint_pos = None;
        self.conceal_lasso_points.clear();
        self.conceal_line_start = None;
        self.conceal_line_end = None;
        self.conceal_shape_drag_start = None;
        self.conceal_shape_drag_end = None;
        if let Some(idx) = self.fullscreen_idx {
            self.delete_conceal_with_sidecar(idx);
            // 表示パイプラインの該当 idx エントリは次フレームの compose で空マスク扱いに
            // なるため、明示的な invalidate は不要 (compose_conceal_for_idx 側で
            // composite_mask が空のとき bypass する)。
            self.clear_conceal_caches(idx);
        }
    }

    // ── パラメータプリセット (4 スロット、settings 永続化) ──────────────────

    /// スロット番号 (0..4) のプリセットを現在の settings に適用する。
    /// 隠蔽タイプとすべてのパラメータが一度に変わるため、`conceal_cache` の世代を
    /// bump して全 idx を stale 扱いにする (次回 compose で再生成)。
    pub(crate) fn apply_conceal_preset(&mut self, slot: usize) {
        let Some(preset) = self.settings.conceal_presets.get(slot).cloned().flatten() else {
            self.show_feedback_toast(format!("[プリセット{}は空]", slot + 1));
            return;
        };
        self.settings.conceal_type = preset.conceal_type;
        self.settings.conceal_mosaic_tile_mode = preset.mosaic_tile_mode;
        self.settings.conceal_mosaic_boundary = preset.mosaic_boundary;
        self.settings.conceal_fill_opacity_percent = preset.fill_opacity_percent;
        self.settings.conceal_fill_edge = preset.fill_edge;
        self.settings.conceal_blur_radius_px = preset.blur_radius_px;
        self.settings.conceal_blur_mode = preset.blur_mode;
        self.settings.conceal_blur_feather = preset.blur_feather;
        self.bump_conceal_generation();
        let label = if preset.name.is_empty() {
            format!("プリセット{}", slot + 1)
        } else {
            preset.name.clone()
        };
        self.show_feedback_toast(format!("[適用: {}]", label));
    }

    /// 現在の settings をスロット番号 (0..4) に保存する。既存名は保持。
    pub(crate) fn save_conceal_preset_to_slot(&mut self, slot: usize) {
        if slot >= 4 {
            return;
        }
        let prev_name = self
            .settings
            .conceal_presets
            .get(slot)
            .and_then(|p| p.as_ref())
            .map(|p| p.name.clone())
            .unwrap_or_default();
        let preset = crate::conceal::ConcealPreset {
            name: prev_name,
            conceal_type: self.settings.conceal_type,
            mosaic_tile_mode: self.settings.conceal_mosaic_tile_mode,
            mosaic_boundary: self.settings.conceal_mosaic_boundary,
            fill_opacity_percent: self.settings.conceal_fill_opacity_percent,
            fill_edge: self.settings.conceal_fill_edge,
            blur_radius_px: self.settings.conceal_blur_radius_px,
            blur_mode: self.settings.conceal_blur_mode,
            blur_feather: self.settings.conceal_blur_feather,
        };
        self.settings.conceal_presets[slot] = Some(preset);
        self.show_feedback_toast(format!("[プリセット{}に保存]", slot + 1));
    }

    // ── キー入力 ────────────────────────────────────────────────────

    /// 隠蔽加工モード中のキー入力を処理する。
    pub(crate) fn handle_conceal_keys(
        &mut self,
        ctx: &egui::Context,
        _fs_idx: usize,
    ) -> FsKeyAction {
        let action = FsKeyAction {
            close: false,
            nav_delta: 0,
            ctrl_nav: None,
            jump_to: None,
        };

        // ESC: 選択中があればまず解除、無ければモード終了
        let esc = ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Escape));
        if esc {
            if self.conceal_selected_shape.is_some() {
                self.conceal_selected_shape = None;
                self.conceal_drag = None;
                return action;
            }
            self.reset_conceal_mode();
            return action;
        }

        // Ctrl+M: モード終了 (再押下で抜ける、ui_fullscreen から委譲済み判定用)
        let ctrl_m = ctx.input_mut(|i| i.consume_key(egui::Modifiers::CTRL, egui::Key::M));
        if ctrl_m {
            self.reset_conceal_mode();
            return action;
        }

        // Ctrl+Z: Undo
        let ctrl_z = ctx.input_mut(|i| i.consume_key(egui::Modifiers::CTRL, egui::Key::Z));
        if ctrl_z {
            if self.undo_conceal() {
                self.show_feedback_toast("[元に戻す]".to_string());
            } else {
                self.show_feedback_toast("[履歴なし]".to_string());
            }
        }

        // Delete: 選択中の Shape を削除
        let key_del = ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Delete));
        if key_del {
            if let Some(idx) = self.conceal_selected_shape {
                if idx < self.conceal_shapes.len() {
                    self.push_conceal_undo();
                    self.conceal_shapes.remove(idx);
                    self.conceal_selected_shape = None;
                    self.conceal_drag = None;
                    self.conceal_mask_texture = None;
                    self.show_feedback_toast("[ベクタ削除]".to_string());
                }
            }
        }

        let ctrl_held = ctx.input(|i| i.modifiers.ctrl);

        // 矢印キー: 平行移動 (Ctrl で 10px)
        let step = if ctrl_held {
            NUDGE_PIXELS_FAST
        } else {
            NUDGE_PIXELS
        };
        let (mut dx, mut dy) = (0.0f32, 0.0f32);
        ctx.input_mut(|i| {
            if i.consume_key(egui::Modifiers::NONE, egui::Key::ArrowLeft)
                || i.consume_key(egui::Modifiers::CTRL, egui::Key::ArrowLeft)
            {
                dx -= step;
            }
            if i.consume_key(egui::Modifiers::NONE, egui::Key::ArrowRight)
                || i.consume_key(egui::Modifiers::CTRL, egui::Key::ArrowRight)
            {
                dx += step;
            }
            if i.consume_key(egui::Modifiers::NONE, egui::Key::ArrowUp)
                || i.consume_key(egui::Modifiers::CTRL, egui::Key::ArrowUp)
            {
                dy -= step;
            }
            if i.consume_key(egui::Modifiers::NONE, egui::Key::ArrowDown)
                || i.consume_key(egui::Modifiers::CTRL, egui::Key::ArrowDown)
            {
                dy += step;
            }
        });
        if dx != 0.0 || dy != 0.0 {
            self.nudge_conceal(dx, dy);
        }

        // ツール切替: S/B/L/I/V/H/R/O
        let mut switched: Option<ConcealTool> = None;
        ctx.input_mut(|i| {
            if i.consume_key(egui::Modifiers::NONE, egui::Key::S) {
                switched = Some(ConcealTool::Select);
            }
            if i.consume_key(egui::Modifiers::NONE, egui::Key::B) {
                switched = Some(ConcealTool::Brush);
            }
            if i.consume_key(egui::Modifiers::NONE, egui::Key::L) {
                switched = Some(ConcealTool::Lasso);
            }
            if i.consume_key(egui::Modifiers::NONE, egui::Key::I) {
                switched = Some(ConcealTool::Line);
            }
            if i.consume_key(egui::Modifiers::NONE, egui::Key::V) {
                switched = Some(ConcealTool::VertLine);
            }
            if i.consume_key(egui::Modifiers::NONE, egui::Key::H) {
                switched = Some(ConcealTool::HorizLine);
            }
            if i.consume_key(egui::Modifiers::NONE, egui::Key::R) {
                switched = Some(ConcealTool::Rect);
            }
            if i.consume_key(egui::Modifiers::NONE, egui::Key::O) {
                switched = Some(ConcealTool::Ellipse);
            }
        });
        if let Some(tool) = switched {
            self.conceal_tool = tool;
            self.conceal_drag = None;
            self.conceal_lasso_points.clear();
            self.conceal_line_start = None;
            self.conceal_line_end = None;
            self.conceal_shape_drag_start = None;
            self.conceal_shape_drag_end = None;
            self.show_feedback_toast(format!("[{}]", tool.label()));
        }

        // D: 描画モード, F: 消去モード
        let key_d = ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::D));
        let key_f = ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::F));
        if key_d {
            self.conceal_paint_mode = true;
            self.show_feedback_toast("[描画モード]".to_string());
        }
        if key_f {
            self.conceal_paint_mode = false;
            self.show_feedback_toast("[消去モード]".to_string());
        }

        // T: 隠蔽タイプを順次切替 (Mosaic → WhiteFill → BlackFill → Blur → Mosaic …)。
        // タイプ変更はグローバルパラメータの変更なので世代を bump。
        let key_t = ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::T));
        if key_t {
            let next = self.settings.conceal_type.next();
            self.settings.conceal_type = next;
            self.bump_conceal_generation();
            self.show_feedback_toast(format!("[処理: {}]", next.label()));
        }

        // 1/2/3/4: プリセット適用 (modifier なし)。consume_key の結果を一旦ローカル
        // 変数に集めてから input_mut の借用を解放したあと `apply_conceal_preset` を呼ぶ
        // (self mut + input mut の同時借用を避ける)。
        let mut preset_slot: Option<usize> = None;
        ctx.input_mut(|i| {
            for (k, slot) in [
                (egui::Key::Num1, 0usize),
                (egui::Key::Num2, 1),
                (egui::Key::Num3, 2),
                (egui::Key::Num4, 3),
            ] {
                if i.consume_key(egui::Modifiers::NONE, k) {
                    preset_slot = Some(slot);
                }
            }
        });
        if let Some(slot) = preset_slot {
            self.apply_conceal_preset(slot);
        }

        // 未使用キーを消費してフルスクリーン共通ショートカットの動作を抑止する
        // (T / Num1..4 は上で処理済みなのでここには含めない)。
        const SINGLE_KEYS: &[egui::Key] = &[
            egui::Key::Space,
            egui::Key::Tab,
            egui::Key::G,
            egui::Key::P,
            egui::Key::U,
            egui::Key::N,
            egui::Key::Z,
            egui::Key::F1,
            egui::Key::F2,
            egui::Key::F3,
            egui::Key::F4,
            egui::Key::F5,
            egui::Key::F6,
        ];
        ctx.input_mut(|i| {
            for &k in SINGLE_KEYS {
                let _ = i.consume_key(egui::Modifiers::NONE, k);
            }
        });

        action
    }

    /// 矢印キー / Ctrl+矢印キーでマスクをシフトする。
    /// 選択中の Shape があればそれだけ、無ければビットマップ + 全 Shape を動かす。
    fn nudge_conceal(&mut self, dx: f32, dy: f32) {
        self.push_conceal_undo_throttled();
        match self.conceal_selected_shape {
            Some(idx) if idx < self.conceal_shapes.len() => {
                self.conceal_shapes[idx].translate(dx, dy);
            }
            _ => {
                for s in &mut self.conceal_shapes {
                    s.translate(dx, dy);
                }
                // ビットマップシフトは未実装 (erase_mode の `shift_bitmap` 同等は
                // Phase 4 でビットマップマスク移動 UX を入れるときに同居させる)。
            }
        }
        self.conceal_mask_texture = None;
    }

    // ── 座標変換 ──────────────────────────────────────────────────

    fn conceal_image_layout(
        &self,
        full_rect: egui::Rect,
        zoom_pan: Option<(f32, egui::Vec2)>,
    ) -> Option<(f32, egui::Rect)> {
        let [iw, ih] = self.conceal_mask_size;
        if iw == 0 || ih == 0 {
            return None;
        }
        let display_size = egui::vec2(iw as f32, ih as f32);
        let fit_scale =
            (full_rect.width() / display_size.x).min(full_rect.height() / display_size.y);
        let (total_scale, center) = match zoom_pan {
            Some((zoom, pan)) => (fit_scale * zoom, full_rect.center() + pan),
            None => (fit_scale, full_rect.center()),
        };
        Some((
            total_scale,
            egui::Rect::from_center_size(center, display_size * total_scale),
        ))
    }

    fn conceal_screen_to_image(
        &self,
        screen_pos: egui::Pos2,
        full_rect: egui::Rect,
        zoom_pan: Option<(f32, egui::Vec2)>,
        clamp_inside: bool,
    ) -> Option<(f32, f32)> {
        let (total_scale, img_rect) = self.conceal_image_layout(full_rect, zoom_pan)?;
        let [iw, ih] = self.conceal_mask_size;
        let nx = (screen_pos.x - img_rect.min.x) / total_scale;
        let ny = (screen_pos.y - img_rect.min.y) / total_scale;
        if clamp_inside && (nx < 0.0 || ny < 0.0 || nx >= iw as f32 || ny >= ih as f32) {
            return None;
        }
        Some((nx, ny))
    }

    fn conceal_image_to_screen(
        &self,
        img: (f32, f32),
        full_rect: egui::Rect,
        zoom_pan: Option<(f32, egui::Vec2)>,
    ) -> egui::Pos2 {
        let (total_scale, img_rect) = self
            .conceal_image_layout(full_rect, zoom_pan)
            .unwrap_or((1.0, full_rect));
        egui::pos2(
            img_rect.min.x + img.0 * total_scale,
            img_rect.min.y + img.1 * total_scale,
        )
    }

    // ── ビットマップ塗り (Brush / Lasso 用) ────────────────────────────

    fn paint_brush_line_conceal(&mut self, from: (f32, f32), to: (f32, f32), paint: bool) {
        let radius = self.settings.conceal_brush_radius.max(1.0);
        let [w, h] = self.conceal_mask_size;
        let Some(mask) = self.conceal_mask.as_mut() else {
            return;
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
        self.conceal_mask_texture = None;
    }

    fn paint_polygon_conceal(&mut self, points: &[(f32, f32)], paint: bool) {
        let [w, h] = self.conceal_mask_size;
        let Some(mask) = self.conceal_mask.as_mut() else {
            return;
        };
        crate::mask_db::scanline_fill_polygon(mask, points, w, h, paint);
        self.conceal_mask_texture = None;
    }

    // ── マスクテクスチャ ──────────────────────────────────────────

    fn ensure_conceal_mask_texture(&mut self, ctx: &egui::Context) {
        if self.conceal_mask_texture.is_some() {
            return;
        }
        let Some(composite) = self.composite_conceal_mask() else {
            return;
        };
        let [w, h] = self.conceal_mask_size;
        let mut rgba = vec![0u8; w * h * 4];
        for i in 0..composite.len() {
            if composite[i] {
                rgba[i * 4] = MASK_OVERLAY_R;
                rgba[i * 4 + 1] = MASK_OVERLAY_G;
                rgba[i * 4 + 2] = MASK_OVERLAY_B;
                rgba[i * 4 + 3] = MASK_OVERLAY_A;
            }
        }
        let ci = egui::ColorImage::from_rgba_unmultiplied([w, h], &rgba);
        let tex = ctx.load_texture("conceal_mask", ci, egui::TextureOptions::NEAREST);
        self.conceal_mask_texture = Some(tex);
    }

    /// ビットマップとベクタを合成した最終マスクを返す (Phase 3 の合成入力でも使う)。
    pub(crate) fn composite_conceal_mask(&self) -> Option<Vec<bool>> {
        let mask = self.conceal_mask.as_ref()?;
        let [w, h] = self.conceal_mask_size;
        if w == 0 || h == 0 {
            return None;
        }
        if self.conceal_shapes.is_empty() && !mask.iter().any(|&b| b) {
            return Some(vec![false; w * h]);
        }
        let mut out = mask.clone();
        crate::mask_db::rasterize_shapes_into(&mut out, &self.conceal_shapes, w, h);
        Some(out)
    }

    // ── ヒットテスト (Select ツール用) ─────────────────────────────

    /// 画像座標 `pos` から、shape のホバーターゲットを判定する。
    ///
    /// ルール (Codex P2 #4 対応):
    ///
    /// 1. **選択中の shape のハンドル (= Body 以外) は最優先**: 角・辺中点・回転・
    ///    端点が見える状態なので、それらを掴みに行く操作を阻害させない
    /// 2. **新しい順 (添字大→小) に走査**: 上に重なっているものを優先
    ///    - 選択中 shape は全ハンドル + Body
    ///    - 他の shape は Body のみ (ハンドルは描画されていないので、間違って端点に
    ///      乗ったクリックは Body 判定にフォールバックさせるべき)
    /// 3. すべて外れたら None (= 空クリック → 選択解除)
    fn hit_test_conceal(
        &self,
        pos: (f32, f32),
        scale: f32,
    ) -> Option<(usize, vector_edit::HoverTarget)> {
        // 1. 選択中の shape のハンドル (Body 以外) を最優先で判定
        if let Some(sel) = self.conceal_selected_shape {
            if let Some(s) = self.conceal_shapes.get(sel) {
                let layout = vector_edit::compute_handle_layout(s, scale);
                if let Some(t) = vector_edit::hit_test(&layout, pos, scale) {
                    if !matches!(t, vector_edit::HoverTarget::Body) {
                        return Some((sel, t));
                    }
                }
            }
        }
        // 2. 新しい順に Body 判定 (選択中含む全 shape を対象)
        for (i, s) in self.conceal_shapes.iter().enumerate().rev() {
            let layout = vector_edit::compute_handle_layout(s, scale);
            // 選択中以外は hit_test が Endpoint / Corner を返す可能性があるので、
            // Body 領域 (= body_corners 多角形) に対して別途判定する。
            if point_in_polygon_local(pos, &layout.body_corners) {
                let target = if Some(i) == self.conceal_selected_shape {
                    // 選択中 shape の Body
                    vector_edit::HoverTarget::Body
                } else {
                    vector_edit::HoverTarget::Body
                };
                return Some((i, target));
            }
        }
        None
    }

    // ── 入力処理 ──────────────────────────────────────────────────

    /// ドラッグ入力を処理する (ツール別 dispatch)。
    pub(crate) fn handle_conceal_paint(
        &mut self,
        ctx: &egui::Context,
        full_rect: egui::Rect,
        zoom_pan: Option<(f32, egui::Vec2)>,
    ) {
        if self.fs_suppress_primary_until_release {
            return;
        }
        let primary_down = ctx.input(|i| i.pointer.primary_down());
        let primary_pressed = ctx.input(|i| i.pointer.primary_pressed());
        let primary_released = ctx.input(|i| i.pointer.primary_released());
        let pointer_pos = ctx.input(|i| i.pointer.hover_pos());
        let paint = self.conceal_paint_mode;
        let space_held = ctx.input(|i| i.key_down(egui::Key::Space));
        let modifiers = ctx.input(|i| i.modifiers);

        // パネル上のクリックはツール操作に使わない。ただし、画像上で進行中のドラッグを
        // パネル上で離した場合は state を片付けないとリーク (Codex P2 #6) する。
        // → release 検知時は通常 dispatch を通し、各 tool ハンドラが state リセットする。
        let panel_rect = self.conceal_panel_rect(full_rect);
        if let Some(pos) = pointer_pos {
            if panel_rect.contains(pos) && !primary_released {
                return;
            }
        }

        // Space+ドラッグ: 一時パン (Photoshop 流)
        let drawing_in_progress = self.conceal_last_paint_pos.is_some()
            || self.conceal_line_start.is_some()
            || self.conceal_shape_drag_start.is_some()
            || self.conceal_drag.is_some()
            || !self.conceal_lasso_points.is_empty();
        if space_held && !drawing_in_progress {
            if primary_pressed {
                if let Some(pos) = pointer_pos {
                    self.fs_pan_drag_start = Some((pos, self.fs_pan));
                }
            } else if primary_down {
                if let Some((start_pos, start_pan)) = self.fs_pan_drag_start {
                    if let Some(pos) = pointer_pos {
                        self.fs_pan = start_pan + (pos - start_pos);
                    }
                }
            }
            if primary_released {
                self.fs_pan_drag_start = None;
            }
            ctx.set_cursor_icon(if primary_down {
                egui::CursorIcon::Grabbing
            } else {
                egui::CursorIcon::Grab
            });
            return;
        }
        if !space_held && self.fs_pan_drag_start.is_some() {
            self.fs_pan_drag_start = None;
        }

        // Select 以外のツール開始時に選択を解除
        if self.conceal_tool != ConcealTool::Select
            && self.conceal_drag.is_none()
            && self.conceal_selected_shape.is_some()
        {
            self.conceal_selected_shape = None;
            self.conceal_mask_texture = None;
        }

        match self.conceal_tool {
            ConcealTool::Select => {
                self.handle_select_tool(
                    primary_pressed,
                    primary_released,
                    pointer_pos,
                    full_rect,
                    zoom_pan,
                    modifiers,
                );
            }
            ConcealTool::Brush => {
                self.handle_brush_tool(
                    primary_down,
                    primary_released,
                    pointer_pos,
                    paint,
                    full_rect,
                    zoom_pan,
                );
            }
            ConcealTool::Lasso => {
                self.handle_lasso_tool(
                    primary_down,
                    primary_released,
                    pointer_pos,
                    paint,
                    full_rect,
                    zoom_pan,
                );
            }
            ConcealTool::Line | ConcealTool::VertLine | ConcealTool::HorizLine => {
                self.handle_line_tool(
                    primary_down,
                    primary_released,
                    pointer_pos,
                    paint,
                    full_rect,
                    zoom_pan,
                );
            }
            ConcealTool::Rect | ConcealTool::Ellipse => {
                self.handle_rect_ellipse_tool(
                    primary_down,
                    primary_released,
                    pointer_pos,
                    paint,
                    full_rect,
                    zoom_pan,
                );
            }
        }
    }

    fn handle_select_tool(
        &mut self,
        primary_pressed: bool,
        primary_released: bool,
        pointer_pos: Option<egui::Pos2>,
        full_rect: egui::Rect,
        zoom_pan: Option<(f32, egui::Vec2)>,
        modifiers: egui::Modifiers,
    ) {
        let Some(screen) = pointer_pos else {
            // ポインタが取れない瞬間に release が来た場合、drag は次フレームに持ち越して
            // しまうので、ここで明示的に drag だけ片付ける (P2 #6 補強)。
            if primary_released && self.conceal_drag.is_some() {
                self.conceal_drag = None;
                self.conceal_mask_texture = None;
            }
            return;
        };
        let Some((scale, img_rect)) = self.conceal_image_layout(full_rect, zoom_pan) else {
            return;
        };
        let img_pos = (
            (screen.x - img_rect.min.x) / scale,
            (screen.y - img_rect.min.y) / scale,
        );

        if primary_pressed {
            if let Some((idx, target)) = self.hit_test_conceal(img_pos, scale) {
                self.push_conceal_undo();
                self.conceal_selected_shape = Some(idx);
                let base = self.conceal_shapes[idx];
                self.conceal_drag = Some(vector_edit::begin_drag(target, idx, base, img_pos));
                // ドラッグ開始時に 1 回だけマスクテクスチャを invalidate (handle 描画は
                // 即時更新されるが、ベース mask が変わったことを反映するため)。
                self.conceal_mask_texture = None;
            } else if self.conceal_selected_shape.is_some() {
                self.conceal_selected_shape = None;
                self.conceal_drag = None;
                self.conceal_mask_texture = None;
            }
        }

        if let Some(drag) = self.conceal_drag {
            let new_shape = vector_edit::apply_drag(&drag, img_pos, &modifiers);
            if drag.idx() < self.conceal_shapes.len() {
                self.conceal_shapes[drag.idx()] = new_shape;
                // ドラッグ中はマスクテクスチャを invalidate しない (Codex P2 #8)。
                // 4K 画像で毎フレーム 32MB の RGBA バッファ生成 + GPU upload が走るのを
                // 避ける。ユーザーには handle の bbox 描画でリアルタイムフィードバックが
                // 出ているので、最終 mask テクスチャは release 後の 1 回更新で十分。
            }
            if primary_released {
                self.conceal_drag = None;
                // ドラッグ完了 → 最終形状を反映するためにマスクテクスチャ再生成。
                self.conceal_mask_texture = None;
            }
        }
    }

    fn handle_brush_tool(
        &mut self,
        primary_down: bool,
        primary_released: bool,
        pointer_pos: Option<egui::Pos2>,
        paint: bool,
        full_rect: egui::Rect,
        zoom_pan: Option<(f32, egui::Vec2)>,
    ) {
        if primary_down {
            if let Some(pos) = pointer_pos {
                if let Some(img_pos) = self.conceal_screen_to_image(pos, full_rect, zoom_pan, true)
                {
                    if self.conceal_last_paint_pos.is_none() {
                        self.push_conceal_undo();
                    }
                    let prev = self
                        .conceal_last_paint_pos
                        .and_then(|p| self.conceal_screen_to_image(p, full_rect, zoom_pan, false))
                        .unwrap_or(img_pos);
                    self.paint_brush_line_conceal(prev, img_pos, paint);
                }
                self.conceal_last_paint_pos = Some(pos);
            }
        }
        if primary_released {
            self.conceal_last_paint_pos = None;
        }
    }

    fn handle_lasso_tool(
        &mut self,
        primary_down: bool,
        primary_released: bool,
        pointer_pos: Option<egui::Pos2>,
        paint: bool,
        full_rect: egui::Rect,
        zoom_pan: Option<(f32, egui::Vec2)>,
    ) {
        if primary_down {
            if let Some(pos) = pointer_pos {
                if let Some(img_pos) = self.conceal_screen_to_image(pos, full_rect, zoom_pan, true)
                {
                    // サンプリング間引き (~2px 離れたら追加)
                    if self
                        .conceal_lasso_points
                        .last()
                        .map(|&(lx, ly)| {
                            let dx = lx - img_pos.0;
                            let dy = ly - img_pos.1;
                            dx * dx + dy * dy > 4.0
                        })
                        .unwrap_or(true)
                    {
                        self.conceal_lasso_points.push(img_pos);
                    }
                }
            }
        }
        if primary_released {
            if self.conceal_lasso_points.len() >= 3 {
                self.push_conceal_undo();
                let pts: Vec<(f32, f32)> = self.conceal_lasso_points.drain(..).collect();
                self.paint_polygon_conceal(&pts, paint);
            } else {
                self.conceal_lasso_points.clear();
            }
        }
    }

    /// 直線 / 縦線 / 横線ツール共通。Line は始点 → 終点をそのまま、
    /// VertLine / HorizLine は始点 → 終点の bbox から軸並行な線を作る。
    fn handle_line_tool(
        &mut self,
        primary_down: bool,
        primary_released: bool,
        pointer_pos: Option<egui::Pos2>,
        paint: bool,
        full_rect: egui::Rect,
        zoom_pan: Option<(f32, egui::Vec2)>,
    ) {
        if primary_down {
            if let Some(pos) = pointer_pos {
                if let Some(img_pos) = self.conceal_screen_to_image(pos, full_rect, zoom_pan, false)
                {
                    if self.conceal_line_start.is_none() {
                        self.conceal_line_start = Some(img_pos);
                    }
                    self.conceal_line_end = Some(img_pos);
                }
            }
        }
        if primary_released {
            if let (Some(start), Some(end)) = (self.conceal_line_start, self.conceal_line_end) {
                let [w, h] = self.conceal_mask_size;
                let thickness = self.settings.conceal_line_width.max(1.0);
                self.push_conceal_undo();
                let shape = match self.conceal_tool {
                    ConcealTool::Line => Shape::Line {
                        kind: LineKind::Diagonal,
                        p0: start,
                        p1: end,
                        thickness,
                    },
                    ConcealTool::VertLine => {
                        let lx = start.0.min(end.0);
                        let rx = start.0.max(end.0);
                        let cx = (lx + rx) * 0.5;
                        let thick = (rx - lx).max(thickness);
                        Shape::Line {
                            kind: LineKind::Vertical,
                            p0: (cx, 0.0),
                            p1: (cx, h as f32),
                            thickness: thick,
                        }
                    }
                    ConcealTool::HorizLine => {
                        let ty = start.1.min(end.1);
                        let by = start.1.max(end.1);
                        let cy = (ty + by) * 0.5;
                        let thick = (by - ty).max(thickness);
                        Shape::Line {
                            kind: LineKind::Horizontal,
                            p0: (0.0, cy),
                            p1: (w as f32, cy),
                            thickness: thick,
                        }
                    }
                    _ => unreachable!(),
                };
                self.commit_conceal_shape(shape, paint);
            }
            self.conceal_line_start = None;
            self.conceal_line_end = None;
        }
    }

    /// 矩形 / 楕円ツール: 始点 → 終点の bbox で Shape::Rect / Shape::Ellipse を作る。
    fn handle_rect_ellipse_tool(
        &mut self,
        primary_down: bool,
        primary_released: bool,
        pointer_pos: Option<egui::Pos2>,
        paint: bool,
        full_rect: egui::Rect,
        zoom_pan: Option<(f32, egui::Vec2)>,
    ) {
        if primary_down {
            if let Some(pos) = pointer_pos {
                if let Some(img_pos) = self.conceal_screen_to_image(pos, full_rect, zoom_pan, false)
                {
                    if self.conceal_shape_drag_start.is_none() {
                        self.conceal_shape_drag_start = Some(img_pos);
                    }
                    self.conceal_shape_drag_end = Some(img_pos);
                }
            }
        }
        if primary_released {
            if let (Some(start), Some(end)) =
                (self.conceal_shape_drag_start, self.conceal_shape_drag_end)
            {
                let dx = end.0 - start.0;
                let dy = end.1 - start.1;
                if dx.abs() > 1.0 && dy.abs() > 1.0 {
                    self.push_conceal_undo();
                    let cx = (start.0 + end.0) * 0.5;
                    let cy = (start.1 + end.1) * 0.5;
                    let hw = dx.abs() * 0.5;
                    let hh = dy.abs() * 0.5;
                    let shape = match self.conceal_tool {
                        ConcealTool::Rect => Shape::Rect {
                            center: (cx, cy),
                            half_w: hw,
                            half_h: hh,
                            rotation_rad: 0.0,
                        },
                        ConcealTool::Ellipse => Shape::Ellipse {
                            center: (cx, cy),
                            rx: hw,
                            ry: hh,
                            rotation_rad: 0.0,
                        },
                        _ => unreachable!(),
                    };
                    self.commit_conceal_shape(shape, paint);
                }
            }
            self.conceal_shape_drag_start = None;
            self.conceal_shape_drag_end = None;
        }
    }

    /// 描画モードならベクタ追加、消去モードなら重なる shape を削除しビットマップも消す。
    fn commit_conceal_shape(&mut self, shape: Shape, paint: bool) {
        if paint {
            self.conceal_shapes.push(shape);
        } else {
            // 消去: 重なる shape を削除し、ビットマップも shape 範囲で消す
            let [w, h] = self.conceal_mask_size;
            let (cx_min, cy_min, cx_max, cy_max) = shape_bbox(&shape);
            self.conceal_shapes.retain(|s| {
                let (sxmin, symin, sxmax, symax) = shape_bbox(s);
                !(sxmax < cx_min || sxmin > cx_max || symax < cy_min || symin > cy_max)
            });
            if let Some(mask) = self.conceal_mask.as_mut() {
                crate::mask_db::rasterize_shape_into(mask, &shape, w, h, false);
            }
        }
        self.conceal_mask_texture = None;
    }

    // ── 描画 ──────────────────────────────────────────────────────

    /// マスクオーバーレイ + ハンドル + ツールプレビュー + パネルを描画する。
    pub(crate) fn draw_conceal_overlay(
        &mut self,
        ui: &mut egui::Ui,
        ctx: &egui::Context,
        full_rect: egui::Rect,
        zoom_pan: Option<(f32, egui::Vec2)>,
    ) {
        // マスクオーバーレイ (紫半透明)
        self.ensure_conceal_mask_texture(ctx);
        if let Some(ref tex) = self.conceal_mask_texture {
            if let Some((_total_scale, img_rect)) = self.conceal_image_layout(full_rect, zoom_pan) {
                let painter = if zoom_pan.is_some() {
                    ui.painter().with_clip_rect(full_rect)
                } else {
                    ui.painter().clone()
                };
                painter.image(
                    tex.id(),
                    img_rect,
                    egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
                    egui::Color32::WHITE,
                );
            }
        }

        // ツールプレビュー (ドラッグ中)
        self.draw_conceal_tool_preview(ui, full_rect, zoom_pan);

        // カーソルを Crosshair に。`draw_selected_handles` 内でハンドル上にホバー時のみ
        // 専用カーソル (Resize* / PointingHand) へ上書きする。順序を逆にしないこと
        // (Codex P2 #5: 旧版は Crosshair が後勝ちでハンドルカーソルが見えなかった)。
        ctx.output_mut(|o| o.cursor_icon = egui::CursorIcon::Crosshair);

        // 選択中の shape のハンドル
        self.draw_selected_handles(ui, ctx, full_rect, zoom_pan);

        // パネル
        self.draw_conceal_panel(ctx, full_rect);
    }

    fn draw_conceal_tool_preview(
        &self,
        ui: &mut egui::Ui,
        full_rect: egui::Rect,
        zoom_pan: Option<(f32, egui::Vec2)>,
    ) {
        let painter = if zoom_pan.is_some() {
            ui.painter().with_clip_rect(full_rect)
        } else {
            ui.painter().clone()
        };
        let preview_color = egui::Color32::from_rgba_unmultiplied(
            MASK_OVERLAY_R,
            MASK_OVERLAY_G,
            MASK_OVERLAY_B,
            200,
        );
        let stroke = egui::Stroke::new(1.5, preview_color);

        // Lasso 多角形ライン
        if self.conceal_lasso_points.len() >= 2 {
            let pts: Vec<egui::Pos2> = self
                .conceal_lasso_points
                .iter()
                .map(|&p| self.conceal_image_to_screen(p, full_rect, zoom_pan))
                .collect();
            painter.add(egui::Shape::line(pts, stroke));
        }
        // Line / VertLine / HorizLine プレビュー
        if let (Some(start), Some(end)) = (self.conceal_line_start, self.conceal_line_end) {
            let [w, h] = self.conceal_mask_size;
            let (p0, p1) = match self.conceal_tool {
                ConcealTool::VertLine => {
                    let lx = start.0.min(end.0);
                    let rx = start.0.max(end.0);
                    let cx = (lx + rx) * 0.5;
                    ((cx, 0.0), (cx, h as f32))
                }
                ConcealTool::HorizLine => {
                    let ty = start.1.min(end.1);
                    let by = start.1.max(end.1);
                    let cy = (ty + by) * 0.5;
                    ((0.0, cy), (w as f32, cy))
                }
                _ => (start, end),
            };
            let s0 = self.conceal_image_to_screen(p0, full_rect, zoom_pan);
            let s1 = self.conceal_image_to_screen(p1, full_rect, zoom_pan);
            painter.line_segment([s0, s1], stroke);
        }
        // Rect / Ellipse プレビュー
        if let (Some(start), Some(end)) =
            (self.conceal_shape_drag_start, self.conceal_shape_drag_end)
        {
            let s0 = self.conceal_image_to_screen(start, full_rect, zoom_pan);
            let s1 = self.conceal_image_to_screen(end, full_rect, zoom_pan);
            let rect = egui::Rect::from_two_pos(s0, s1);
            match self.conceal_tool {
                ConcealTool::Rect => {
                    painter.rect_stroke(rect, 0.0, stroke, egui::StrokeKind::Inside);
                }
                ConcealTool::Ellipse => {
                    // egui に楕円 stroke が無いので円周点列で代用
                    let center = rect.center();
                    let r = egui::vec2(rect.width() * 0.5, rect.height() * 0.5);
                    const N: usize = 36;
                    let mut pts = Vec::with_capacity(N + 1);
                    for i in 0..=N {
                        let theta = i as f32 * std::f32::consts::TAU / N as f32;
                        pts.push(egui::pos2(
                            center.x + r.x * theta.cos(),
                            center.y + r.y * theta.sin(),
                        ));
                    }
                    painter.add(egui::Shape::line(pts, stroke));
                }
                _ => {}
            }
        }
    }

    fn draw_selected_handles(
        &self,
        ui: &mut egui::Ui,
        _ctx: &egui::Context,
        full_rect: egui::Rect,
        zoom_pan: Option<(f32, egui::Vec2)>,
    ) {
        let Some(sel) = self.conceal_selected_shape else {
            return;
        };
        let Some(shape) = self.conceal_shapes.get(sel) else {
            return;
        };
        let Some((scale, img_rect)) = self.conceal_image_layout(full_rect, zoom_pan) else {
            return;
        };
        let layout = vector_edit::compute_handle_layout(shape, scale);
        let painter = ui.painter().with_clip_rect(full_rect);

        // ホバー判定はカーソル位置を画像座標に変換してから
        let hovered = ui.ctx().input(|i| i.pointer.hover_pos()).and_then(|p| {
            let img_pos = (
                (p.x - img_rect.min.x) / scale,
                (p.y - img_rect.min.y) / scale,
            );
            vector_edit::hit_test(&layout, img_pos, scale)
        });

        // カーソル変更 (本体ヒット以外で意味あり)
        if let Some(h) = hovered {
            if !matches!(h, vector_edit::HoverTarget::Body) {
                ui.ctx()
                    .set_cursor_icon(vector_edit::cursor_icon_for(h, shape));
            }
        }

        let to_screen = |p: (f32, f32)| self.conceal_image_to_screen(p, full_rect, zoom_pan);
        vector_edit::draw_handles(&painter, &layout, true, hovered, &to_screen);
    }

    // ── ツールパネル ──────────────────────────────────────────────

    /// ツールパネルの矩形を返す。
    fn conceal_panel_rect(&self, full_rect: egui::Rect) -> egui::Rect {
        let panel_pos = egui::pos2(
            full_rect.min.x + PANEL_MARGIN_X,
            full_rect.min.y + PANEL_MARGIN_Y,
        );
        // ヘッダ + ツール 8 個 (2 列 × 4 行) + 描画/消去 + サイズスライダ + ショートカット説明
        let h = 360.0
            + if self.conceal_tool == ConcealTool::Brush
                || matches!(
                    self.conceal_tool,
                    ConcealTool::Line | ConcealTool::VertLine | ConcealTool::HorizLine
                )
            {
                42.0
            } else {
                0.0
            };
        egui::Rect::from_min_size(panel_pos, egui::vec2(PANEL_W, h))
    }

    /// 隠蔽加工モードの左パネル。Phase 2 では 8 ツール + 描画/消去 + サイズスライダを描く。
    pub(crate) fn draw_conceal_panel(&mut self, ctx: &egui::Context, full_rect: egui::Rect) {
        if !self.conceal_mode {
            return;
        }
        let panel_pos = egui::pos2(
            full_rect.min.x + PANEL_MARGIN_X,
            full_rect.min.y + PANEL_MARGIN_Y,
        );

        egui::Area::new(egui::Id::new("conceal_panel"))
            .fixed_pos(panel_pos)
            .order(egui::Order::Foreground)
            .interactable(true)
            .show(ctx, |ui| {
                // Frame の背景色とテキスト色を明示的に設定し、消しゴムパネル
                // ([`crate::ui_erase::draw_erase_panel`]) と同じ視認性に揃える
                // (egui::Frame::popup のデフォルトでは Yu Gothic + dark visuals で
                // 文字が読みにくい問題、Phase 4 ユーザー報告)。
                egui::Frame::popup(ui.style())
                    .fill(egui::Color32::from_rgba_unmultiplied(20, 20, 20, 220))
                    .stroke(egui::Stroke::new(
                        1.0,
                        egui::Color32::from_rgba_unmultiplied(255, 255, 255, 40),
                    ))
                    .corner_radius(6.0)
                    .show(ui, |ui| {
                        ui.set_min_width(PANEL_W);
                        // 子ウィジェット (ボタン / スライダー / ラベル) のテキスト色を
                        // 強制的に白にする。これをやらないとデフォルト dark visuals の
                        // widgets.* 派生色 (グレー 100 前後) で描画されて読みにくい。
                        ui.style_mut().visuals.override_text_color = Some(egui::Color32::WHITE);

                        // ── ヘッダ (消しゴムパネルの「消しゴム」と同じスタイル) ──
                        ui.label(
                            egui::RichText::new("隠蔽加工")
                                .size(15.0)
                                .strong()
                                .color(egui::Color32::WHITE),
                        );
                        ui.add_space(2.0);
                        ui.label(
                            egui::RichText::new(format!(
                                "処理: {}",
                                self.settings.conceal_type.label()
                            ))
                            .color(egui::Color32::from_gray(200)),
                        );
                        ui.separator();

                        // 描画 / 消去
                        ui.horizontal(|ui| {
                            ui.selectable_value(&mut self.conceal_paint_mode, true, "描画 [D]");
                            ui.selectable_value(&mut self.conceal_paint_mode, false, "消去 [F]");
                        });
                        ui.separator();

                        // ツールパレット (8 個、2 列 × 4 行)
                        ui.label(
                            egui::RichText::new("ツール:").color(egui::Color32::from_gray(200)),
                        );
                        let mut tool = self.conceal_tool;
                        ui.horizontal(|ui| {
                            ui.selectable_value(&mut tool, ConcealTool::Select, "選 [S]");
                            ui.selectable_value(&mut tool, ConcealTool::Brush, "筆 [B]");
                        });
                        ui.horizontal(|ui| {
                            ui.selectable_value(&mut tool, ConcealTool::Lasso, "囲 [L]");
                            ui.selectable_value(&mut tool, ConcealTool::Line, "直 [I]");
                        });
                        ui.horizontal(|ui| {
                            ui.selectable_value(&mut tool, ConcealTool::VertLine, "縦 [V]");
                            ui.selectable_value(&mut tool, ConcealTool::HorizLine, "横 [H]");
                        });
                        ui.horizontal(|ui| {
                            ui.selectable_value(&mut tool, ConcealTool::Rect, "矩 [R]");
                            ui.selectable_value(&mut tool, ConcealTool::Ellipse, "楕 [O]");
                        });
                        if tool != self.conceal_tool {
                            self.conceal_tool = tool;
                            self.conceal_drag = None;
                            self.conceal_lasso_points.clear();
                            self.conceal_line_start = None;
                            self.conceal_line_end = None;
                            self.conceal_shape_drag_start = None;
                            self.conceal_shape_drag_end = None;
                        }

                        // サイズスライダ (Brush は半径、Line 系は太さ)
                        let [w, h] = self.conceal_mask_size;
                        let long_edge = w.max(h).max(1) as f32;
                        match self.conceal_tool {
                            ConcealTool::Brush => {
                                ui.separator();
                                let mut r = self.settings.conceal_brush_radius;
                                ui.add(
                                    egui::Slider::new(&mut r, 1.0..=long_edge / 5.0)
                                        .text("ブラシ半径"),
                                );
                                self.settings.conceal_brush_radius = r;
                            }
                            ConcealTool::Line | ConcealTool::VertLine | ConcealTool::HorizLine => {
                                ui.separator();
                                let mut t = self.settings.conceal_line_width;
                                ui.add(
                                    egui::Slider::new(&mut t, 1.0..=long_edge / 5.0).text("線幅"),
                                );
                                self.settings.conceal_line_width = t;
                            }
                            _ => {}
                        }

                        // ── 隠蔽タイプ + パラメータ (Phase 4) ─────────────────
                        ui.separator();
                        ui.label(
                            egui::RichText::new("処理タイプ [T]:")
                                .color(egui::Color32::from_gray(200)),
                        );
                        let mut type_changed = false;
                        let mut new_type = self.settings.conceal_type;
                        ui.horizontal(|ui| {
                            for t in [
                                crate::conceal::ConcealType::Mosaic,
                                crate::conceal::ConcealType::WhiteFill,
                            ] {
                                if ui.selectable_label(new_type == t, t.label()).clicked() {
                                    new_type = t;
                                }
                            }
                        });
                        ui.horizontal(|ui| {
                            for t in [
                                crate::conceal::ConcealType::BlackFill,
                                crate::conceal::ConcealType::Blur,
                            ] {
                                if ui.selectable_label(new_type == t, t.label()).clicked() {
                                    new_type = t;
                                }
                            }
                        });
                        if new_type != self.settings.conceal_type {
                            self.settings.conceal_type = new_type;
                            type_changed = true;
                        }

                        // Mosaic パラメータ (Phase 3a のみ実装、Fill/Blur は Phase 3b/3c)
                        if matches!(
                            self.settings.conceal_type,
                            crate::conceal::ConcealType::Mosaic
                        ) {
                            ui.separator();
                            ui.label(
                                egui::RichText::new("タイルサイズ:")
                                    .color(egui::Color32::from_gray(200)),
                            );
                            let mut tile_mode = self.settings.conceal_mosaic_tile_mode;
                            let mut tile_changed = false;
                            let is_ratio =
                                matches!(tile_mode, crate::conceal::TileSizeMode::LongEdgeRatio(_));
                            ui.horizontal(|ui| {
                                if ui.selectable_label(is_ratio, "長辺比率").clicked() && !is_ratio
                                {
                                    tile_mode = crate::conceal::TileSizeMode::LongEdgeRatio(1.0);
                                    tile_changed = true;
                                }
                                if ui.selectable_label(!is_ratio, "固定 px").clicked() && is_ratio
                                {
                                    tile_mode = crate::conceal::TileSizeMode::FixedPx(16);
                                    tile_changed = true;
                                }
                            });
                            match tile_mode {
                                crate::conceal::TileSizeMode::LongEdgeRatio(mut m) => {
                                    let prev = m;
                                    ui.add(
                                        egui::Slider::new(
                                            &mut m,
                                            crate::conceal::TILE_RATIO_MIN
                                                ..=crate::conceal::TILE_RATIO_MAX,
                                        )
                                        .step_by(crate::conceal::TILE_RATIO_STEP as f64)
                                        .text("倍率"),
                                    );
                                    if (m - prev).abs() > 1e-6 {
                                        tile_mode = crate::conceal::TileSizeMode::LongEdgeRatio(m);
                                        tile_changed = true;
                                    }
                                    let long_edge_u = w.max(h) as u32;
                                    let tile_px =
                                        crate::conceal::compute_tile_size(long_edge_u, tile_mode);
                                    ui.label(
                                        egui::RichText::new(format!(
                                            "= {}px @ {}px 長辺",
                                            tile_px, long_edge_u
                                        ))
                                        .size(10.0)
                                        .color(egui::Color32::from_gray(170)),
                                    );
                                }
                                crate::conceal::TileSizeMode::FixedPx(mut px) => {
                                    let prev = px;
                                    ui.add(
                                        egui::Slider::new(
                                            &mut px,
                                            crate::conceal::TILE_FIXED_MIN
                                                ..=crate::conceal::TILE_FIXED_MAX,
                                        )
                                        .text("px"),
                                    );
                                    if px != prev {
                                        tile_mode = crate::conceal::TileSizeMode::FixedPx(px);
                                        tile_changed = true;
                                    }
                                }
                            }
                            if tile_changed {
                                self.settings.conceal_mosaic_tile_mode = tile_mode;
                            }

                            ui.label(
                                egui::RichText::new("境界処理:")
                                    .color(egui::Color32::from_gray(200)),
                            );
                            let mut bnd = self.settings.conceal_mosaic_boundary;
                            let mut bnd_changed = false;
                            for b in [
                                crate::conceal::MosaicBoundary::Opaque,
                                crate::conceal::MosaicBoundary::Translucent,
                                crate::conceal::MosaicBoundary::MaskShape,
                            ] {
                                let label = egui::RichText::new(b.process_description()).size(11.0);
                                if ui.radio(bnd == b, label).clicked() {
                                    bnd = b;
                                    bnd_changed = true;
                                }
                            }
                            if bnd_changed {
                                self.settings.conceal_mosaic_boundary = bnd;
                            }

                            if tile_changed || bnd_changed || type_changed {
                                self.bump_conceal_generation();
                            }
                        } else if type_changed {
                            // Mosaic 以外でも type 切替なら世代 bump
                            self.bump_conceal_generation();
                            // Phase 3b/3c 実装待ち。一時的に Mosaic 合成にフォールバック
                            // (ensure_conceal_texture が Mosaic 設定で合成する)。
                            ui.colored_label(
                                egui::Color32::from_rgb(220, 180, 80),
                                "※ このタイプは未実装 (Mosaic にフォールバック)",
                            );
                        } else if !matches!(
                            self.settings.conceal_type,
                            crate::conceal::ConcealType::Mosaic
                        ) {
                            ui.colored_label(
                                egui::Color32::from_rgb(220, 180, 80),
                                "※ このタイプは未実装 (Mosaic にフォールバック)",
                            );
                        }

                        // ── プリセット 4 スロット (Phase 4) ───────────────────
                        ui.separator();
                        ui.label(
                            egui::RichText::new("プリセット (1-4 で適用):")
                                .color(egui::Color32::from_gray(200)),
                        );
                        for i in 0..4usize {
                            ui.horizontal(|ui| {
                                let label = match &self.settings.conceal_presets[i] {
                                    Some(p) if !p.name.is_empty() => p.name.clone(),
                                    Some(_) => format!("プリセット {}", i + 1),
                                    None => format!("(空) {}", i + 1),
                                };
                                let has = self.settings.conceal_presets[i].is_some();
                                if ui
                                    .add_enabled(
                                        has,
                                        egui::Button::new(label).min_size(egui::vec2(120.0, 0.0)),
                                    )
                                    .clicked()
                                {
                                    self.apply_conceal_preset(i);
                                }
                                if ui.small_button("保存").clicked() {
                                    self.save_conceal_preset_to_slot(i);
                                }
                            });
                        }

                        // ── マスクスロット (差分画像生成用、2 スロット) ──────
                        ui.separator();
                        ui.label(
                            egui::RichText::new("マスクスロット:")
                                .color(egui::Color32::from_gray(200)),
                        );
                        for slot in 1..=2usize {
                            ui.horizontal(|ui| {
                                ui.label(format!("スロット {}", slot));
                                if ui.small_button("保存").clicked() {
                                    self.save_conceal_mask_to_slot(slot);
                                }
                                if ui.small_button("ロード").clicked() {
                                    self.load_conceal_mask_from_slot(slot);
                                }
                            });
                        }

                        // ── マスク全削除 ───────────────────────────────────
                        ui.separator();
                        if ui
                            .add(
                                egui::Button::new(
                                    egui::RichText::new("マスク全削除").color(egui::Color32::WHITE),
                                )
                                .fill(egui::Color32::from_rgb(120, 50, 50)),
                            )
                            .clicked()
                        {
                            self.delete_all_conceal_mask();
                        }

                        ui.separator();
                        ui.label(
                            egui::RichText::new(
                                "Shift+ハンドル: 軸拘束 / 等比 / 15°snap\n\
                                 Alt+ハンドル: 中心固定\n\
                                 T: タイプ切替  1-4: プリセット適用\n\
                                 Ctrl+Z: 元に戻す  Delete: 選択削除\n\
                                 Esc / Ctrl+M: 終了 (DB 保存)",
                            )
                            .size(11.0)
                            .color(egui::Color32::from_gray(190)),
                        );
                    });
            });
    }
}

// ── 純関数ヘルパー ────────────────────────────────────────────────

/// 多角形内判定 (奇数交差判定、本体ヒット用)。`vector_edit::point_in_polygon` は
/// private なのでここに同等版を置く。
fn point_in_polygon_local(p: (f32, f32), poly: &[(f32, f32)]) -> bool {
    let n = poly.len();
    if n < 3 {
        return false;
    }
    let mut inside = false;
    let mut j = n - 1;
    for i in 0..n {
        let (xi, yi) = poly[i];
        let (xj, yj) = poly[j];
        if (yi > p.1) != (yj > p.1) {
            let x_intersect = (xj - xi) * (p.1 - yi) / (yj - yi + 1e-9) + xi;
            if p.0 < x_intersect {
                inside = !inside;
            }
        }
        j = i;
    }
    inside
}

/// shape の AABB (回転考慮後の 4 隅から取る)。
fn shape_bbox(shape: &Shape) -> (f32, f32, f32, f32) {
    let corners = match shape {
        Shape::Line {
            p0, p1, thickness, ..
        } => {
            // Line の corners (中心軸 + 太さ) を line_corners と同じロジックで取る
            let dx = p1.0 - p0.0;
            let dy = p1.1 - p0.1;
            let len = (dx * dx + dy * dy).sqrt().max(1e-6);
            let nx = -dy / len;
            let ny = dx / len;
            let half = (thickness * 0.5).max(0.0);
            [
                (p0.0 + nx * half, p0.1 + ny * half),
                (p1.0 + nx * half, p1.1 + ny * half),
                (p1.0 - nx * half, p1.1 - ny * half),
                (p0.0 - nx * half, p0.1 - ny * half),
            ]
        }
        Shape::Rect {
            center,
            half_w,
            half_h,
            rotation_rad,
        }
        | Shape::Ellipse {
            center,
            rx: half_w,
            ry: half_h,
            rotation_rad,
        } => crate::mask_db::rect_corners(*center, *half_w, *half_h, *rotation_rad),
    };
    let mut min_x = f32::MAX;
    let mut min_y = f32::MAX;
    let mut max_x = f32::MIN;
    let mut max_y = f32::MIN;
    for &(x, y) in &corners {
        min_x = min_x.min(x);
        min_y = min_y.min(y);
        max_x = max_x.max(x);
        max_y = max_y.max(y);
    }
    (min_x, min_y, max_x, max_y)
}
