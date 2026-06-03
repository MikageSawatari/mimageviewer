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

use crate::app::{App, ConcealSnapshot, EraseSpreadCtx, MaskDirtyRect};
use crate::conceal::ConcealTool;
use crate::mask_db::{LineKind, Shape, ShapeOp};
use crate::ui_fullscreen::FsKeyAction;
use crate::ui_fullscreen::draw_icons::{PanelToggleColors, panel_toggle_button};
use crate::vector_edit;

// ── 定数 ────────────────────────────────────────────────────────────────

/// ツールパネルの幅。消しゴムパネル ([`crate::ui_erase::PANEL_W`]) と統一して
/// 200px。実機 FB R4 で「マスク全削除ボタンの幅を揃えたい」要望に対応。
const PANEL_W: f32 = 200.0;
/// ツールパネルの左上マージン。
const PANEL_MARGIN_X: f32 = 16.0;
const PANEL_MARGIN_Y: f32 = 60.0;
/// パネル下端をウィンドウ下端から少し浮かせる余白。
const PANEL_BOTTOM_MARGIN: f32 = 20.0;
/// ScrollArea の最低高さ。極端に低いウィンドウでも操作領域を潰しすぎない。
const PANEL_MIN_BODY_H: f32 = 120.0;

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

fn conceal_panel_outer_height(full_rect: egui::Rect, panel_pos: egui::Pos2) -> f32 {
    (full_rect.max.y - panel_pos.y - PANEL_BOTTOM_MARGIN).max(PANEL_MIN_BODY_H + 40.0)
}

impl App {
    /// 「フルスクリーン上の主要 UI (メタデータパネル / 上部ホバーバー / カーソル自動隠し /
    /// マウスホイールでのページ送り 等) を抑制すべき編集モード」が現在 active か判定する
    /// (= 消しゴム / 補正レイヤー / 隠蔽加工)。Phase 4 で追加。
    ///
    /// 既存コード (ui_fullscreen.rs) に散らばっている `!self.erase_mode` 判定の多くは
    /// 「左側の編集パネル中の UI を邪魔するな」という同じ意図なので、本ヘルパーで統一する。
    /// 将来別の overlay edit mode (例: 切り抜き / ワイヤフレーム) を追加するときも
    /// この 1 箇所を拡張するだけで済む。
    pub(crate) fn is_overlay_edit_mode_active(&self) -> bool {
        self.erase_mode || self.local_adjust_mode || self.conceal_mode
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

        // 元画像取得 (state mutation 前)。プレビュー合成と同じ優先順位
        // (erase_result > adjustment > ai_upscale > fs) で source を決める。
        let (pixels, source_kind) = match self.current_conceal_source_pixels(target_idx) {
            Some((p, kind)) => {
                self.conceal_base_cache.insert(target_idx, Arc::clone(&p));
                (p, kind)
            }
            None => {
                crate::logger::log("conceal: enter aborted (no base pixels)".to_string());
                return;
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
        self.conceal_preview_active = false;
        self.clear_meta_undo();
        if !self.post_filter_bypassed {
            self.post_filter_bypassed = true;
            if self.effective_params(fs_idx).post_filter != crate::adjustment::PostFilter::None {
                self.clear_adjustment_render_caches_for_bypass(fs_idx);
            }
        }

        self.conceal_mask_size = [w, h];
        self.conceal_mask_texture = None;
        self.conceal_mask_texture_dirty_rect = None;
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
            "conceal: enter mode, source={source_kind}, image={w}x{h}, shapes={}, type={:?}, tool={:?}",
            self.conceal_shapes.len(),
            self.settings.conceal_type,
            self.conceal_tool,
        ));
    }

    fn finish_conceal_edit_for_current(&mut self, reason: &'static str) {
        let Some(idx) = self.fullscreen_idx else {
            return;
        };
        // キーボードやページ切替で mid-drag 終了された場合も、保存前の解像度同期を
        // ブロックしないよう中間状態だけ先に畳む。未確定の線/矩形/lasso は
        // 通常の release 前キャンセルとして破棄される。
        self.conceal_drag = None;
        self.conceal_last_paint_pos = None;
        self.conceal_lasso_points.clear();
        self.conceal_line_start = None;
        self.conceal_line_end = None;
        self.conceal_shape_drag_start = None;
        self.conceal_shape_drag_end = None;
        if let Some((pixels, _)) = self.current_conceal_source_pixels(idx) {
            self.rescale_active_conceal_edit_to_size(idx, pixels.size, reason);
        }
        let [w, h] = self.conceal_mask_size;
        if let Some(mask) = self.conceal_mask.clone() {
            let shapes = self.conceal_shapes.clone();
            self.save_conceal_with_sidecar(idx, &mask, &shapes, w, h);
        }
        // 編集中のマスクが新しい形状になったので、`conceal_cache[idx]` を
        // 破棄して退場後の最初の表示パスで再合成させる (Phase 4)。
        self.clear_conceal_caches(idx);
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
            self.finish_conceal_edit_for_current("exit_save");
        }

        self.conceal_mode = false;
        self.conceal_preview_active = false;

        if was_conceal_mode {
            self.clear_meta_undo();
        }

        // post-filter バイパス解除 (analysis_mode が同時にアクティブでない場合)
        if self.post_filter_bypassed && !self.analysis_mode {
            let needs_post_filter_restore = restore_idx
                .map(|idx| {
                    self.effective_params(idx).post_filter != crate::adjustment::PostFilter::None
                })
                .unwrap_or(false);
            self.post_filter_bypassed = false;
            if needs_post_filter_restore {
                if let Some(idx) = restore_idx {
                    self.clear_adjustment_render_caches_for_bypass(idx);
                }
            }
        }

        self.conceal_mask = None;
        self.conceal_mask_size = [0, 0];
        self.conceal_mask_texture = None;
        self.conceal_mask_texture_dirty_rect = None;
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

    /// 見開き隠蔽加工中に「左ページ」「右ページ」ボタンで編集対象を切り替える。
    /// 現ページのマスクを保存してから単ページ状態のままもう一方へ入り直す。
    pub(crate) fn switch_conceal_target_in_spread(&mut self, new_idx: usize) {
        if self.fullscreen_idx == Some(new_idx) {
            return;
        }
        self.finish_conceal_edit_for_current("switch_save");
        self.spread_mode = crate::settings::SpreadMode::Single;
        self.fullscreen_idx = Some(new_idx);
        self.fs_zoom = 1.0;
        self.fs_pan = egui::Vec2::ZERO;
        self.enter_conceal_mode(new_idx);
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
            // mask / shapes が変わったので合成 cache を失効させる。
            if let Some(fs_idx) = self.fullscreen_idx {
                self.clear_conceal_caches(fs_idx);
            }
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
        // mask / shapes が差し替わったので合成 cache を失効させる。
        if let Some(fs_idx) = self.fullscreen_idx {
            self.clear_conceal_caches(fs_idx);
        }
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
            sibling_nav: None,
            jump_to: None,
        };

        // ESC: 選択中があればまず解除、無ければモード終了
        let esc = ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Escape));
        if esc {
            if self.conceal_tool == ConcealTool::Polygon && !self.conceal_lasso_points.is_empty() {
                self.conceal_lasso_points.clear();
                self.show_feedback_toast("[多角形をキャンセル]".to_string());
                return action;
            }
            if self.conceal_selected_shape.is_some() {
                self.conceal_selected_shape = None;
                self.conceal_drag = None;
                return action;
            }
            self.reset_conceal_mode();
            return action;
        }

        // Enter: 多角形ツールの頂点列を確定。
        let enter = ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Enter));
        if enter
            && self.conceal_tool == ConcealTool::Polygon
            && let Some(pts) =
                crate::manual_mask_tools::take_completed_polygon(&mut self.conceal_lasso_points)
        {
            self.push_conceal_undo();
            self.paint_polygon_conceal(&pts, self.conceal_paint_mode);
            self.show_feedback_toast("[多角形を確定]".to_string());
            return action;
        }

        // Ctrl+M: モード終了 (再押下で抜ける、ui_fullscreen から委譲済み判定用)
        let ctrl_m = ctx.input_mut(|i| i.consume_key(egui::Modifiers::CTRL, egui::Key::M));
        if ctrl_m {
            self.reset_conceal_mode();
            return action;
        }

        // G: 通常フルスクリーンと同じピクセル境界グリッドの表示切替。
        let key_g = ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::G));
        if key_g {
            self.fs_pixel_grid_enabled = !self.fs_pixel_grid_enabled;
            self.show_feedback_toast(if self.fs_pixel_grid_enabled {
                "[ピクセルグリッド ON]".to_string()
            } else {
                "[ピクセルグリッド OFF]".to_string()
            });
        }

        // Ctrl+Z: Undo
        let ctrl_z = ctx.input_mut(|i| i.consume_key(egui::Modifiers::CTRL, egui::Key::Z));
        if ctrl_z {
            if self.conceal_tool == ConcealTool::Polygon
                && self.conceal_lasso_points.pop().is_some()
            {
                self.show_feedback_toast("[頂点を戻す]".to_string());
                return action;
            }
            if self.undo_conceal() {
                self.show_feedback_toast("[元に戻す]".to_string());
            } else {
                self.show_feedback_toast("[履歴なし]".to_string());
            }
        }

        // Delete: 選択中の Shape を削除
        let key_del = ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Delete));
        if key_del
            && let Some(idx) = self.conceal_selected_shape
            && idx < self.conceal_shapes.len()
        {
            self.push_conceal_undo();
            self.conceal_shapes.remove(idx);
            self.conceal_selected_shape = None;
            self.conceal_drag = None;
            self.conceal_mask_texture = None;
            // shape 構成が変わったので合成 cache を失効させる (Codex P2 R3 #1)。
            if let Some(fs_idx) = self.fullscreen_idx {
                self.clear_conceal_caches(fs_idx);
            }
            self.show_feedback_toast("[ベクタ削除]".to_string());
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

        // ツール切替: S/B/L/P/I/V/H/R/O
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
            if i.consume_key(egui::Modifiers::NONE, egui::Key::P) {
                switched = Some(ConcealTool::Polygon);
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
        if let Some(tool) = switched
            && tool != self.conceal_tool
        {
            let entering_select = tool == ConcealTool::Select;
            self.conceal_tool = tool;
            self.conceal_drag = None;
            self.conceal_lasso_points.clear();
            self.conceal_line_start = None;
            self.conceal_line_end = None;
            self.conceal_shape_drag_start = None;
            self.conceal_shape_drag_end = None;
            // ツール切替時は選択も解除 (= 別ツールに移ったので前の shape の
            // ハンドル編集は終了とみなす、Codex P1 対応)。
            // ただし **Select に入る場合は保持** (commit_conceal_shape が auto-select
            // した shape を [S] でそのまま編集できるようにする、code-review CONFIRMED)。
            if !entering_select {
                self.conceal_selected_shape = None;
            }
            self.conceal_mask_texture = None;
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
        // shape の位置が変わったので合成 cache も失効させる (Codex P2 R3 #1)。
        if let Some(fs_idx) = self.fullscreen_idx {
            self.clear_conceal_caches(fs_idx);
        }
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
        let dirty = crate::mask_db::brush_line_bbox(w, h, from, to, radius)
            .and_then(|(x0, y0, x1, y1)| MaskDirtyRect::new(x0, y0, x1, y1));
        if crate::mask_db::paint_brush_line_bitmap(mask, w, h, from, to, radius, paint) {
            self.mark_conceal_mask_texture_dirty(dirty);
            // brush もマスクを変えるので conceal_cache 失効が必要 (= preview 時に
            // 最新マスクで再合成される、commit_conceal_shape と同じ理由)。
            if let Some(fs_idx) = self.fullscreen_idx {
                self.clear_conceal_caches(fs_idx);
            }
        }
    }

    fn paint_polygon_conceal(&mut self, points: &[(f32, f32)], paint: bool) {
        let [w, h] = self.conceal_mask_size;
        let Some(mask) = self.conceal_mask.as_mut() else {
            return;
        };
        crate::mask_db::scanline_fill_polygon(mask, points, w, h, paint);
        self.conceal_mask_texture = None;
        self.conceal_mask_texture_dirty_rect = None;
        // lasso もマスクを変えるので conceal_cache 失効が必要。
        if let Some(fs_idx) = self.fullscreen_idx {
            self.clear_conceal_caches(fs_idx);
        }
    }

    // ── マスクテクスチャ ──────────────────────────────────────────

    fn mark_conceal_mask_texture_dirty(&mut self, dirty: Option<MaskDirtyRect>) {
        match (self.conceal_mask_texture.is_some(), dirty) {
            (true, Some(rect)) => {
                self.conceal_mask_texture_dirty_rect = Some(
                    self.conceal_mask_texture_dirty_rect
                        .map_or(rect, |prev| prev.union(rect)),
                );
            }
            _ => {
                self.conceal_mask_texture = None;
                self.conceal_mask_texture_dirty_rect = None;
            }
        }
    }

    fn conceal_mask_region_image(&self, rect: MaskDirtyRect) -> Option<egui::ColorImage> {
        let mask = self.conceal_mask.as_ref()?;
        let [w, h] = self.conceal_mask_size;
        let composite = crate::mask_db::composite_mask_region(
            mask,
            &self.conceal_shapes,
            w,
            h,
            (rect.x0, rect.y0, rect.x1, rect.y1),
        )?;
        let [rw, rh] = rect.size();
        let mut rgba = vec![0u8; rw * rh * 4];
        for (i, masked) in composite.iter().copied().enumerate() {
            if masked {
                rgba[i * 4] = MASK_OVERLAY_R;
                rgba[i * 4 + 1] = MASK_OVERLAY_G;
                rgba[i * 4 + 2] = MASK_OVERLAY_B;
                rgba[i * 4 + 3] = MASK_OVERLAY_A;
            }
        }
        Some(egui::ColorImage::from_rgba_unmultiplied([rw, rh], &rgba))
    }

    fn ensure_conceal_mask_texture(&mut self, ctx: &egui::Context) {
        let [w, h] = self.conceal_mask_size;
        if self
            .conceal_mask_texture
            .as_ref()
            .is_some_and(|tex| tex.size() != [w, h])
        {
            self.conceal_mask_texture = None;
            self.conceal_mask_texture_dirty_rect = None;
        }
        if let Some(rect) = self.conceal_mask_texture_dirty_rect.take() {
            if self.conceal_mask_texture.is_some() {
                if let Some(ci) = self.conceal_mask_region_image(rect)
                    && let Some(tex) = self.conceal_mask_texture.as_mut()
                {
                    tex.set_partial([rect.x0, rect.y0], ci, egui::TextureOptions::NEAREST);
                    return;
                }
            }
            self.conceal_mask_texture = None;
            self.conceal_mask_texture_dirty_rect = None;
        }
        if self.conceal_mask_texture.is_some() {
            return;
        }
        self.conceal_mask_texture_dirty_rect = None;
        let Some(composite) = self.composite_conceal_mask() else {
            return;
        };
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
        let secondary_pressed = ctx.input(|i| i.pointer.secondary_pressed());
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

        // ⚠ 旧版は「Select 以外のツール開始時に選択を解除」していたが、これだと
        // `commit_conceal_shape` で自動選択した直後のフレームで選択が消されて
        // 「ハンドルが一瞬だけ出る」現象になっていた (Codex P1 / 実機 FB R3)。
        //
        // 新方針: 選択は ツール切替時 (`tool` enum 値が変わったとき) のみクリア。
        // ツールを切り替えずに描き続けている間は、直近 shape の選択を保持して
        // ハンドル操作 (= 太さ・サイズ・回転の微調整) を許可する。

        // ── 共通ハンドル処理 (ツール非依存): 直近 shape のハンドルが操作中なら
        //    そちらを優先処理して、新規 shape 作成側に流さない。
        //
        // - 既に conceal_drag があれば apply_drag で更新 → release で完了
        // - 新規 press のとき、選択中 shape のハンドル (= Body 以外) なら
        //   begin_drag を仕込んで return (= 通常ツール処理をスキップ)
        let polygon_in_progress =
            self.conceal_tool == ConcealTool::Polygon && !self.conceal_lasso_points.is_empty();
        if !polygon_in_progress
            && self.try_handle_active_drag_or_handle_hit(
                primary_pressed,
                primary_released,
                pointer_pos,
                full_rect,
                zoom_pan,
                modifiers,
            )
        {
            return;
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
            ConcealTool::Polygon => {
                self.handle_polygon_tool(
                    primary_pressed,
                    secondary_pressed,
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

    /// Select 以外のツールでも「直近の選択 shape のハンドル」を操作できるよう、
    /// ツール dispatch の前に走らせる共通処理。
    ///
    /// 戻り値 `true` のときは呼び出し側で `return` して通常ツール処理を skip する:
    /// - 既に `conceal_drag` が立っている (= 進行中のハンドル操作) → 更新して継続
    /// - 新規 primary_pressed で選択中 shape の **ハンドル (= Body 以外)** に
    ///   ヒットした → drag を仕込む
    ///
    /// 戻り値 `false` のときは通常ツール処理 (= 新規 shape 作成) に進む。
    /// 選択中 shape の **Body 上クリック** はここでは消費せず、通常ツール側で
    /// 新規 shape ドラッグ開始扱いになる (= 関係ない場所のクリックと同じ動作)。
    fn try_handle_active_drag_or_handle_hit(
        &mut self,
        primary_pressed: bool,
        primary_released: bool,
        pointer_pos: Option<egui::Pos2>,
        full_rect: egui::Rect,
        zoom_pan: Option<(f32, egui::Vec2)>,
        modifiers: egui::Modifiers,
    ) -> bool {
        // ① 進行中のドラッグがあれば最優先で処理
        if let Some(drag) = self.conceal_drag {
            // ポインタが取れないと apply_drag は走らせない (落としてしまうので
            // primary_released だけは検出して drag を片付ける)。
            let img_pos_opt = pointer_pos.and_then(|p| {
                self.conceal_image_layout(full_rect, zoom_pan)
                    .map(|(scale, img_rect)| {
                        (
                            (p.x - img_rect.min.x) / scale,
                            (p.y - img_rect.min.y) / scale,
                        )
                    })
            });
            if let Some(img_pos) = img_pos_opt {
                let new_shape = vector_edit::apply_drag(&drag, img_pos, &modifiers);
                let drag_idx = drag.idx();
                if drag_idx < self.conceal_shapes.len() {
                    self.conceal_shapes[drag_idx] = new_shape;
                }
                // R5: ドラッグ中もマスク overlay + 隠蔽テクスチャを毎フレ再生成。
                // mask_texture を落とすと次フレームの ensure_conceal_mask_texture で再生、
                // conceal_caches を落とすとプレビュー押下時の合成結果も追従する。
                self.conceal_mask_texture = None;
                if let Some(fs_idx) = self.fullscreen_idx {
                    self.clear_conceal_caches(fs_idx);
                }
            }
            if primary_released {
                self.conceal_drag = None;
                self.conceal_mask_texture = None;
                if let Some(fs_idx) = self.fullscreen_idx {
                    self.clear_conceal_caches(fs_idx);
                }
            }
            return true;
        }
        // ② 新規 primary_pressed で、選択中 shape の handle (Body 以外) なら drag 開始
        if !primary_pressed {
            return false;
        }
        let Some(sel) = self.conceal_selected_shape else {
            return false;
        };
        let Some(screen) = pointer_pos else {
            return false;
        };
        let Some((scale, img_rect)) = self.conceal_image_layout(full_rect, zoom_pan) else {
            return false;
        };
        let img_pos = (
            (screen.x - img_rect.min.x) / scale,
            (screen.y - img_rect.min.y) / scale,
        );
        let Some(shape) = self.conceal_shapes.get(sel).copied() else {
            return false;
        };
        let layout = vector_edit::compute_handle_layout(&shape, scale);
        let Some(target) = vector_edit::hit_test(&layout, img_pos, scale) else {
            return false;
        };
        // 選択中 shape の **Body** クリックは描画モード時のみ Pan ドラッグとして
        // 消費する。**消去モード (F)** では fallthrough して新規消去 shape の
        // 作成へ流す (Codex P2 R4 #2、消しゴム側と同じ条件)。
        //
        // ⚠ 非選択 shape の Body クリックはここに来ない (= layout は選択中 shape
        // 限定で計算しているため)。
        if matches!(target, vector_edit::HoverTarget::Body) && !self.conceal_paint_mode {
            return false;
        }
        self.push_conceal_undo();
        self.conceal_drag = Some(vector_edit::begin_drag(target, sel, shape, img_pos));
        self.conceal_mask_texture = None;
        true
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
                // R5: ドラッグ中もマスク overlay + 隠蔽合成キャッシュを毎フレ更新する
                // (実機 FB: 「ハンドルのドラッグでも楕円は再描画されません」)。
                // 旧版は 4K = 32MB のテクスチャアップロードを嫌って release 1 回更新に
                // していたが、ユーザー要望でリアルタイム化を優先する。
                self.conceal_mask_texture = None;
                if let Some(fs_idx) = self.fullscreen_idx {
                    self.clear_conceal_caches(fs_idx);
                }
            }
            if primary_released {
                self.conceal_drag = None;
                // ドラッグ完了 → 最終形状を反映するためにマスクテクスチャ再生成。
                self.conceal_mask_texture = None;
                // shape の geometry が変わったので合成 cache も失効させる
                // (= preview / 隠蔽合成時に最新形状で再計算される)。
                if let Some(fs_idx) = self.fullscreen_idx {
                    self.clear_conceal_caches(fs_idx);
                }
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
                if let Some(img_pos) = self.conceal_screen_to_image(pos, full_rect, zoom_pan, false)
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
                if let Some(img_pos) = self.conceal_screen_to_image(pos, full_rect, zoom_pan, false)
                {
                    // サンプリング間引き (~2px 離れたら追加)
                    crate::manual_mask_tools::push_freehand_point(
                        &mut self.conceal_lasso_points,
                        img_pos,
                    );
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

    fn handle_polygon_tool(
        &mut self,
        primary_pressed: bool,
        secondary_pressed: bool,
        pointer_pos: Option<egui::Pos2>,
        paint: bool,
        full_rect: egui::Rect,
        zoom_pan: Option<(f32, egui::Vec2)>,
    ) {
        if secondary_pressed {
            if let Some(pts) =
                crate::manual_mask_tools::take_completed_polygon(&mut self.conceal_lasso_points)
            {
                self.push_conceal_undo();
                self.paint_polygon_conceal(&pts, paint);
                self.show_feedback_toast("[多角形を確定]".to_string());
            }
            return;
        }
        if !primary_pressed {
            return;
        }
        let Some(screen) = pointer_pos else {
            return;
        };
        let Some((scale, img_rect)) = self.conceal_image_layout(full_rect, zoom_pan) else {
            return;
        };
        let img_pos = (
            (screen.x - img_rect.min.x) / scale,
            (screen.y - img_rect.min.y) / scale,
        );
        if crate::manual_mask_tools::should_close_polygon(
            &self.conceal_lasso_points,
            img_pos,
            scale,
        ) {
            if let Some(pts) =
                crate::manual_mask_tools::take_completed_polygon(&mut self.conceal_lasso_points)
            {
                self.push_conceal_undo();
                self.paint_polygon_conceal(&pts, paint);
                self.show_feedback_toast("[多角形を確定]".to_string());
            }
        } else {
            crate::manual_mask_tools::push_polygon_vertex(
                &mut self.conceal_lasso_points,
                img_pos,
                scale,
            );
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
                        op: ShapeOp::Add,
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
                            op: ShapeOp::Add,
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
                            op: ShapeOp::Add,
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
                            op: ShapeOp::Add,
                            center: (cx, cy),
                            half_w: hw,
                            half_h: hh,
                            rotation_rad: 0.0,
                        },
                        ConcealTool::Ellipse => Shape::Ellipse {
                            op: ShapeOp::Add,
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

    /// 描画モードなら Add Shape、消去モードなら Subtract Shape を追加する。
    ///
    /// ビットマップマスクを下地にし、その上に Shape を作成順で重ねる。
    /// 消去モードの矩形/楕円/線は既存 Shape を丸ごと削除せず、上から削る
    /// ベクターオブジェクトとして残る。
    fn commit_conceal_shape(&mut self, shape: Shape, paint: bool) {
        let op = if paint {
            ShapeOp::Add
        } else {
            ShapeOp::Subtract
        };
        self.conceal_shapes.push(shape.with_op(op));
        crate::logger::log(format!(
            "conceal: shape commit tool={:?} op={op:?} mask={}x{} shapes={}",
            self.conceal_tool,
            self.conceal_mask_size[0],
            self.conceal_mask_size[1],
            self.conceal_shapes.len(),
        ));
        // 新規 shape を自動選択 (実機 FB)。コミット直後にハンドルが描画される
        // ので、ユーザーは [S] で選択ツールへ切替→ハンドル操作で太さ/サイズを
        // 微調整できる (= 「線幅をパネルで設定 → 引いた線にハンドルで調整」
        // ワークフロー)。
        self.conceal_selected_shape = Some(self.conceal_shapes.len() - 1);
        self.conceal_mask_texture = None;
        // 新規 shape / shape 削除は conceal_cache の composite 内容を変えるので、
        // 現在編集中の idx 分だけ cache を破棄して次回 preview / 隠蔽合成時に
        // 再計算させる (実機 FB: 矩形/楕円描画後にプレビューしてもモザイクが
        // 反映されず、再エントリして初めて反映された問題への対応)。
        // `bump_conceal_generation` だと全 idx の cache を stale 化してしまい
        // 他ページの composite も無駄に再計算するので、idx-specific な
        // `clear_conceal_caches` を使う。
        if let Some(fs_idx) = self.fullscreen_idx {
            self.clear_conceal_caches(fs_idx);
        }
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
        // マスクオーバーレイ (紫半透明)。プレビュー押下中はマスクを隠して
        // 「合成後の結果」だけが見えるようにする (= ユーザー要望: プレビューでは
        // マスク表示オフ)。
        if !self.conceal_preview_active {
            self.ensure_conceal_mask_texture(ctx);
            if let Some(ref tex) = self.conceal_mask_texture {
                if let Some((_total_scale, img_rect)) =
                    self.conceal_image_layout(full_rect, zoom_pan)
                {
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
        }

        // ツールプレビュー (ドラッグ中)
        self.draw_conceal_tool_preview(ui, full_rect, zoom_pan);

        // 選択ツール中は、最終マスクでは透明になる削除オブジェクトも編集対象として
        // 見えるように全ベクタのアウトラインを表示する。
        self.draw_conceal_shape_outlines(ui, full_rect, zoom_pan);

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

        // Lasso / Polygon 多角形ライン
        if self.conceal_lasso_points.len() >= 2 {
            let pts: Vec<egui::Pos2> = self
                .conceal_lasso_points
                .iter()
                .map(|&p| self.conceal_image_to_screen(p, full_rect, zoom_pan))
                .collect();
            painter.add(egui::Shape::line(pts.clone(), stroke));
            if matches!(self.conceal_tool, ConcealTool::Lasso | ConcealTool::Polygon) {
                painter.line_segment(
                    [*pts.last().unwrap(), pts[0]],
                    egui::Stroke::new(
                        1.0,
                        egui::Color32::from_rgba_unmultiplied(255, 255, 255, 100),
                    ),
                );
            }
            if self.conceal_tool == ConcealTool::Polygon {
                for (idx, p) in pts.into_iter().enumerate() {
                    let fill = if idx == 0 {
                        egui::Color32::from_rgb(255, 245, 120)
                    } else {
                        preview_color
                    };
                    painter.circle_filled(p, 4.0, fill);
                    painter.circle_stroke(p, 4.0, egui::Stroke::new(1.0, egui::Color32::BLACK));
                }
            }
        } else if self.conceal_tool == ConcealTool::Polygon && self.conceal_lasso_points.len() == 1
        {
            let p = self.conceal_image_to_screen(self.conceal_lasso_points[0], full_rect, zoom_pan);
            painter.circle_filled(p, 4.0, egui::Color32::from_rgb(255, 245, 120));
            painter.circle_stroke(p, 4.0, egui::Stroke::new(1.0, egui::Color32::BLACK));
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

    /// 選択ツール中、全ベクタオブジェクトの存在を示す編集用アウトラインを描く。
    ///
    /// `Subtract` Shape は合成済みのマスク上では透明になるため、選択・編集のための
    /// 補助表示として add/subtract を色分けした枠を別レイヤーに描く。
    fn draw_conceal_shape_outlines(
        &self,
        ui: &mut egui::Ui,
        full_rect: egui::Rect,
        zoom_pan: Option<(f32, egui::Vec2)>,
    ) {
        if self.conceal_preview_active
            || self.conceal_tool != ConcealTool::Select
            || self.conceal_shapes.is_empty()
        {
            return;
        }
        let Some((scale, _img_rect)) = self.conceal_image_layout(full_rect, zoom_pan) else {
            return;
        };

        let painter = ui.painter().with_clip_rect(full_rect);
        let to_screen = |p: (f32, f32)| self.conceal_image_to_screen(p, full_rect, zoom_pan);
        for (idx, shape) in self.conceal_shapes.iter().enumerate() {
            if Some(idx) == self.conceal_selected_shape {
                continue;
            }
            let layout = vector_edit::compute_handle_layout(shape, scale);
            vector_edit::draw_shape_outline(&painter, &layout, shape.op(), &to_screen);
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
    pub(crate) fn conceal_panel_rect(&self, full_rect: egui::Rect) -> egui::Rect {
        let panel_pos = egui::pos2(
            full_rect.min.x + PANEL_MARGIN_X,
            full_rect.min.y + PANEL_MARGIN_Y,
        );
        // 可視パネルはウィンドウ下端近くまで伸びるため、入力抑制領域も同じ高さに
        // 揃える。固定見積もりにすると、パネル下半分のクリックがキャンバス操作へ
        // 抜けてしまう。
        let h = conceal_panel_outer_height(full_rect, panel_pos);
        egui::Rect::from_min_size(panel_pos, egui::vec2(PANEL_W, h))
    }

    /// 隠蔽加工モードの左パネル。Phase 2 では 8 ツール + 描画/消去 + サイズスライダを描く。
    pub(crate) fn draw_conceal_panel(&mut self, ctx: &egui::Context, full_rect: egui::Rect) {
        if !self.conceal_mode {
            return;
        }
        // 描画後に reset_conceal_mode を発火するフラグ (closure 内で直接 reset すると
        // モード state が描画中に変わって widget の借用問題が起こりうる)。
        let mut should_close_after_draw = false;
        let mut switch_to: Option<usize> = None;
        let panel_pos = egui::pos2(
            full_rect.min.x + PANEL_MARGIN_X,
            full_rect.min.y + PANEL_MARGIN_Y,
        );

        // クリック吸収用に generous な sink rect を Frame::popup より先に登録する。
        // egui の hit test ルール「同じ rect の Response は後勝ち」のもと:
        // - widget の click は widget 側 (= 後登録) で勝ち取る
        // - widget 外 (パネル内の隙間 + sink rect が visible Frame より少しはみ出す
        //   部分) は sink が吸収
        //
        // 旧実装の固定 1000px sink は 1440p/4K/縦長環境でパネル下端まで届かない
        // ことがあるため、可視パネルと同じ高さから動的に作る。
        let panel_h = conceal_panel_outer_height(full_rect, panel_pos);
        let sink_rect =
            egui::Rect::from_min_size(panel_pos, egui::vec2(PANEL_W + 4.0, panel_h + 8.0));

        egui::Area::new(egui::Id::new("conceal_panel"))
            .fixed_pos(panel_pos)
            .order(egui::Order::Foreground)
            .interactable(true)
            .show(ctx, |ui| {
                // 1. sink を widget より前に登録 (egui の hit test 後勝ちルール対策)
                ui.interact(
                    sink_rect,
                    egui::Id::new("conceal_panel_click_sink"),
                    egui::Sense::click_and_drag(),
                );
                // 2. Frame::popup で背景 + 内容を描く (auto-size、clipping なし)
                egui::Frame::popup(ui.style())
                    .fill(egui::Color32::from_rgba_unmultiplied(20, 20, 20, 230))
                    .stroke(egui::Stroke::new(
                        1.0,
                        egui::Color32::from_rgba_unmultiplied(255, 255, 255, 40),
                    ))
                    .corner_radius(6.0)
                    .show(ui, |ui| {
                        // 幅キャップ (= 内容が広い widget で auto-size 拡大しないように)。
                        ui.set_min_width(PANEL_W);
                        ui.set_max_width(PANEL_W);
                        // ⚠ 重要: テーマに依存せず常に DARK visuals を使う (R3 FB)。
                        *ui.visuals_mut() = egui::Visuals::dark();
                        ui.visuals_mut().override_text_color = Some(egui::Color32::WHITE);

                        // ── ヘッダ (タイトル + プレビュー + 閉じる × ボタン) ──
                        // R5: ヘッダは **ScrollArea の外** に出す。旧版はスクロールバー
                        // が × ボタンに重なる + × が縦スクロールに巻き込まれる現象が
                        // あったため (実機 FB R5、消しゴム側と同じ対応)。
                        let mut preview_pressed = false;
                        let mut close_clicked = false;
                        ui.horizontal(|ui| {
                            ui.label(
                                egui::RichText::new("隠蔽加工")
                                    .size(15.0)
                                    .strong()
                                    .color(egui::Color32::WHITE),
                            );
                            ui.with_layout(
                                egui::Layout::right_to_left(egui::Align::Center),
                                |ui| {
                                    // 閉じる × ボタン
                                    let (close_rect, close_resp) = ui.allocate_exact_size(
                                        egui::vec2(26.0, 22.0),
                                        egui::Sense::click(),
                                    );
                                    let close_bg = if close_resp.hovered() {
                                        egui::Color32::from_rgba_unmultiplied(220, 80, 80, 200)
                                    } else {
                                        egui::Color32::from_rgba_unmultiplied(80, 80, 80, 120)
                                    };
                                    ui.painter().rect_filled(close_rect, 4.0, close_bg);
                                    crate::ui_fullscreen::draw_icons::draw_close_icon(
                                        ui.painter(),
                                        close_rect.center(),
                                        8.0,
                                    );
                                    if close_resp.clicked() {
                                        close_clicked = true;
                                    }
                                    close_resp.on_hover_text("閉じる (Esc / Ctrl+M)");
                                    ui.add_space(2.0);
                                    // プレビュー 目アイコン (= while held)
                                    let (eye_rect, eye_resp) = ui.allocate_exact_size(
                                        egui::vec2(26.0, 22.0),
                                        egui::Sense::click_and_drag(),
                                    );
                                    let eye_bg = if eye_resp.is_pointer_button_down_on() {
                                        // 押下中 = アクセント青
                                        egui::Color32::from_rgb(60, 120, 200)
                                    } else if eye_resp.hovered() {
                                        egui::Color32::from_rgba_unmultiplied(100, 100, 100, 220)
                                    } else {
                                        egui::Color32::from_rgba_unmultiplied(80, 80, 80, 120)
                                    };
                                    ui.painter().rect_filled(eye_rect, 4.0, eye_bg);
                                    crate::ui_fullscreen::draw_icons::draw_eye_icon(
                                        ui.painter(),
                                        eye_rect.center(),
                                        8.0,
                                    );
                                    if eye_resp.is_pointer_button_down_on() {
                                        preview_pressed = true;
                                    }
                                    eye_resp.on_hover_text(
                                        "押している間: モザイク反映後の最終結果プレビュー",
                                    );
                                },
                            );
                        });
                        self.conceal_preview_active = preview_pressed;
                        if close_clicked {
                            should_close_after_draw = true;
                        }
                        ui.label(
                            egui::RichText::new(format!(
                                "処理: {}",
                                self.settings.conceal_type.label()
                            ))
                            .color(egui::Color32::from_gray(200)),
                        );
                        ui.separator();

                        // ── 残りを ScrollArea で囲む ──
                        // R5+: ScrollArea は親 UI の available_rect を上限にするため、
                        // `max_height` だけでは Area + Frame::popup 内で content 高に
                        // 縮むことがある。先に親領域を明示確保してから、その中で
                        // ScrollArea を下端近くまで伸ばす。
                        let body_height =
                            (full_rect.max.y - ui.cursor().top() - PANEL_BOTTOM_MARGIN)
                                .max(PANEL_MIN_BODY_H);
                        ui.allocate_ui_with_layout(
                            egui::vec2(PANEL_W, body_height),
                            egui::Layout::top_down(egui::Align::LEFT),
                            |ui| {
                                ui.set_min_width(PANEL_W);
                                ui.set_max_width(PANEL_W);
                                ui.set_min_height(body_height);
                                egui::ScrollArea::vertical()
                                    .max_height(body_height)
                                    .auto_shrink([false, false])
                                    .show(ui, |ui| {
                                        let btn_w = ((PANEL_W - 16.0 - 4.0) / 2.0).max(60.0);
                                        let btn_size = egui::vec2(btn_w, 24.0);

                                        // 見開きペアの左/右切替 (見開きから入った場合のみ)。
                                        if let Some((left_idx, right_idx)) =
                                            self.conceal_spread_ctx.map(|c| c.pair)
                                        {
                                            let pages =
                                                [("左ページ", left_idx), ("右ページ", right_idx)];
                                            ui.horizontal(|ui| {
                                                ui.spacing_mut().item_spacing.x = 4.0;
                                                for &(label, target_idx) in pages.iter() {
                                                    let is_active =
                                                        self.fullscreen_idx == Some(target_idx);
                                                    if panel_toggle_button(
                                                        ui,
                                                        label,
                                                        is_active,
                                                        Some(btn_size),
                                                        None,
                                                    )
                                                    .clicked()
                                                        && !is_active
                                                    {
                                                        switch_to = Some(target_idx);
                                                    }
                                                }
                                            });
                                            ui.separator();
                                        }

                                        // 描画 / 消去 (active=赤/青、inactive=暗灰、hover=やや明灰)
                                        ui.horizontal(|ui| {
                                            ui.spacing_mut().item_spacing.x = 4.0;
                                            if panel_toggle_button(
                                                ui,
                                                "描画 [D]",
                                                self.conceal_paint_mode,
                                                Some(btn_size),
                                                Some(PanelToggleColors::paint_red()),
                                            )
                                            .clicked()
                                            {
                                                self.conceal_paint_mode = true;
                                            }
                                            if panel_toggle_button(
                                                ui,
                                                "消去 [F]",
                                                !self.conceal_paint_mode,
                                                Some(btn_size),
                                                Some(PanelToggleColors::erase_blue()),
                                            )
                                            .clicked()
                                            {
                                                self.conceal_paint_mode = false;
                                            }
                                        });
                                        ui.separator();

                                        // ツールパレット。筆/囲みはビットマップ下地、線/矩形/楕円は
                                        // その上に作成順で重なるオブジェクト。消去モードの
                                        // オブジェクトは Subtract Shape として残る。
                                        ui.label(
                                            egui::RichText::new("ビットマップ:")
                                                .color(egui::Color32::from_gray(200)),
                                        );
                                        let mut tool = self.conceal_tool;
                                        let bitmap_rows: &[&[(ConcealTool, &str)]] = &[
                                            &[
                                                (ConcealTool::Brush, "筆 [B]"),
                                                (ConcealTool::Lasso, "囲み [L]"),
                                            ],
                                            &[(ConcealTool::Polygon, "多角形 [P]")],
                                        ];
                                        for row in bitmap_rows {
                                            ui.horizontal(|ui| {
                                                ui.spacing_mut().item_spacing.x = 4.0;
                                                for &(kind, label) in *row {
                                                    if panel_toggle_button(
                                                        ui,
                                                        label,
                                                        tool == kind,
                                                        Some(btn_size),
                                                        None,
                                                    )
                                                    .clicked()
                                                    {
                                                        tool = kind;
                                                    }
                                                }
                                            });
                                        }
                                        ui.label(
                                            egui::RichText::new("オブジェクト:")
                                                .color(egui::Color32::from_gray(200)),
                                        );
                                        let object_rows: [[(ConcealTool, &str); 2]; 3] = [
                                            [
                                                (ConcealTool::Select, "選択 [S]"),
                                                (ConcealTool::Line, "直線 [I]"),
                                            ],
                                            [
                                                (ConcealTool::VertLine, "縦線 [V]"),
                                                (ConcealTool::HorizLine, "横線 [H]"),
                                            ],
                                            [
                                                (ConcealTool::Rect, "矩形 [R]"),
                                                (ConcealTool::Ellipse, "楕円 [O]"),
                                            ],
                                        ];
                                        for row in object_rows.iter() {
                                            ui.horizontal(|ui| {
                                                ui.spacing_mut().item_spacing.x = 4.0;
                                                for &(kind, label) in row.iter() {
                                                    if panel_toggle_button(
                                                        ui,
                                                        label,
                                                        tool == kind,
                                                        Some(btn_size),
                                                        None,
                                                    )
                                                    .clicked()
                                                    {
                                                        tool = kind;
                                                    }
                                                }
                                            });
                                        }
                                        ui.add(
                                            egui::Label::new(
                                                egui::RichText::new(
                                                    "オブジェクトは下地の上に作成順で反映",
                                                )
                                                .size(10.0)
                                                .color(egui::Color32::from_gray(150)),
                                            )
                                            .wrap(),
                                        );
                                        if tool != self.conceal_tool {
                                            let entering_select = tool == ConcealTool::Select;
                                            self.conceal_tool = tool;
                                            self.conceal_drag = None;
                                            self.conceal_lasso_points.clear();
                                            self.conceal_line_start = None;
                                            self.conceal_line_end = None;
                                            self.conceal_shape_drag_start = None;
                                            self.conceal_shape_drag_end = None;
                                            // ツール切替時は選択もクリア (Codex P1 対応)。
                                            // Select 入場時は auto-select 維持
                                            // (code-review CONFIRMED)。
                                            if !entering_select {
                                                self.conceal_selected_shape = None;
                                            }
                                            self.conceal_mask_texture = None;
                                        }

                                        // サイズスライダ (Brush は半径、Line 系は太さ)
                                        let [w, h] = self.conceal_mask_size;
                                        let long_edge = w.max(h).max(1) as f32;
                                        match self.conceal_tool {
                                            ConcealTool::Brush => {
                                                ui.separator();
                                                let mut r = self.settings.conceal_brush_radius;
                                                ui.add(
                                                    egui::Slider::new(
                                                        &mut r,
                                                        1.0..=long_edge / 5.0,
                                                    )
                                                    .text("ブラシ半径"),
                                                );
                                                self.settings.conceal_brush_radius = r;
                                            }
                                            ConcealTool::Line
                                            | ConcealTool::VertLine
                                            | ConcealTool::HorizLine => {
                                                ui.separator();
                                                let mut t = self.settings.conceal_line_width;
                                                ui.add(
                                                    egui::Slider::new(
                                                        &mut t,
                                                        1.0..=long_edge / 5.0,
                                                    )
                                                    .text("線幅"),
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
                                        let type_rows: [[crate::conceal::ConcealType; 2]; 2] = [
                                            [
                                                crate::conceal::ConcealType::Mosaic,
                                                crate::conceal::ConcealType::WhiteFill,
                                            ],
                                            [
                                                crate::conceal::ConcealType::BlackFill,
                                                crate::conceal::ConcealType::Blur,
                                            ],
                                        ];
                                        for row in type_rows.iter() {
                                            ui.horizontal(|ui| {
                                                ui.spacing_mut().item_spacing.x = 4.0;
                                                for &t in row.iter() {
                                                    if panel_toggle_button(
                                                        ui,
                                                        t.label(),
                                                        new_type == t,
                                                        Some(btn_size),
                                                        None,
                                                    )
                                                    .clicked()
                                                    {
                                                        new_type = t;
                                                    }
                                                }
                                            });
                                        }
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
                                            let mut tile_mode =
                                                self.settings.conceal_mosaic_tile_mode;
                                            let mut tile_changed = false;
                                            let is_ratio = matches!(
                                                tile_mode,
                                                crate::conceal::TileSizeMode::LongEdgeRatio(_)
                                            );
                                            ui.horizontal(|ui| {
                                                ui.spacing_mut().item_spacing.x = 4.0;
                                                if panel_toggle_button(
                                                    ui,
                                                    "長辺比率",
                                                    is_ratio,
                                                    Some(btn_size),
                                                    None,
                                                )
                                                .clicked()
                                                    && !is_ratio
                                                {
                                                    tile_mode =
                                                        crate::conceal::TileSizeMode::LongEdgeRatio(
                                                            1.0,
                                                        );
                                                    tile_changed = true;
                                                }
                                                if panel_toggle_button(
                                                    ui,
                                                    "固定 px",
                                                    !is_ratio,
                                                    Some(btn_size),
                                                    None,
                                                )
                                                .clicked()
                                                    && is_ratio
                                                {
                                                    tile_mode =
                                                        crate::conceal::TileSizeMode::FixedPx(16);
                                                    tile_changed = true;
                                                }
                                            });
                                            match tile_mode {
                                                crate::conceal::TileSizeMode::LongEdgeRatio(
                                                    mut m,
                                                ) => {
                                                    let prev = m;
                                                    ui.add(
                                                        egui::Slider::new(
                                                            &mut m,
                                                            crate::conceal::TILE_RATIO_MIN
                                                                ..=crate::conceal::TILE_RATIO_MAX,
                                                        )
                                                        .step_by(
                                                            crate::conceal::TILE_RATIO_STEP as f64,
                                                        )
                                                        .text("倍率"),
                                                    );
                                                    if (m - prev).abs() > 1e-6 {
                                                        tile_mode =
                                                    crate::conceal::TileSizeMode::LongEdgeRatio(m);
                                                        tile_changed = true;
                                                    }
                                                    let long_edge_u = w.max(h) as u32;
                                                    let tile_px = crate::conceal::compute_tile_size(
                                                        long_edge_u,
                                                        tile_mode,
                                                    );
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
                                                        tile_mode =
                                                            crate::conceal::TileSizeMode::FixedPx(
                                                                px,
                                                            );
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
                                                let label =
                                                    egui::RichText::new(b.process_description())
                                                        .size(11.0);
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
                                        } else if matches!(
                                            self.settings.conceal_type,
                                            crate::conceal::ConcealType::WhiteFill
                                                | crate::conceal::ConcealType::BlackFill
                                        ) {
                                            // ── WhiteFill / BlackFill パラメータ (Phase 3b) ──
                                            ui.separator();
                                            ui.label(
                                                egui::RichText::new("不透明度:")
                                                    .color(egui::Color32::from_gray(200)),
                                            );
                                            let mut opacity =
                                                self.settings.conceal_fill_opacity_percent;
                                            let prev_opacity = opacity;
                                            ui.add(
                                                egui::Slider::new(&mut opacity, 1..=100)
                                                    .text("%")
                                                    .step_by(1.0),
                                            );
                                            let mut fill_changed = false;
                                            if opacity != prev_opacity {
                                                self.settings.conceal_fill_opacity_percent =
                                                    opacity;
                                                fill_changed = true;
                                            }
                                            ui.label(
                                                egui::RichText::new("境界処理:")
                                                    .color(egui::Color32::from_gray(200)),
                                            );
                                            let mut edge = self.settings.conceal_fill_edge;
                                            for e in [
                                                crate::conceal::FillEdge::Sharp,
                                                crate::conceal::FillEdge::Feathered,
                                            ] {
                                                let label =
                                                    egui::RichText::new(e.process_description())
                                                        .size(11.0);
                                                if ui.radio(edge == e, label).clicked() && edge != e
                                                {
                                                    edge = e;
                                                    self.settings.conceal_fill_edge = e;
                                                    fill_changed = true;
                                                }
                                            }
                                            let _ = edge;
                                            if fill_changed || type_changed {
                                                self.bump_conceal_generation();
                                            }
                                        } else if matches!(
                                            self.settings.conceal_type,
                                            crate::conceal::ConcealType::Blur
                                        ) {
                                            // ── Blur パラメータ (Phase 3c) ──────────────────
                                            ui.separator();
                                            ui.label(
                                                egui::RichText::new("ぼかし半径:")
                                                    .color(egui::Color32::from_gray(200)),
                                            );
                                            let mut blur_radius =
                                                self.settings.conceal_blur_radius_px;
                                            let prev = blur_radius;
                                            ui.add(
                                                egui::Slider::new(&mut blur_radius, 5.0..=100.0)
                                                    .text("px")
                                                    .step_by(1.0),
                                            );
                                            let mut blur_changed = false;
                                            if (blur_radius - prev).abs() > 0.01 {
                                                self.settings.conceal_blur_radius_px = blur_radius;
                                                blur_changed = true;
                                            }
                                            ui.label(
                                                egui::RichText::new("ぼかしモード:")
                                                    .color(egui::Color32::from_gray(200)),
                                            );
                                            let mut bmode = self.settings.conceal_blur_mode;
                                            for m in [
                                                crate::conceal::BlurMode::AsMask,
                                                crate::conceal::BlurMode::ExtendByRadius,
                                                crate::conceal::BlurMode::InsideOnly,
                                            ] {
                                                let label =
                                                    egui::RichText::new(m.process_description())
                                                        .size(11.0);
                                                if ui.radio(bmode == m, label).clicked()
                                                    && bmode != m
                                                {
                                                    bmode = m;
                                                    self.settings.conceal_blur_mode = m;
                                                    blur_changed = true;
                                                }
                                            }
                                            let _ = bmode;
                                            let mut feather = self.settings.conceal_blur_feather;
                                            let prev_feather = feather;
                                            ui.checkbox(&mut feather, "境界フェードを掛ける");
                                            if feather != prev_feather {
                                                self.settings.conceal_blur_feather = feather;
                                                blur_changed = true;
                                            }
                                            if blur_changed || type_changed {
                                                self.bump_conceal_generation();
                                            }
                                        }

                                        // ── プリセット 4 スロット (= 保存/読込ボタン、消しゴムマスクスロットと同じ見た目) ──
                                        //
                                        // 旧 UI は「1: (未保存) 1」+ 小さな保存ボタン で構成していたが、
                                        // 名称を編集する機能が無いため「1: ...」のような番号付きラベルは
                                        // ノイズになっていた (実機 FB)。マスクスロットと同じ
                                        // 「保存N / 読込N」グリッドに統一する。
                                        ui.separator();
                                        ui.label(
                                            egui::RichText::new("プリセット (1-4 で適用):")
                                                .color(egui::Color32::from_gray(200)),
                                        );
                                        // 4 列 × 2 行: 上段 [保存 1..4]、下段 [読込 1..4]
                                        let preset_btn_w =
                                            ((PANEL_W - 16.0 - 12.0) / 4.0).max(40.0);
                                        let preset_btn_size = egui::vec2(preset_btn_w, 22.0);
                                        for (row, action_label) in
                                            ["保存", "読込"].iter().enumerate()
                                        {
                                            ui.horizontal(|ui| {
                                                ui.spacing_mut().item_spacing.x = 4.0;
                                                for slot in 0..4usize {
                                                    let label =
                                                        format!("{}{}", action_label, slot + 1);
                                                    let has = self.settings.conceal_presets[slot]
                                                        .is_some();
                                                    // 読込は has==false (= 未保存) のとき押せないようにする
                                                    let enabled = if row == 0 { true } else { has };
                                                    let resp = ui
                                                        .add_enabled_ui(enabled, |ui| {
                                                            panel_toggle_button(
                                                                ui,
                                                                label,
                                                                false,
                                                                Some(preset_btn_size),
                                                                None,
                                                            )
                                                        })
                                                        .inner;
                                                    if resp.clicked() {
                                                        if row == 0 {
                                                            self.save_conceal_preset_to_slot(slot);
                                                        } else {
                                                            self.apply_conceal_preset(slot);
                                                        }
                                                    }
                                                }
                                            });
                                        }

                                        // ── マスクスロット (= 消しゴムと同じ「保存N / 読込N」2x2 grid) ──
                                        ui.separator();
                                        ui.label(
                                            egui::RichText::new("マスクスロット:")
                                                .color(egui::Color32::from_gray(200)),
                                        );
                                        let mask_btn_w = ((PANEL_W - 16.0 - 4.0) / 2.0).max(60.0);
                                        let mask_btn_size = egui::vec2(mask_btn_w, 22.0);
                                        for (row, action_label) in
                                            ["保存", "読込"].iter().enumerate()
                                        {
                                            ui.horizontal(|ui| {
                                                ui.spacing_mut().item_spacing.x = 4.0;
                                                for slot in 1..=2usize {
                                                    let label = format!("{}{}", action_label, slot);
                                                    if panel_toggle_button(
                                                        ui,
                                                        label,
                                                        false,
                                                        Some(mask_btn_size),
                                                        None,
                                                    )
                                                    .clicked()
                                                    {
                                                        if row == 0 {
                                                            self.save_conceal_mask_to_slot(slot);
                                                        } else {
                                                            self.load_conceal_mask_from_slot(slot);
                                                        }
                                                    }
                                                }
                                            });
                                        }
                                        ui.add(
                                            egui::Label::new(
                                                egui::RichText::new(
                                                    "フルスクリーン中 F9/F10 で隠蔽保存 1/2 を即適用",
                                                )
                                                .size(10.0)
                                                .color(egui::Color32::from_gray(150)),
                                            )
                                            .wrap(),
                                        );

                                        // ── マスク全削除 ───────────────────────────────────
                                        // 消しゴムパネル側と同じ幅 (= PANEL_W - 20) で揃える (実機 FB R4)。
                                        ui.separator();
                                        if ui
                                            .add(
                                                egui::Button::new(
                                                    egui::RichText::new("マスク全削除")
                                                        .color(egui::Color32::WHITE),
                                                )
                                                .fill(egui::Color32::from_rgb(120, 50, 50))
                                                .min_size(egui::vec2(PANEL_W - 20.0, 22.0)),
                                            )
                                            .clicked()
                                        {
                                            self.delete_all_conceal_mask();
                                        }

                                        ui.separator();
                                        let help = "Space+ドラッグ:一時パン\n\
                                            Ctrl+ホイール:ズーム\n\
                                            矢印:選択/全体を移動 (Ctrl:10倍)\n\
                                            Shift+ハンドル:拘束/等比/15°snap\n\
                                            Alt+ハンドル:中心固定\n\
                                            T:タイプ  G:グリッド  1-4:プリセット\n\
                                            多角形:始点クリック/右クリック/Enterで確定 Escで取消\n\
                                            Ctrl+Z:戻す  Delete:選択削除\n\
                                            Esc:解除/終了  Ctrl+M:終了\n\
                                            終了時はDB保存";
                                        ui.add(
                                            egui::Label::new(
                                                egui::RichText::new(help)
                                                    .size(10.5)
                                                    .color(egui::Color32::from_gray(190)),
                                            )
                                            .wrap(),
                                        );
                                    }); // ScrollArea::show
                            },
                        ); // allocate_ui_with_layout
                    }); // Frame::popup .show
                // クリック吸収 sink は Area::show の冒頭で登録済み (= widget より前)。
                // 後追いで sink を足すと egui の hit test ルール (= 同じ rect の Response
                // は後勝ち) で widget が click を受け取れなくなる。
            });
        if let Some(target) = switch_to {
            self.switch_conceal_target_in_spread(target);
        }
        if should_close_after_draw {
            self.reset_conceal_mode();
        }
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
