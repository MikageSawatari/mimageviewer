//! 隠蔽加工 (Conceal) モード: フルスクリーン画像の任意領域に
//! モザイク / 白塗り / 黒塗り / ぼかし を適用する。
//!
//! 詳細仕様: [docs/conceal-feature-plan.md](../../docs/conceal-feature-plan.md)
//!
//! # Phase 進捗
//!
//! このファイルは **Phase 1: モード骨組み**として導入される。実装範囲:
//!
//! - [`App::enter_conceal_mode`] / [`App::reset_conceal_mode`]
//! - `Ctrl+M` ホットキー処理 ([`ui_fullscreen.rs`] 側で呼ぶ)
//! - 空の左パネル ([`App::draw_conceal_panel`])
//!
//! 実際のツール処理 (8 種パレット、ハンドル編集、合成、Undo、保存) は Phase 2 以降で
//! このファイルに追加する。当面は最小機能だけ動かして「モードが入退場できる」
//! 「パネルが出る」までを確認する位置付け。

use eframe::egui;
use std::sync::Arc;

use crate::app::App;
use crate::fs_animation::FsCacheEntry;

/// ツールパネルの幅 (消しゴムと同じ値で揃える)。
const PANEL_W: f32 = 240.0;
/// ツールパネルの左上マージン。
const PANEL_MARGIN_X: f32 = 16.0;
const PANEL_MARGIN_Y: f32 = 60.0;

impl App {
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
        // (Phase 3 で `conceal_cache > adjustment_cache > ai_upscale_cache > fs_cache`
        //  の表示パイプラインを構築するときに同じ優先順を踏襲する。)
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
            self.conceal_spread_ctx = Some(crate::app::EraseSpreadCtx {
                saved_mode: self.spread_mode,
                pair,
            });
            // Single 表示にして fs_idx を左ページに固定
            self.spread_mode = crate::settings::SpreadMode::Single;
            self.fullscreen_idx = Some(target_idx);
            self.fs_zoom = 1.0;
            self.fs_pan = egui::Vec2::ZERO;
        }
        let fs_idx = target_idx;
        let [w, h] = pixels.size;

        self.conceal_mode = true;
        // メタ Undo (rating / tag / 補正) は Conceal モード中は別文脈なのでクリア。
        // 消しゴムと同じ境界扱い (Conceal 中の Ctrl+Z は `conceal_undo_stack` 専属)。
        self.clear_meta_undo();
        // 補正バイパス (post-filter 等): 当面は消しゴムと同じ扱いで bypass。
        // ドット表示などが混ざると精密な境界操作が難しくなる。
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
            "conceal: enter mode, image={w}x{h}, shapes={}, type={:?}",
            self.conceal_shapes.len(),
            self.settings.conceal_type
        ));
    }

    /// 隠蔽加工モードを終了する (Esc / Ctrl+M 再押下)。
    ///
    /// Phase 1 では DB への保存は実装しない (Phase 4 で永続化フローを完成させる)。
    /// 当面は in-memory のマスクを破棄するだけ。次に [`enter_conceal_mode`] したときに
    /// 再度 `conceal_db::get_full` から hydrate される (= DB に書いていなければ空マスク)。
    pub(crate) fn reset_conceal_mode(&mut self) {
        let restore_idx = self.fullscreen_idx;
        let was_conceal_mode = self.conceal_mode;
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

    // ── パネル描画 (Phase 1: スタブ) ────────────────────────────────

    /// 隠蔽加工モードの左パネル。Phase 1 では「モードに入った」ことが
    /// 視認できる最小 UI のみ。ツールパレット / プリセット / マスクスロットは
    /// Phase 2 以降で本ファイルに追加。
    pub(crate) fn draw_conceal_panel(&mut self, ctx: &egui::Context, full_rect: egui::Rect) {
        if !self.conceal_mode {
            return;
        }

        let panel_pos = egui::pos2(
            full_rect.min.x + PANEL_MARGIN_X,
            full_rect.min.y + PANEL_MARGIN_Y,
        );
        let panel_rect = egui::Rect::from_min_size(panel_pos, egui::vec2(PANEL_W, 180.0));

        egui::Area::new(egui::Id::new("conceal_panel"))
            .fixed_pos(panel_pos)
            .order(egui::Order::Foreground)
            .show(ctx, |ui| {
                egui::Frame::popup(ui.style())
                    .fill(egui::Color32::from_black_alpha(220))
                    .show(ui, |ui| {
                        ui.set_min_width(PANEL_W);
                        ui.heading("隠蔽加工");
                        ui.separator();
                        ui.label(format!(
                            "処理タイプ: {}",
                            self.settings.conceal_type.label()
                        ));
                        ui.label("(Phase 1 stub: ツール UI は Phase 2 で追加)");
                        ui.separator();
                        ui.label("Ctrl+M または Esc で終了");
                    });
            });

        // パネル領域はクリックスルーしない (将来ツール操作を載せるため)。
        // Phase 1 ではパネル外クリックを画像領域に通すだけ。
        let _ = panel_rect;
    }
}
