//! 消しゴム (Erase) モード: フルスクリーン画像の任意領域をマスクし、
//! MI-GAN で補完 (inpaint) する。
//!
//! ツール (Phase 2b で 8 種に拡張、隠蔽加工と統一): 選択 (Select) / 筆 (Brush) /
//! 囲み (Lasso) / 直線 (Line) / 縦線 / 横線 / 矩形 (Rect) / 楕円 (Ellipse)
//! モード: 描画 / 消去 の切り替え
//! マスクは SQLite (mask_db) に永続化される。
//!
//! # Phase 2b 移行ノート
//!
//! - Select ツールのハンドル操作は [`crate::vector_edit`] 経由に統一
//!   (旧 Line の Ctrl+ドラッグ複合操作 = 垂直回転・水平太さ変更 は廃止、専用ハンドル方式に置換)
//! - ベクタオブジェクトは `Vec<LineObject>` → `Vec<Shape>` に変更
//!   (旧 mask_db データは `shapes_from_json` で `Shape::Line` として自動変換)
//! - 新ツール: Rect / Ellipse (Phase 0b、`Shape::Rect` / `Shape::Ellipse` を作成)

use eframe::egui;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;

use crate::app::{App, EraseSnapshot, EraseTool, ShiftDragState};
use crate::fs_animation::FsCacheEntry;
use crate::mask_db::{LineKind, Shape, ShapeOp};
use crate::ui_fullscreen::FsKeyAction;
use crate::ui_fullscreen::draw_icons::{PanelToggleColors, panel_toggle_button};
use crate::vector_edit;

/// 消しゴム MI-GAN ジョブの用途。
///
/// preview と commit は同じ idx でも独立に走れる必要がある。旧実装は idx だけを
/// key にしていたため、preview 押下が commit 中ジョブを cancel できてしまった。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum EraseInpaintKind {
    Preview,
    Commit,
}

/// `App.erase_inpaint_pending` の key。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) struct EraseInpaintPendingKey {
    pub(crate) idx: usize,
    pub(crate) kind: EraseInpaintKind,
}

/// `apply_inpaint_only` の戻り値。
///
/// 旧版は bool で「何かしら処理した = true」を返していたが、入力ピクセル取得不能 /
/// サイズ不一致でも true が返るため caller が `[補完中…]` toast を出してしまい、
/// 実際は commit 未投入の状態 (= ensure_erase_result_texture が次フレ拾うまで何も
/// 動かない) を「処理中」と誤表示していた (Phase 1-5 code-review CONFIRMED)。
pub(crate) enum ApplyInpaintOutcome {
    /// マスクが空で何もしなかった。toast 不要。
    NoMask,
    /// マスクは保存したが、サイズ不一致等で commit を即時投入できなかった。
    /// `ensure_erase_result_texture` が次フレで拾う。
    Deferred,
    /// MI-GAN ジョブを実際にキューへ投入した。
    Launched,
}

/// 進行中の MI-GAN inpaint 推論。`App.erase_inpaint_pending` で保持され、
/// 推論完了 (もしくは新規投入で前ジョブをキャンセル) するまで生存する。
/// 推論本体は worker thread で走り、結果は `rx` 経由で UI スレッドへ届ける。
pub(crate) struct EraseInpaintPending {
    /// 結果を反映する fs_cache のキー (= フルスクリーン idx)。
    pub idx: usize,
    /// 投入時の `App.items_generation`。`poll` 時に世代が進んでいれば結果は捨てる
    /// (フォルダ移動 / 検索結果差し替えで idx の指す item が変わるため)。
    pub items_generation: u64,
    /// 投入時の `page_path_key(idx)`。世代が同じでも path が変わっていれば捨てる
    /// (sort 変更等の rare ケース対策)。
    pub path_key: Option<String>,
    /// worker からの結果受信。`Ok(image)` で完了、`Err(_)` でキャンセル/失敗。
    pub rx: mpsc::Receiver<egui::ColorImage>,
    /// 投入時にセット、worker は毎タイル前に load してキャンセル監視。
    /// 新規ジョブ投入で前ジョブを `store(true)` にして worker に終了を促す。
    pub cancel: Arc<AtomicBool>,
    /// 投入時刻 (ログ用)。
    pub started_at: std::time::Instant,
    /// 投入時の表示入力世代。commit 結果の `EraseResultKey` に使う。
    pub input_generation: u64,
    /// 投入時の消しゴムマスク世代。commit 結果の `EraseResultKey` に使う。
    pub mask_generation: u64,
    /// 呼び出し経路識別子 (ログ用)。
    pub log_prefix: &'static str,
    /// プレビュー専用ジョブか (= preview ボタン押下による起動)。
    ///
    /// `true` のとき完了時は **fs_cache を触らず**、
    /// `erase_preview_cache` だけを更新する (Codex P1 R4 #1)。
    /// プレビューは「現在のマスクで MI-GAN を試した一時結果」なので、ESC で
    /// 抜けた / マスクを変更した / マスクを全削除した時に preview_cache を
    /// 捨てれば残らない設計にする。`false` のときは fs_cache へ書き戻さず、
    /// `erase_result_cache` に commit 結果を格納する。
    pub is_preview: bool,
}

// (Phase 2b で削除: ENDPOINT_HIT_RADIUS / ROTATE_DEG_PER_PIXEL は vector_edit::
// HANDLE_HIT_RADIUS_PX とハンドル方式の回転に置換)

/// 矢印キー 1 回あたりの移動量 (ピクセル)。
const NUDGE_PIXELS: f32 = 1.0;
/// Ctrl+矢印の移動量 (ピクセル)。
const NUDGE_PIXELS_FAST: f32 = 10.0;
/// `[` / `]` キー 1 回あたりの回転量 (度)。
const ROTATE_DEG_STEP: f32 = 0.1;
/// Ctrl+`[` / `]` の回転量 (度)。
const ROTATE_DEG_STEP_FAST: f32 = 1.0;

/// MI-GAN の固定入力サイズ。
const MIGAN_SIZE: usize = 512;

/// ツールパネルの幅。
/// パネル幅 (= 隠蔽パネル `ui_conceal.rs::PANEL_W` と統一)。
/// 実機 FB R4: 「マスク全削除ボタン幅を揃えたい」要望 → 両パネル同幅 200 に。
const PANEL_W: f32 = 200.0;
/// パネル下端をウィンドウ下端から少し浮かせる余白。
const PANEL_BOTTOM_MARGIN: f32 = 20.0;
/// ScrollArea の最低高さ。極端に低いウィンドウでも操作領域を潰しすぎない。
const PANEL_MIN_BODY_H: f32 = 120.0;
/// ツールパネルの左上マージン。
const PANEL_MARGIN_X: f32 = 16.0;
const PANEL_MARGIN_Y: f32 = 60.0;
/// 消しゴム本文の内容高をまだ測定できていない最初のフレームで使う高さ。
/// 以降は `erase_panel_body_content_h` の実測値に追従する。
const PANEL_BODY_FALLBACK_H: f32 = 560.0;

fn erase_panel_outer_height(full_rect: egui::Rect, panel_pos: egui::Pos2) -> f32 {
    (full_rect.max.y - panel_pos.y - PANEL_BOTTOM_MARGIN).max(PANEL_MIN_BODY_H + 40.0)
}

/// Undo スタックの最大エントリ数。
const UNDO_MAX: usize = 20;

impl App {
    // ── モード開始/終了 ─────────────────────────────────────────────

    /// 消しゴムモードに入る。DB にマスクがあればロードする。
    ///
    /// 見開きモード中に呼ばれた場合は spread_mode を一時的に Single へ落とし、
    /// `fullscreen_idx` を見開きペアの左ページに固定する (消しゴムは単一ページ前提のため)。
    /// 終了時に `reset_erase_mode` が元の spread_mode を復元する。ペアの右ページへは
    /// パネルの「右ページ」ボタンで `switch_erase_target_in_spread` 経由で切り替える。
    /// 透明部を含む画像を「黒の不透明背景に合成」して全面不透明にする。
    /// egui::Color32 は premultiplied alpha なので、premultiplied RGB をそのまま alpha=255 に
    /// すれば黒背景への合成と等価になる (透明部 = premult RGB 0 = 黒、半透明 = 黒へ減衰)。
    /// MI-GAN は alpha を扱えず透明部を黒として補完するため、消しゴム作業時はこの「不透明黒」に
    /// 統一して WYSIWYG にする。全画素が既に不透明なら None (= 変換不要)。
    pub(crate) fn black_flatten_if_transparent(img: &egui::ColorImage) -> Option<egui::ColorImage> {
        if img.pixels.iter().all(|p| p.a() == 255) {
            return None;
        }
        let pixels = img
            .pixels
            .iter()
            .map(|p| egui::Color32::from_rgba_premultiplied(p.r(), p.g(), p.b(), 255))
            .collect();
        Some(egui::ColorImage::new(img.size, pixels))
    }

    pub(crate) fn enter_erase_mode(&mut self, fs_idx: usize) {
        // 見開きから入った場合は左ページへピボット。Single 起動 / 片側のみのページ
        // (表紙・末尾奇数・横長画像) では `resolve_spread_pair` が Single を返すので
        // ピボット処理はスキップされる。
        let spread_pair = match self.resolve_spread_pair(fs_idx) {
            crate::ui_fullscreen::SpreadPair::Double { left, right } => Some((left, right)),
            crate::ui_fullscreen::SpreadPair::Single => None,
        };
        let target_idx = spread_pair.map(|(l, _)| l).unwrap_or(fs_idx);
        // 消しゴム入力取得は state mutation より前にやる。ここで取れないと erase は始められず、
        // 取れる前に spread_mode / fullscreen_idx を弄ると見開きが解除されたまま
        // 編集も開始しない不整合状態になる (Codex P2 指摘)。
        let pixels = if let Some(base) = self.erase_base_cache.get(&target_idx) {
            Arc::clone(base)
        } else {
            let from_cache = self
                .fs_cache
                .get(&target_idx)
                .and_then(|entry| match entry {
                    FsCacheEntry::Static { pixels, .. } => Some(Arc::clone(pixels)),
                    _ => None,
                });
            match from_cache {
                Some(p) => {
                    // 初回: pre-erase 入力を base_cache に保存。透明 PNG は MI-GAN が alpha を
                    // 扱えず透明部を黒補完するため、ここで「黒で不透明化」したコピーを base に
                    // する (WYSIWYG: 表示も MI-GAN 入力も黒不透明)。fs_cache の透明原本は
                    // 無変更なので、マスク無しで消しゴムを抜ければ元の透明画像に戻る (P3-8 後続)。
                    let base = match Self::black_flatten_if_transparent(&p) {
                        Some(flat) => Arc::new(flat),
                        None => p,
                    };
                    self.erase_base_cache.insert(target_idx, Arc::clone(&base));
                    base
                }
                None => return,
            }
        };
        // ピクセル取得成功 → ここから state mutation。
        if let Some(pair) = spread_pair {
            self.erase_spread_ctx = Some(crate::app::EraseSpreadCtx {
                saved_mode: self.spread_mode,
                pair,
            });
            self.set_single_page_view(target_idx);
        }
        let fs_idx = target_idx;
        let [w, h] = pixels.size;
        self.erase_mode = true;
        // 通常表示と消しゴムは UI / Ctrl+Z の文脈が異なるので、メタ Undo スタックを破棄。
        // 消しゴム中は erase_undo_stack が Ctrl+Z を担当する。
        self.clear_meta_undo();
        // 新ページに入ったら preview cache を完全リセット (= 他ページの残骸を
        // 持ち込まない、Codex P1 R4 #1)。
        self.clear_erase_preview(fs_idx);
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
        self.erase_preview_active = false;
        // base texture cache は前回の state を引き継ぐ可能性があるので毎回 clear
        // (例: 別ページからの再入場で同 idx のテクスチャが残ると bad cross-talk)。
        self.erase_base_tex_cache.clear();
        self.erase_undo_stack.clear();
        self.erase_last_undo_at = None;
        self.erase_shapes.clear();
        self.erase_selected_shape = None;
        self.erase_drag = None;
        self.erase_panel_last_rect = None;
        self.erase_panel_body_content_h = None;
        self.erase_shape_drag_start = None;
        self.erase_shape_drag_end = None;

        // デフォルトブラシ半径: 長辺の 1/100
        if self.erase_brush_radius <= 0.0 {
            self.erase_brush_radius = (w.max(h) as f32 / 100.0).max(2.0);
        }
        // デフォルト直線幅: 長辺の 1/500 (細い線ノイズ除去に適した値)
        if self.erase_line_width <= 0.0 {
            self.erase_line_width = (w.max(h) as f32 / 500.0).max(2.0);
        }

        // DB からマスク (ビットマップ + ベクタ) をロード
        let (loaded_mask, loaded_vectors) = self
            .page_path_key(fs_idx)
            .and_then(|key| self.mask_db.as_ref()?.get_full(&key, w, h))
            .unwrap_or_else(|| (vec![false; w * h], Vec::new()));

        self.erase_mask = Some(loaded_mask);
        self.erase_shapes = loaded_vectors;
        crate::logger::log(format!(
            "erase: enter mode, image={w}x{h}, vectors={}",
            self.erase_shapes.len()
        ));
    }

    /// 消しゴムモードをリセットする。
    pub(crate) fn reset_erase_mode(&mut self) {
        let restore_idx = self.fullscreen_idx;
        let was_erase_mode = self.erase_mode;
        self.erase_mode = false;
        // 消しゴム → 通常表示への遷移境界でもメタ Undo をクリアする。
        // (enter_erase_mode と対称、行き来したときに残骸が残らない)
        if was_erase_mode {
            self.clear_meta_undo();
        }
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
        self.erase_preview_active = false;
        self.erase_base_tex_cache.clear();
        self.erase_undo_stack.clear();
        // **Preview cache を破棄** (Codex P1 R4 #1)。preview 完了が遅延して届く
        // ケースで `fs_cache` を汚染しないよう、pending preview job も cancel する。
        if let Some(idx) = restore_idx {
            self.clear_erase_preview(idx);
        }
        // モード退出時の念入りクリア (= 複数 idx 編集していたケース対応)。
        self.erase_preview_cache.clear();
        // is_preview=true の pending を全部 cancel (= 退出後に結果を届けない)。
        let preview_keys: Vec<EraseInpaintPendingKey> = self
            .erase_inpaint_pending
            .keys()
            .copied()
            .filter(|k| matches!(k.kind, EraseInpaintKind::Preview))
            .collect();
        for k in preview_keys {
            if let Some(prev) = self.erase_inpaint_pending.remove(&k) {
                prev.cancel.store(true, Ordering::Relaxed);
            }
        }
        self.erase_last_undo_at = None;
        self.erase_shapes.clear();
        self.erase_selected_shape = None;
        self.erase_drag = None;
        self.erase_panel_last_rect = None;
        self.erase_panel_body_content_h = None;
        self.erase_shape_drag_start = None;
        self.erase_shape_drag_end = None;
        self.fs_pan_drag_start = None;

        // 見開きから入っていた場合は spread_mode と表示ページを復元する。
        // ページ位置は left_idx に揃える (resolve_spread_pair が同じペアを返すので
        // 元と同じ見開きが再構築される)。ズーム/パンはリセット。
        // ※ `set_single_page_view` を使わないのは spread_mode を Single に倒さない
        //   ため (= 見開きへ復帰する経路)。
        if let Some(ctx) = self.erase_spread_ctx.take() {
            self.spread_mode = ctx.saved_mode;
            self.fullscreen_idx = Some(ctx.pair.0);
            self.fs_zoom = 1.0;
            self.fs_pan = egui::Vec2::ZERO;
        }
    }

    /// 単一ページ表示用に状態を初期化する: spread_mode を Single に倒し、
    /// `fullscreen_idx` を `idx` に固定し、ズーム/パンをリセット。
    /// `enter_erase_mode` のピボット先と `switch_erase_target_in_spread` の
    /// ページ切替先で同じシーケンスを使うので共通化している。
    fn set_single_page_view(&mut self, idx: usize) {
        self.spread_mode = crate::settings::SpreadMode::Single;
        self.fullscreen_idx = Some(idx);
        self.fs_zoom = 1.0;
        self.fs_pan = egui::Vec2::ZERO;
    }

    /// 見開き消しゴム中に「左ページ」「右ページ」ボタンで編集対象を切り替える。
    /// 既存の編集をそのページの inpaint として保存・適用してから、もう一方のページに
    /// 移動して新たに編集モードへ入る ([E] 適用 → 移動 → [E] 開始 と等価)。Undo は
    /// ページごとに独立 (= 切替時にスタックは捨てる)、ズーム/パンは初期化する。
    pub(crate) fn switch_erase_target_in_spread(&mut self, ctx: &egui::Context, new_idx: usize) {
        // 同ページなら no-op
        if self.fullscreen_idx == Some(new_idx) {
            return;
        }
        // 現ページの編集を確定。`apply_inpaint_only` は inpaint 投入だけ行い、
        // `erase_spread_ctx` を含む消しゴム状態は壊さない (spread 切替を意識した版)。
        // マスクが空なら inpaint も DB 書き込みも走らないので、空 toggle でも安全。
        if let Some(idx) = self.fullscreen_idx {
            self.apply_inpaint_only(ctx, idx);
        }
        // 新ページへピボット。`erase_spread_ctx` は触らないので panel ボタンは
        // そのまま左/右トグルとして使い続けられる。
        self.set_single_page_view(new_idx);
        // 新ページで編集モード開始。spread_mode = Single なので enter 内のペア再判定は
        // スキップされ、`erase_spread_ctx` はそのまま保たれる。
        self.enter_erase_mode(new_idx);
    }

    // ── Undo / Slot ────────────────────────────────────────────────

    pub(crate) fn push_undo_snapshot(&mut self) {
        if let Some(mask) = &self.erase_mask {
            self.erase_undo_stack.push_back(EraseSnapshot {
                mask: mask.clone(),
                shapes: self.erase_shapes.clone(),
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
            self.erase_shapes = prev.shapes;
            self.erase_selected_shape = None;
            self.erase_drag = None;
            self.erase_mask_texture = None;
            // mask 復元 → preview cache 破棄。
            if let Some(fs_idx) = self.fullscreen_idx {
                self.clear_erase_preview(fs_idx);
            }
            true
        } else {
            false
        }
    }

    /// 現在のマスク (ビットマップ + ベクタ) をスロットに保存する。
    pub(crate) fn save_mask_to_slot(&mut self, slot: usize) {
        let [w, h] = self.erase_mask_size;
        let saved = if let (Some(mask), Some(db)) = (&self.erase_mask, &self.mask_db) {
            db.set_slot(slot, mask, &self.erase_shapes, w, h).is_ok()
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
        let slot_data = self
            .mask_db
            .as_ref()
            .and_then(|db| db.get_slot_full(slot, w, h));
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
        self.erase_shapes = slot_vectors;
        self.erase_selected_shape = None;
        self.erase_mask_texture = None;
        // mask 差し替え → preview cache 破棄。
        if let Some(fs_idx) = self.fullscreen_idx {
            self.clear_erase_preview(fs_idx);
        }
        self.show_feedback_toast(format!("[スロット{}をロード]", slot));
    }

    // ── キー入力 ──────────────────────────────────────────────────

    /// 消しゴムモード中のキー入力を処理する。
    /// 通常のフルスクリーンショートカットをブロックし、消しゴム専用キーのみ有効にする。
    pub(crate) fn handle_erase_keys(&mut self, ctx: &egui::Context, fs_idx: usize) -> FsKeyAction {
        let action = FsKeyAction {
            close: false,
            nav_delta: 0,
            ctrl_nav: None,
            jump_to: None,
        };

        // ESC: 選択があればまず解除、無ければマスクを適用 (E と同じ挙動) して終了
        //
        // 旧版は ESC でマスクを DB に保存するだけで inpaint を実行しなかったため、
        // 画像には反映されていないのに次回開くとマスクは残っている、という分かりにくい
        // 状態になっていた。明示破棄したい場合はマスク自体を削除してから抜ける。
        let esc = ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Escape));
        if esc {
            if self.erase_selected_shape.is_some() {
                self.erase_selected_shape = None;
                self.erase_drag = None;
                self.erase_mask_texture = None;
                return action;
            }
            self.execute_erase_inpaint(ctx, fs_idx);
            return action;
        }

        // E: inpaint 実行 (ESC と同じく execute_erase_inpaint を呼ぶ)
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
            if let Some(idx) = self.erase_selected_shape {
                if idx < self.erase_shapes.len() {
                    self.push_undo_snapshot();
                    self.erase_shapes.remove(idx);
                    self.erase_selected_shape = None;
                    self.erase_drag = None;
                    self.erase_mask_texture = None;
                    // shape を消したので preview cache も破棄しないと「shape を消した
                    // 直後の再 preview で古い inpaint 結果が一瞬見える」状態が残る
                    // (Codex R5 P2)。
                    if let Some(fs_idx) = self.fullscreen_idx {
                        self.clear_erase_preview(fs_idx);
                    }
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
            self.nudge_mask(dx, dy);
        }

        // [ / ]: 回転 (Ctrl で 1°)
        let rot_step = if ctrl_held {
            ROTATE_DEG_STEP_FAST
        } else {
            ROTATE_DEG_STEP
        };
        let mut rot_deg = 0.0f32;
        ctx.input_mut(|i| {
            if i.consume_key(egui::Modifiers::NONE, egui::Key::OpenBracket)
                || i.consume_key(egui::Modifiers::CTRL, egui::Key::OpenBracket)
            {
                rot_deg -= rot_step;
            }
            if i.consume_key(egui::Modifiers::NONE, egui::Key::CloseBracket)
                || i.consume_key(egui::Modifiers::CTRL, egui::Key::CloseBracket)
            {
                rot_deg += rot_step;
            }
        });
        if rot_deg != 0.0 {
            self.rotate_mask(rot_deg.to_radians());
        }

        // S/B/L/V/H/I/R/O: ツール切替 (Phase 0b で R / O = 矩形 / 楕円 を追加)
        let key_s_tool = ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::S));
        let key_b = ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::B));
        let key_l = ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::L));
        let key_v = ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::V));
        let key_h = ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::H));
        let key_i = ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::I));
        let key_r_tool = ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::R));
        let key_o_tool = ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::O));
        if key_s_tool {
            self.switch_erase_tool(EraseTool::Select, "[選択]");
        }
        if key_b {
            self.switch_erase_tool(EraseTool::Brush, "[筆]");
        }
        if key_l {
            self.switch_erase_tool(EraseTool::Lasso, "[囲み]");
        }
        if key_v {
            self.switch_erase_tool(EraseTool::VertLine, "[縦線]");
        }
        if key_h {
            self.switch_erase_tool(EraseTool::HorizLine, "[横線]");
        }
        if key_i {
            self.switch_erase_tool(EraseTool::Line, "[直線]");
        }
        if key_r_tool {
            self.switch_erase_tool(EraseTool::Rect, "[矩形]");
        }
        if key_o_tool {
            self.switch_erase_tool(EraseTool::Ellipse, "[楕円]");
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
        // Phase 0b: I / R / O はツール切替に使うので SINGLE_KEYS から除外。
        // (I は直線、R は矩形、O は楕円)
        const SINGLE_KEYS: &[egui::Key] = &[
            egui::Key::Space,
            egui::Key::Tab,
            egui::Key::Z,
            egui::Key::G,
            egui::Key::M,
            egui::Key::P,
            egui::Key::T,
            egui::Key::U,
            egui::Key::N,
            egui::Key::F1,
            egui::Key::F2,
            egui::Key::F3,
            egui::Key::F4,
            egui::Key::F5,
            egui::Key::F6,
        ];
        // 数字キーは全て消費 (スロット系ショートカットは廃止、誤動作を防ぐ)
        const NUM_KEYS: &[egui::Key] = &[
            egui::Key::Num0,
            egui::Key::Num1,
            egui::Key::Num2,
            egui::Key::Num3,
            egui::Key::Num4,
            egui::Key::Num5,
            egui::Key::Num6,
            egui::Key::Num7,
            egui::Key::Num8,
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
        match self.erase_selected_shape {
            Some(idx) if idx < self.erase_shapes.len() => {
                self.erase_shapes[idx].translate(dx, dy);
            }
            _ => {
                // 全ベクタを移動
                for v in &mut self.erase_shapes {
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
        // mask 変化 → preview cache 破棄。
        if let Some(fs_idx) = self.fullscreen_idx {
            self.clear_erase_preview(fs_idx);
        }
    }

    /// マスクを回転する。選択中ベクタがあればそれだけ、無ければ全体を画像中心周りに回転する。
    fn rotate_mask(&mut self, angle_rad: f32) {
        self.push_undo_snapshot_throttled();
        match self.erase_selected_shape {
            Some(idx) if idx < self.erase_shapes.len() => {
                let center = self.erase_shapes[idx].center();
                self.erase_shapes[idx].rotate_around(center.0, center.1, angle_rad);
            }
            _ => {
                let [w, h] = self.erase_mask_size;
                let cx = w as f32 * 0.5;
                let cy = h as f32 * 0.5;
                for v in &mut self.erase_shapes {
                    v.rotate_around(cx, cy, angle_rad);
                }
                if let Some(mask) = self.erase_mask.as_mut() {
                    rotate_bitmap(mask, w, h, cx, cy, angle_rad);
                }
            }
        }
        self.erase_mask_texture = None;
        // mask 変化 → preview cache 破棄。
        if let Some(fs_idx) = self.fullscreen_idx {
            self.clear_erase_preview(fs_idx);
        }
    }

    // ── 座標変換 ──────────────────────────────────────────────────

    /// 画像レイアウト情報 (total_scale, img_rect) を計算する。
    fn erase_image_layout(
        &self,
        full_rect: egui::Rect,
        zoom_pan: Option<(f32, egui::Vec2)>,
    ) -> Option<(f32, egui::Rect)> {
        let [iw, ih] = self.erase_mask_size;
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
        let (total_scale, img_rect) = self
            .erase_image_layout(full_rect, zoom_pan)
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
        // mask 変化 → preview cache を破棄。
        if let Some(fs_idx) = self.fullscreen_idx {
            self.clear_erase_preview(fs_idx);
        }
    }

    /// 多角形の内部をビットマップに塗る/消す。`mask_db::scanline_fill_polygon` の薄いラッパ。
    fn paint_polygon(&mut self, points: &[(f32, f32)], paint: bool) {
        let [w, h] = self.erase_mask_size;
        let Some(mask) = self.erase_mask.as_mut() else {
            return;
        };
        crate::mask_db::scanline_fill_polygon(mask, points, w, h, paint);
        self.erase_mask_texture = None;
        // mask 変化 → preview cache を破棄。
        if let Some(fs_idx) = self.fullscreen_idx {
            self.clear_erase_preview(fs_idx);
        }
    }

    // ── マスクテクスチャ ──────────────────────────────────────────

    fn ensure_mask_texture(&mut self, ctx: &egui::Context) {
        if self.erase_mask_texture.is_some() {
            return;
        }
        let Some(composite) = self.composite_mask() else {
            return;
        };
        let [w, h] = self.erase_mask_size;
        let mut rgba = vec![0u8; w * h * 4];
        for i in 0..composite.len() {
            if composite[i] {
                rgba[i * 4] = 255;
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
        if w == 0 || h == 0 {
            return None;
        }
        // 早期 return: ビットマップに 1 つも true が無く、ベクタも空ならクローン不要。
        // 4K ページでは bitmap.clone() ≈ 33MB なので、見開き左/右トグル等で空マスク
        // のまま `composite_mask` が呼ばれるケースで無駄なアロケーションを避ける。
        if self.erase_shapes.is_empty() && !mask.iter().any(|&b| b) {
            return Some(vec![false; w * h]);
        }
        let mut out = mask.clone();
        crate::mask_db::rasterize_shapes_into(&mut out, &self.erase_shapes, w, h);
        Some(out)
    }

    // ── ベクタオブジェクトのヒットテスト・ドラッグ編集 ──────────────

    /// 画像座標 `pos` から、Shape のホバーターゲットを判定する。
    /// Phase 2b で `vector_edit` 経由に統一。`ui_conceal::hit_test_conceal` と同じロジック:
    ///
    /// 1. 選択中 Shape のハンドル (Body 以外) を最優先
    /// 2. 新しい順 (添字大→小) に Body 判定
    fn hit_test_erase(
        &self,
        pos: (f32, f32),
        scale: f32,
    ) -> Option<(usize, vector_edit::HoverTarget)> {
        if let Some(sel) = self.erase_selected_shape {
            if let Some(s) = self.erase_shapes.get(sel) {
                let layout = vector_edit::compute_handle_layout(s, scale);
                if let Some(t) = vector_edit::hit_test(&layout, pos, scale) {
                    if !matches!(t, vector_edit::HoverTarget::Body) {
                        return Some((sel, t));
                    }
                }
            }
        }
        for (i, s) in self.erase_shapes.iter().enumerate().rev() {
            let layout = vector_edit::compute_handle_layout(s, scale);
            if point_in_polygon(pos, &layout.body_corners) {
                return Some((i, vector_edit::HoverTarget::Body));
            }
        }
        None
    }

    /// 消しゴムツールを切り替える共通ヘルパー。
    /// 同じツールへの切替は no-op、別ツールへの切替時は選択をクリアして次の commit
    /// で再選択させる。トーストも表示する。
    fn switch_erase_tool(&mut self, tool: EraseTool, toast: &str) {
        if self.erase_tool == tool {
            return;
        }
        let entering_select = tool == EraseTool::Select;
        self.erase_tool = tool;
        // ツール切替時は選択をクリア (= 別ツールに移ったので前 shape の編集を終了)
        // (Codex P1 対応、実機 FB R3)。
        // ただし **Select に入る場合は erase_selected_shape を保持** する: 直前に
        // 別ツールで commit_erase_shape が auto-select した shape の編集を [S] で
        // すぐ始められるようにするのが UX 意図 (commit_erase_shape のコメント参照、
        // code-review CONFIRMED)。
        if !entering_select {
            self.erase_selected_shape = None;
        }
        self.erase_drag = None;
        self.erase_mask_texture = None;
        // ツールごとに slider 行の有無が変わるためパネル本文高も変わる。前ツール
        // の measured 値を残すと 1 frame だけ slider が clip されたり余白が出る
        // (Phase 1-5 code-review CONFIRMED)。
        self.erase_panel_body_content_h = None;
        self.show_feedback_toast(toast.to_string());
    }

    /// Select 以外のツールでも「直近の選択 shape のハンドル」を操作できるよう、
    /// ツール dispatch の前に走らせる共通処理 (隠蔽パネルと同じパターン)。
    ///
    /// 戻り値 `true` のときは呼び出し側で `return` して通常ツール処理を skip する:
    /// - 既に `erase_drag` が立っている (= 進行中のハンドル操作) → 更新して継続
    /// - 新規 primary_pressed で選択中 shape の **ハンドル (= Body 以外)** に
    ///   ヒットした → drag を仕込む
    ///
    /// 戻り値 `false` のときは通常ツール処理 (= 新規 shape 作成) に進む。
    fn try_handle_active_erase_drag_or_handle_hit(
        &mut self,
        primary_pressed: bool,
        primary_released: bool,
        pointer_pos: Option<egui::Pos2>,
        full_rect: egui::Rect,
        zoom_pan: Option<(f32, egui::Vec2)>,
        modifiers: egui::Modifiers,
    ) -> bool {
        // ① 進行中のドラッグがあれば最優先で処理
        if let Some(drag) = self.erase_drag {
            let img_pos_opt = pointer_pos.and_then(|p| {
                self.erase_image_layout(full_rect, zoom_pan)
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
                if drag_idx < self.erase_shapes.len() {
                    self.erase_shapes[drag_idx] = new_shape;
                }
                // R5: ドラッグ中の毎フレ再描画 (= ハンドル / Pan を動かすたびに
                // マスク overlay を作り直して画像上に反映)。
                self.erase_mask_texture = None;
                // shape の geometry が変わったので preview cache (= 古い MI-GAN
                // 結果) を捨てる (Codex R5 P2: 「preview → 移動/変形 → 再 preview」で
                // 再計算中の古い preview が見える漏れ)。
                if let Some(fs_idx) = self.fullscreen_idx {
                    self.clear_erase_preview(fs_idx);
                }
            }
            if primary_released {
                self.erase_drag = None;
                self.erase_mask_texture = None;
            }
            return true;
        }
        // ② 新規 primary_pressed で、選択中 shape の handle (Body 以外) なら drag 開始
        if !primary_pressed {
            return false;
        }
        let Some(sel) = self.erase_selected_shape else {
            return false;
        };
        let Some(screen) = pointer_pos else {
            return false;
        };
        let Some((scale, img_rect)) = self.erase_image_layout(full_rect, zoom_pan) else {
            return false;
        };
        let img_pos = (
            (screen.x - img_rect.min.x) / scale,
            (screen.y - img_rect.min.y) / scale,
        );
        let Some(shape) = self.erase_shapes.get(sel).copied() else {
            return false;
        };
        let layout = vector_edit::compute_handle_layout(&shape, scale);
        let Some(target) = vector_edit::hit_test(&layout, img_pos, scale) else {
            return false;
        };
        // 選択中 shape の **Body** クリックは描画モード時のみ Pan ドラッグとして
        // 消費する。**消去モード (F)** では fallthrough して、同じ領域に重ねて
        // 消去 shape を作る動作を許可する (Codex P2 R4 #2: 「描画モードで選択 →
        // F で消去 → 領域内をクリック」が「移動」になってしまう問題)。
        //
        // 非選択 shape の Body クリックはそもそもここに来ない (= layout は
        // 選択中 shape 限定で計算しているため)。
        if matches!(target, vector_edit::HoverTarget::Body) && !self.erase_paint_mode {
            return false;
        }
        self.push_undo_snapshot();
        self.erase_drag = Some(vector_edit::begin_drag(target, sel, shape, img_pos));
        self.erase_mask_texture = None;
        true
    }

    /// vector_edit ベースのドラッグ更新。`erase_drag` (Option<DragState>) と現在位置から
    /// 新しい Shape を計算して `erase_shapes[idx]` に書き戻す。
    fn update_erase_drag(
        &mut self,
        pointer_pos: Option<egui::Pos2>,
        full_rect: egui::Rect,
        zoom_pan: Option<(f32, egui::Vec2)>,
        modifiers: egui::Modifiers,
    ) {
        let Some(screen) = pointer_pos else {
            return;
        };
        let Some((total_scale, img_rect)) = self.erase_image_layout(full_rect, zoom_pan) else {
            return;
        };
        let cur = (
            (screen.x - img_rect.min.x) / total_scale,
            (screen.y - img_rect.min.y) / total_scale,
        );

        let Some(drag) = self.erase_drag else {
            return;
        };
        let idx = drag.idx();
        if idx >= self.erase_shapes.len() {
            return;
        }
        let new_shape = vector_edit::apply_drag(&drag, cur, &modifiers);
        self.erase_shapes[idx] = new_shape;
        // R5: ドラッグ中もマスクを毎フレ再生成する (実機 FB: 「ハンドルのドラッグでも
        // 楕円は再描画されません。性能的に問題なければリアルタイムにマスクを表示」)。
        // mask_texture を None に落とすと次フレームの ensure_mask_texture で再生成される。
        // 4K RGBA だと 32MB のテクスチャアップロードになるが、ユーザーが要望した UX を
        // 優先する (旧最適化コメントは削除)。
        self.erase_mask_texture = None;
        // shape geometry 変化 → 古い preview cache (= MI-GAN 結果) を捨てる
        // (Codex R5 P2: 移動/変形後の再 preview で古い結果が一瞬見える漏れ)。
        if let Some(fs_idx) = self.fullscreen_idx {
            self.clear_erase_preview(fs_idx);
        }
    }

    /// ツールパネルの矩形を返す。
    fn erase_panel_rect(&self, full_rect: egui::Rect) -> egui::Rect {
        let panel_pos = egui::pos2(
            full_rect.min.x + PANEL_MARGIN_X,
            full_rect.min.y + PANEL_MARGIN_Y,
        );
        let max_h = erase_panel_outer_height(full_rect, panel_pos);
        if let Some(rect) = self.erase_panel_last_rect {
            let same_origin =
                (rect.min.x - panel_pos.x).abs() < 2.0 && (rect.min.y - panel_pos.y).abs() < 2.0;
            if same_origin && rect.is_positive() {
                return rect.intersect(egui::Rect::from_min_size(
                    panel_pos,
                    egui::vec2(PANEL_W + 24.0, max_h),
                ));
            }
        }
        // 実描画 rect がまだ無い (= 初フレーム / ウィンドウサイズ変動直後) は、
        // **狭い** PANEL_W 幅で取る。旧版は固定 650px 高で初フレに広めに取って
        // いたため、パネル下に出ようとした 1 stroke を奪うことがあった
        // (Phase 1-5 code-review CONFIRMED)。
        // 高さもヘッダ + ボタン 2 行 ぶん (= ~120px) に抑え、次フレに正確な rect で
        // 上書きする方が安全。実害はパネル下端へ即クリックが届くケースで
        // 「初フレだけ image 側に抜ける」だが、初フレで brush stroke を始めるのは
        // ほぼ起こらないので許容する。
        egui::Rect::from_min_size(panel_pos, egui::vec2(PANEL_W, PANEL_MIN_BODY_H.min(max_h)))
    }

    // ── 入力処理 ──────────────────────────────────────────────────

    /// ドラッグ入力を処理する（ツール別分岐）。
    pub(crate) fn handle_erase_paint(
        &mut self,
        ctx: &egui::Context,
        full_rect: egui::Rect,
        zoom_pan: Option<(f32, egui::Vec2)>,
    ) {
        // フォーカス復帰クリック中は塗り・選択操作を一切発生させない
        // (handle_fs_wheel_and_click で検出・セットされる)
        if self.fs_suppress_primary_until_release {
            return;
        }
        let primary_down = ctx.input(|i| i.pointer.primary_down());
        let primary_pressed = ctx.input(|i| i.pointer.primary_pressed());
        let primary_released = ctx.input(|i| i.pointer.primary_released());
        let pointer_pos = ctx.input(|i| i.pointer.hover_pos());
        let paint = self.erase_paint_mode;
        let space_held = ctx.input(|i| i.key_down(egui::Key::Space));

        // パネル上のクリックはツール操作に使わない。
        //
        // ⚠ ただし `primary_released` のフレームだけは通す。これがないと、
        // canvas でハンドル/線/シェイプドラッグ中にパネル上で離した場合、
        // primary_released がツールハンドラに届かず `erase_drag` / `erase_line_*` /
        // `erase_shape_drag_*` などの中間状態が残ったままになる
        // (Codex P2 R3 #2、隠蔽側 `ui_conceal.rs::handle_conceal_paint` の同条件
        // と揃える)。
        let panel_rect = self.erase_panel_rect(full_rect);
        if let Some(pos) = pointer_pos
            && panel_rect.contains(pos)
            && !primary_released
        {
            return;
        }

        // ── Space+ドラッグ: 一時パン (Photoshop 流) ─────────────────
        // 描画ドラッグ進行中は Space を無視し、現在の描画を最後まで完結させる。
        // (途中で Space 検知 → パンに切替するとマスクが中途半端に確定するため)
        let drawing_in_progress = self.erase_last_paint_pos.is_some()
            || self.erase_line_start.is_some()
            || self.erase_shape_drag_start.is_some()
            || self.erase_shift_drag.is_some()
            || self.erase_drag.is_some()
            || !self.erase_lasso_points.is_empty();
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
        // Space 離した瞬間の取りこぼし対策: 描画パスへ戻る前に pan drag を片付ける。
        if !space_held && self.fs_pan_drag_start.is_some() {
            self.fs_pan_drag_start = None;
        }

        // 修飾キーは Ctrl で統一: [/] キーは Shift+ が論理キー {/} に化ける制約があり
        // 回転系を Ctrl にしたため、パン・ツール時のフィット調整もすべて Ctrl に揃える。
        let ctrl_held = ctx.input(|i| i.modifiers.ctrl);

        // ── ベクタオブジェクト編集パス (選択ツール時のみ) ───────────
        // 選択ツール中はドロー系の操作を行わず、クリック=選択/ハンドルドラッグ=編集
        // に徹する。Phase 2b で vector_edit に統一: 角・辺中点・回転ハンドル + 端点。
        // Shift = 軸拘束/等比/15°snap、Alt = 中心固定。
        let modifiers = ctx.input(|i| i.modifiers);
        if self.erase_tool == EraseTool::Select {
            if primary_pressed {
                if let Some((scale, img_rect)) = self.erase_image_layout(full_rect, zoom_pan) {
                    if let Some(screen) = pointer_pos {
                        let img_pos = (
                            (screen.x - img_rect.min.x) / scale,
                            (screen.y - img_rect.min.y) / scale,
                        );
                        if let Some((idx, target)) = self.hit_test_erase(img_pos, scale) {
                            self.push_undo_snapshot();
                            self.erase_selected_shape = Some(idx);
                            let base = self.erase_shapes[idx];
                            self.erase_drag =
                                Some(vector_edit::begin_drag(target, idx, base, img_pos));
                            self.erase_mask_texture = None;
                        } else if self.erase_selected_shape.is_some() {
                            self.erase_selected_shape = None;
                            self.erase_drag = None;
                            self.erase_mask_texture = None;
                        }
                    }
                }
            }
            if self.erase_drag.is_some() {
                self.update_erase_drag(pointer_pos, full_rect, zoom_pan, modifiers);
                if primary_released {
                    self.erase_drag = None;
                    self.erase_mask_texture = None;
                }
            }
            return;
        }

        // ⚠ 旧版は「他ツールに切り替わったら選択を自動解除」していたが、これだと
        // `commit_erase_shape` で自動選択した直後のフレームで選択が消されて
        // 「ハンドルが一瞬だけ出る」現象になっていた (Codex P1 / 実機 FB R3)。
        //
        // 新方針: 選択はツール enum 値が変わったときのみクリアする (= ツール切替
        // ボタン / ショートカット押下のときに明示クリア)。ツール継続中は選択を保持。

        // ── 共通ハンドル処理 (ツール非依存): 直近 shape のハンドルが操作中なら
        //    そちらを優先処理して、新規 shape 作成側に流さない。
        if self.try_handle_active_erase_drag_or_handle_hit(
            primary_pressed,
            primary_released,
            pointer_pos,
            full_rect,
            zoom_pan,
            modifiers,
        ) {
            return;
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
                                    Some(ShiftDragState::BrushSize { base_radius, .. }) => {
                                        base_radius
                                    }
                                    _ => {
                                        self.erase_shift_drag = Some(ShiftDragState::BrushSize {
                                            origin: img_pos,
                                            base_radius: self.erase_brush_radius,
                                        });
                                        self.erase_brush_radius
                                    }
                                };
                                if let Some(ShiftDragState::BrushSize { origin, .. }) =
                                    self.erase_shift_drag
                                {
                                    let delta = (img_pos.0 - origin.0) + (img_pos.1 - origin.1);
                                    let max_r = self.erase_mask_size[0].max(self.erase_mask_size[1])
                                        as f32
                                        / 20.0;
                                    self.erase_brush_radius =
                                        (base_radius + delta).clamp(1.0, max_r);
                                }
                            } else {
                                self.erase_shift_drag = None;
                                if self.erase_last_paint_pos.is_none() {
                                    self.push_undo_snapshot();
                                }
                                let prev = self
                                    .erase_last_paint_pos
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
                            if self
                                .erase_lasso_points
                                .last()
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
                    primary_down,
                    primary_released,
                    pointer_pos,
                    ctrl_held,
                    paint,
                    full_rect,
                    zoom_pan,
                    true,
                );
            }
            EraseTool::HorizLine => {
                self.handle_line_tool_paint(
                    primary_down,
                    primary_released,
                    pointer_pos,
                    ctrl_held,
                    paint,
                    full_rect,
                    zoom_pan,
                    false,
                );
            }
            EraseTool::Line => {
                // 旧 Ctrl+ドラッグ太さ変更モディファイヤは廃止 (実機 FB R3)。
                // 線幅はパネル slider で設定し、引いた直後に自動選択される shape を
                // S ツールでハンドル操作することで微調整する設計に統一する
                // (= 矩形/楕円/縦線/横線と同じワークフロー)。
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
                    if let (Some((x0, y0)), Some((x1, y1))) =
                        (self.erase_line_start, self.erase_line_end)
                    {
                        let dx = x1 - x0;
                        let dy = y1 - y0;
                        let len = (dx * dx + dy * dy).sqrt();
                        if len > 1.0 {
                            self.push_undo_snapshot();
                            let shape = Shape::Line {
                                op: ShapeOp::Add,
                                kind: LineKind::Diagonal,
                                p0: (x0, y0),
                                p1: (x1, y1),
                                thickness: self.erase_line_width.max(1.0),
                            };
                            self.commit_erase_shape(shape, paint);
                            self.erase_mask_texture = None;
                        }
                    }
                    self.erase_line_start = None;
                    self.erase_line_end = None;
                }
            }
            EraseTool::Rect | EraseTool::Ellipse => {
                // 矩形 / 楕円: 始点 → 終点の bbox を Shape::Rect / Shape::Ellipse に変換。
                // Ctrl 等の修飾はここでは使わない (作成後に Select ツールで Shift/Alt
                // ハンドル編集する設計)。
                if primary_down {
                    if let Some(pos) = pointer_pos {
                        if let Some(img_pos) = self.screen_to_image_f32(pos, full_rect, zoom_pan) {
                            if self.erase_shape_drag_start.is_none() {
                                self.erase_shape_drag_start = Some(img_pos);
                            }
                            self.erase_shape_drag_end = Some(img_pos);
                        }
                    }
                }
                if primary_released {
                    if let (Some(start), Some(end)) =
                        (self.erase_shape_drag_start, self.erase_shape_drag_end)
                    {
                        let dx = end.0 - start.0;
                        let dy = end.1 - start.1;
                        if dx.abs() > 1.0 && dy.abs() > 1.0 {
                            self.push_undo_snapshot();
                            let cx = (start.0 + end.0) * 0.5;
                            let cy = (start.1 + end.1) * 0.5;
                            let hw = dx.abs() * 0.5;
                            let hh = dy.abs() * 0.5;
                            let shape = match self.erase_tool {
                                EraseTool::Rect => Shape::Rect {
                                    op: ShapeOp::Add,
                                    center: (cx, cy),
                                    half_w: hw,
                                    half_h: hh,
                                    rotation_rad: 0.0,
                                },
                                EraseTool::Ellipse => Shape::Ellipse {
                                    op: ShapeOp::Add,
                                    center: (cx, cy),
                                    rx: hw,
                                    ry: hh,
                                    rotation_rad: 0.0,
                                },
                                _ => unreachable!(),
                            };
                            self.commit_erase_shape(shape, paint);
                            self.erase_mask_texture = None;
                        }
                    }
                    self.erase_shape_drag_start = None;
                    self.erase_shape_drag_end = None;
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
                            Some(ShiftDragState::LineAdjust {
                                base_tilt,
                                base_start,
                                base_end,
                                ..
                            }) => (base_tilt, base_start, base_end),
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
                        if let Some(ShiftDragState::LineAdjust { origin, .. }) =
                            self.erase_shift_drag
                        {
                            let dx = img_pos.0 - origin.0;
                            let dy = img_pos.1 - origin.1;
                            // 縦線: 向きに沿う軸 (Y) に沿ったドラッグは幅を変えず、直交する X ドラッグでパン・Y ドラッグで回転
                            // 横線: X/Y が入れ替わる
                            let (pan_x, pan_y, tilt_delta) = if is_vertical {
                                (dx, 0.0, dy)
                            } else {
                                (0.0, dy, dx)
                            };
                            self.erase_line_start =
                                Some((base_start.0 + pan_x, base_start.1 + pan_y));
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
                    let shape = Shape::Line {
                        op: ShapeOp::Add,
                        kind: LineKind::Vertical,
                        p0: (cx + tilt, 0.0),
                        p1: (cx, h as f32),
                        thickness,
                    };
                    self.commit_erase_shape(shape, paint);
                } else {
                    let ty = start.1.min(end.1);
                    let by = start.1.max(end.1);
                    let thickness = (by - ty).max(1.0);
                    let cy = (ty + by) * 0.5;
                    let shape = Shape::Line {
                        op: ShapeOp::Add,
                        kind: LineKind::Horizontal,
                        p0: (0.0, cy),
                        p1: (w as f32, cy + tilt),
                        thickness,
                    };
                    self.commit_erase_shape(shape, paint);
                }
                self.erase_mask_texture = None;
            }
            self.erase_line_start = None;
            self.erase_line_end = None;
            self.erase_line_tilt = 0.0;
            self.erase_shift_drag = None;
        }
    }

    /// 描画モードなら Add Shape、消去モードなら Subtract Shape を追加する。
    ///
    /// 筆/囲みで作るビットマップマスクを下地にし、その上に Shape を作成順で重ねる。
    /// そのため消去モードの矩形/楕円/線は既存 Shape を丸ごと削除せず、上から
    /// 削り取るベクターオブジェクトとして残る。
    fn commit_erase_shape(&mut self, shape: Shape, paint: bool) {
        // マスクが変わるので preview cache を破棄 (= 次回 preview 押下で再投入)。
        if let Some(fs_idx) = self.fullscreen_idx {
            self.clear_erase_preview(fs_idx);
        }
        let op = if paint {
            ShapeOp::Add
        } else {
            ShapeOp::Subtract
        };
        self.erase_shapes.push(shape.with_op(op));
        // 新規 shape を自動選択 (実機 FB)。コミット直後にハンドルが描画され、
        // [S] で選択ツールへ切替後に位置/サイズ/回転/太さを微調整できる。
        self.erase_selected_shape = Some(self.erase_shapes.len() - 1);
    }

    // (Phase 2b で `erase_shapes_overlapping_polygon` は削除。消去モードの Shape は
    //  既存 Shape を削除せず、Subtract Shape として作成順に合成する。)

    // ── 描画 ──────────────────────────────────────────────────────

    /// マスクオーバーレイ + ツールパネル + カーソルを描画する。
    pub(crate) fn draw_erase_overlay(
        &mut self,
        ui: &mut egui::Ui,
        ctx: &egui::Context,
        full_rect: egui::Rect,
        zoom_pan: Option<(f32, egui::Vec2)>,
    ) {
        // マスクオーバーレイ描画。プレビュー押下中は inpaint 反映後の結果を見せたいので
        // マスク表示はオフにする (= ユーザー要望: プレビュー中はマスク非表示)。
        if !self.erase_preview_active {
            self.ensure_mask_texture(ctx);
            if let Some(ref tex) = self.erase_mask_texture {
                let Some((_total_scale, img_rect)) = self.erase_image_layout(full_rect, zoom_pan)
                else {
                    return;
                };
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
        self.draw_shape_outlines(ui, full_rect, zoom_pan);

        // 選択中の Shape のハンドル (Phase 2b: vector_edit::draw_handles に委譲)
        if let Some(idx) = self.erase_selected_shape {
            if let Some(shape) = self.erase_shapes.get(idx) {
                if let Some((scale, _img_rect)) = self.erase_image_layout(full_rect, zoom_pan) {
                    let layout = vector_edit::compute_handle_layout(shape, scale);
                    let painter = ui.painter().with_clip_rect(full_rect);
                    let hovered = ui.ctx().input(|i| i.pointer.hover_pos()).and_then(|p| {
                        let img_rect = self.erase_image_layout(full_rect, zoom_pan)?.1;
                        let img_pos = (
                            (p.x - img_rect.min.x) / scale,
                            (p.y - img_rect.min.y) / scale,
                        );
                        vector_edit::hit_test(&layout, img_pos, scale)
                    });
                    let to_screen =
                        |p: (f32, f32)| self.image_to_screen(p.0, p.1, full_rect, zoom_pan);
                    vector_edit::draw_handles(&painter, &layout, true, hovered, &to_screen);
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
                let pts: Vec<egui::Pos2> = self
                    .erase_lasso_points
                    .iter()
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
                        egui::Stroke::new(
                            1.0,
                            egui::Color32::from_rgba_unmultiplied(255, 255, 255, 100),
                        ),
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
                if let (Some((x0, y0)), Some((x1, y1))) =
                    (self.erase_line_start, self.erase_line_end)
                {
                    let dx = x1 - x0;
                    let dy = y1 - y0;
                    let len = (dx * dx + dy * dy).sqrt();
                    if len > 1.0 {
                        let nx = -dy / len;
                        let ny = dx / len;
                        let half_w = self.erase_line_width * 0.5;
                        let pts = vec![
                            self.image_to_screen(
                                x0 + nx * half_w,
                                y0 + ny * half_w,
                                full_rect,
                                zoom_pan,
                            ),
                            self.image_to_screen(
                                x1 + nx * half_w,
                                y1 + ny * half_w,
                                full_rect,
                                zoom_pan,
                            ),
                            self.image_to_screen(
                                x1 - nx * half_w,
                                y1 - ny * half_w,
                                full_rect,
                                zoom_pan,
                            ),
                            self.image_to_screen(
                                x0 - nx * half_w,
                                y0 - ny * half_w,
                                full_rect,
                                zoom_pan,
                            ),
                        ];
                        ui.painter().add(egui::Shape::convex_polygon(
                            pts,
                            color,
                            egui::Stroke::new(1.0, stroke_color),
                        ));
                        // 中心線も重ねて表示
                        let p0 = self.image_to_screen(x0, y0, full_rect, zoom_pan);
                        let p1 = self.image_to_screen(x1, y1, full_rect, zoom_pan);
                        ui.painter()
                            .line_segment([p0, p1], egui::Stroke::new(1.0, stroke_color));
                    }
                }
            }
            EraseTool::Rect | EraseTool::Ellipse => {
                if let (Some(start), Some(end)) =
                    (self.erase_shape_drag_start, self.erase_shape_drag_end)
                {
                    let s0 = self.image_to_screen(start.0, start.1, full_rect, zoom_pan);
                    let s1 = self.image_to_screen(end.0, end.1, full_rect, zoom_pan);
                    let rect = egui::Rect::from_two_pos(s0, s1);
                    match self.erase_tool {
                        EraseTool::Rect => {
                            ui.painter().rect_stroke(
                                rect,
                                0.0,
                                egui::Stroke::new(2.0, stroke_color),
                                egui::StrokeKind::Inside,
                            );
                        }
                        EraseTool::Ellipse => {
                            // 楕円: 36 角形近似で描画
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
                            ui.painter()
                                .add(egui::Shape::line(pts, egui::Stroke::new(2.0, stroke_color)));
                        }
                        _ => {}
                    }
                }
            }
            _ => {}
        }
    }

    /// 選択ツール中、全ベクタオブジェクトの存在を示す編集用アウトラインを描く。
    ///
    /// `Subtract` Shape は最終マスクでは透明になるため、マスクテクスチャだけでは
    /// クリック対象が見えない。選択中 Shape は直後のハンドル描画に任せる。
    fn draw_shape_outlines(
        &self,
        ui: &mut egui::Ui,
        full_rect: egui::Rect,
        zoom_pan: Option<(f32, egui::Vec2)>,
    ) {
        if self.erase_preview_active
            || self.erase_tool != EraseTool::Select
            || self.erase_shapes.is_empty()
        {
            return;
        }
        let Some((scale, _img_rect)) = self.erase_image_layout(full_rect, zoom_pan) else {
            return;
        };

        let painter = ui.painter().with_clip_rect(full_rect);
        let to_screen = |p: (f32, f32)| self.image_to_screen(p.0, p.1, full_rect, zoom_pan);
        for (idx, shape) in self.erase_shapes.iter().enumerate() {
            if Some(idx) == self.erase_selected_shape {
                continue;
            }
            let layout = vector_edit::compute_handle_layout(shape, scale);
            vector_edit::draw_shape_outline(&painter, &layout, shape.op(), &to_screen);
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
        let (Some(start), Some(end)) = (self.erase_line_start, self.erase_line_end) else {
            return;
        };
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
            ui.painter().rect_stroke(
                rect,
                0.0,
                egui::Stroke::new(1.0, stroke_color),
                egui::StrokeKind::Outside,
            );
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
                pts,
                color,
                egui::Stroke::new(1.0, stroke_color),
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
        if self.erase_tool != EraseTool::Brush {
            return;
        }
        if let Some(pos) = ctx.input(|i| i.pointer.hover_pos()) {
            if full_rect.contains(pos) {
                let Some((total_scale, _)) = self.erase_image_layout(full_rect, zoom_pan) else {
                    return;
                };
                let screen_r = self.erase_brush_radius * total_scale;
                draw_dashed_circle(ui.painter(), pos, screen_r);
            }
        }
    }

    // ── ツールパネル ──────────────────────────────────────────────

    fn draw_erase_panel(&mut self, _ui: &mut egui::Ui, ctx: &egui::Context, full_rect: egui::Rect) {
        // 隠蔽パネルと同じ egui::Area + Frame::popup + panel_toggle_button の構成に
        // 揃える (2026-05-27 統一)。raw painter で固定 y オフセットを積んで描画して
        // いた旧実装と違い、Frame::popup の auto-size + egui レイアウトに任せて
        // 内容のセクションが増減しても自然に縦が伸び縮みする。
        let panel_pos = egui::pos2(
            full_rect.min.x + PANEL_MARGIN_X,
            full_rect.min.y + PANEL_MARGIN_Y,
        );

        // クリック吸収 sink は直近の実パネル矩形に揃える。本文が内容量に応じて
        // 縮むため、下端までの巨大な sink を残すと画像側のクリックを奪ってしまう。
        let sink_rect = self
            .erase_panel_rect(full_rect)
            .expand2(egui::vec2(4.0, 8.0));

        // closure 内で self mutation を避ける用のローカル変数群。Area::show 後に
        // まとめてディスパッチする。
        let mut preview_pressed = false;
        let mut close_clicked = false;
        let mut switch_to: Option<usize> = None;
        let mut mask_delete_clicked = false;

        egui::Area::new(egui::Id::new("erase_tool_panel"))
            .fixed_pos(panel_pos)
            .order(egui::Order::Foreground)
            .interactable(true)
            .show(ctx, |ui| {
                // 1. クリック吸収 sink を widget より先に登録 (= egui の hit test
                //    "同じ rect の Response は後勝ち" ルールで widget の click を
                //    優先しつつ、widget 外の隙間/Frame 外周は sink が拾う)。
                ui.interact(
                    sink_rect,
                    egui::Id::new("erase_panel_click_sink"),
                    egui::Sense::click_and_drag(),
                );
                let frame_response = egui::Frame::popup(ui.style())
                    .fill(egui::Color32::from_rgba_unmultiplied(20, 20, 20, 230))
                    .stroke(egui::Stroke::new(
                        1.0,
                        egui::Color32::from_rgba_unmultiplied(255, 255, 255, 40),
                    ))
                    .corner_radius(6.0)
                    .show(ui, |ui| {
                        // 幅キャップを min/max 両方で固定。ui.set_min_width だけだと
                        // 中の widget (例: 折り返さない help label) が広いと auto-size で
                        // パネル全体が広がり、右側が「スカスカ」に見えるため (実機 FB)。
                        ui.set_min_width(PANEL_W);
                        ui.set_max_width(PANEL_W);
                        // ⚠ 重要: テーマに依存せず常に DARK visuals を使う (R3 FB)。
                        // 詳細は ui_conceal.rs の同様コメント参照。
                        *ui.visuals_mut() = egui::Visuals::dark();
                        ui.visuals_mut().override_text_color = Some(egui::Color32::WHITE);

                        // ── ヘッダ (タイトル + プレビュー + 閉じる × ボタン) ──
                        // R5: ヘッダは **ScrollArea の外** に出す。旧版は ScrollArea
                        // 内に置いていたため、スクロールバーが × ボタンに重なる + ×
                        // が縦スクロールに巻き込まれる現象があった (実機 FB R5)。
                        // ヘッダ固定にすることでスクロールバーも × の右側ではなく
                        // ScrollArea (= 中身) の右端に張り付く。
                        ui.horizontal(|ui| {
                            ui.label(
                                egui::RichText::new("消しゴム")
                                    .size(15.0)
                                    .strong()
                                    .color(egui::Color32::WHITE),
                            );
                            ui.with_layout(
                                egui::Layout::right_to_left(egui::Align::Center),
                                |ui| {
                                    // 閉じる × ボタン (右端)
                                    let (close_rect, close_resp) = ui.allocate_exact_size(
                                        egui::vec2(26.0, 22.0),
                                        egui::Sense::click(),
                                    );
                                    let close_bg = if close_resp.hovered() {
                                        egui::Color32::from_rgba_unmultiplied(
                                            220, 80, 80, 200,
                                        )
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
                                    close_resp.on_hover_text("閉じる + 補完実行 (Esc)");
                                    ui.add_space(2.0);
                                    // プレビュー 目アイコン (= while held)
                                    let (eye_rect, eye_resp) = ui.allocate_exact_size(
                                        egui::vec2(26.0, 22.0),
                                        egui::Sense::click_and_drag(),
                                    );
                                    let eye_bg = if eye_resp.is_pointer_button_down_on() {
                                        egui::Color32::from_rgb(60, 120, 200)
                                    } else if eye_resp.hovered() {
                                        egui::Color32::from_rgba_unmultiplied(
                                            100, 100, 100, 220,
                                        )
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
                                        "押している間: 消しゴム適用後の結果プレビュー (モザイクは除外)",
                                    );
                                },
                            );
                        });
                        ui.separator();

                        // ── 残り (ツール選択 / スライダー / スロット / ヘルプ) を
                        //     ScrollArea で囲む ──
                        // Area + Frame::popup の中では ScrollArea が親の available_rect
                        // に引っ張られて小さくなりやすい。前フレームで測った本文高を
                        // 使って親領域を明示確保し、「必要な高さまで伸びるが、下端を
                        // 超える場合だけスクロール」にする。
                        let body_max_height =
                            (full_rect.max.y - ui.cursor().top() - PANEL_BOTTOM_MARGIN)
                                .max(PANEL_MIN_BODY_H);
                        let measured_body_h = self
                            .erase_panel_body_content_h
                            .filter(|h| h.is_finite() && *h > 0.0)
                            .unwrap_or(PANEL_BODY_FALLBACK_H);
                        let body_height =
                            body_max_height.min((measured_body_h + 8.0).max(PANEL_MIN_BODY_H));
                        ui.allocate_ui_with_layout(
                            egui::vec2(PANEL_W, body_height),
                            egui::Layout::top_down(egui::Align::LEFT),
                            |ui| {
                                ui.set_min_width(PANEL_W);
                                ui.set_max_width(PANEL_W);
                                ui.set_min_height(body_height);
                                let scroll_output = egui::ScrollArea::vertical()
                                    .max_height(body_height)
                                    .auto_shrink([false, false])
                                    .show(ui, |ui| {
                                        ui.set_min_width(PANEL_W);
                                        ui.set_max_width(PANEL_W);

                        // ボタン共通の最小サイズ。PANEL_W (= 190) - Frame::popup
                        // padding (~10*2) - 中央 gap (4) を 2 で割って、片側 ~78px。
                        let btn_w = ((PANEL_W - 20.0 - 4.0) / 2.0).max(60.0);
                        let btn_size = egui::vec2(btn_w, 24.0);

                        // ── 見開きペアの左/右切替 (見開きから入った場合のみ) ──
                        if let Some((left_idx, right_idx)) =
                            self.erase_spread_ctx.map(|c| c.pair)
                        {
                            let pages = [("左ページ", left_idx), ("右ページ", right_idx)];
                            ui.horizontal(|ui| {
                                ui.spacing_mut().item_spacing.x = 4.0;
                                for &(label, target_idx) in pages.iter() {
                                    let is_active = self.fullscreen_idx == Some(target_idx);
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

                        // ── 描画 / 消去 (active=赤/青、inactive=暗灰) ──
                        ui.horizontal(|ui| {
                            ui.spacing_mut().item_spacing.x = 4.0;
                            if panel_toggle_button(
                                ui,
                                "描画 [D]",
                                self.erase_paint_mode,
                                Some(btn_size),
                                Some(PanelToggleColors::paint_red()),
                            )
                            .clicked()
                            {
                                self.erase_paint_mode = true;
                            }
                            if panel_toggle_button(
                                ui,
                                "消去 [F]",
                                !self.erase_paint_mode,
                                Some(btn_size),
                                Some(PanelToggleColors::erase_blue()),
                            )
                            .clicked()
                            {
                                self.erase_paint_mode = false;
                            }
                        });
                        ui.separator();

                        // ── ツール選択 ──
                        // 筆/囲みはビットマップ下地、線/矩形/楕円はその上に作成順で
                        // 重なるオブジェクト。消去モードのオブジェクトは Subtract Shape
                        // として残り、既存オブジェクトを丸ごと削除しない。
                        ui.label(
                            egui::RichText::new("ビットマップ:")
                                .color(egui::Color32::from_gray(200)),
                        );
                        let bitmap_rows: [[(&str, EraseTool); 2]; 1] =
                            [[("筆 [B]", EraseTool::Brush), ("囲み [L]", EraseTool::Lasso)]];
                        for row in bitmap_rows.iter() {
                            ui.horizontal(|ui| {
                                ui.spacing_mut().item_spacing.x = 4.0;
                                for &(label, tool) in row.iter() {
                                    if panel_toggle_button(
                                        ui,
                                        label,
                                        self.erase_tool == tool,
                                        Some(btn_size),
                                        None,
                                    )
                                    .clicked()
                                    {
                                        let toast = format!("[{}]", label);
                                        self.switch_erase_tool(tool, &toast);
                                    }
                                }
                            });
                        }
                        ui.label(
                            egui::RichText::new("オブジェクト:")
                                .color(egui::Color32::from_gray(200)),
                        );
                        let object_rows: [[(&str, EraseTool); 2]; 3] = [
                            [
                                ("選択 [S]", EraseTool::Select),
                                ("直線 [I]", EraseTool::Line),
                            ],
                            [
                                ("縦線 [V]", EraseTool::VertLine),
                                ("横線 [H]", EraseTool::HorizLine),
                            ],
                            [
                                ("矩形 [R]", EraseTool::Rect),
                                ("楕円 [O]", EraseTool::Ellipse),
                            ],
                        ];
                        for row in object_rows.iter() {
                            ui.horizontal(|ui| {
                                ui.spacing_mut().item_spacing.x = 4.0;
                                for &(label, tool) in row.iter() {
                                    if panel_toggle_button(
                                        ui,
                                        label,
                                        self.erase_tool == tool,
                                        Some(btn_size),
                                        None,
                                    )
                                    .clicked()
                                    {
                                        // 直接代入ではなく helper 経由で選択もクリアする
                                        // (= ハンドルが一瞬出る現象を防ぐ、Codex P1 対応)。
                                        let toast = format!("[{}]", label);
                                        self.switch_erase_tool(tool, &toast);
                                    }
                                }
                            });
                        }
                        ui.add(
                            egui::Label::new(
                                egui::RichText::new("オブジェクトは下地の上に作成順で反映")
                                    .size(10.0)
                                    .color(egui::Color32::from_gray(150)),
                            )
                            .wrap(),
                        );

                        // ── ブラシ半径 / 直線太さ スライダー ──
                        if self.erase_tool == EraseTool::Brush {
                            let max_r =
                                self.erase_mask_size[0].max(self.erase_mask_size[1]) as f32
                                    / 20.0;
                            ui.add(
                                egui::Slider::new(&mut self.erase_brush_radius, 1.0..=max_r)
                                    .text("サイズ")
                                    .step_by(1.0),
                            );
                        }
                        if self.erase_tool == EraseTool::Line {
                            let max_w =
                                self.erase_mask_size[0].max(self.erase_mask_size[1]) as f32
                                    / 20.0;
                            ui.add(
                                egui::Slider::new(&mut self.erase_line_width, 1.0..=max_w)
                                    .text("幅")
                                    .step_by(1.0),
                            );
                        }

                        ui.separator();

                        // ── マスクスロット (= 隠蔽パネルと同じ「保存N / 読込N」2x2 grid) ──
                        ui.label(
                            egui::RichText::new("マスクスロット:")
                                .color(egui::Color32::from_gray(200)),
                        );
                        for (row, action_label) in ["保存", "読込"].iter().enumerate() {
                            ui.horizontal(|ui| {
                                ui.spacing_mut().item_spacing.x = 4.0;
                                for slot in 1..=2usize {
                                    let label = format!("{}{}", action_label, slot);
                                    if panel_toggle_button(
                                        ui,
                                        label,
                                        false,
                                        Some(btn_size),
                                        None,
                                    )
                                    .clicked()
                                    {
                                        if row == 0 {
                                            self.save_mask_to_slot(slot);
                                        } else {
                                            self.load_mask_from_slot(slot);
                                        }
                                    }
                                }
                            });
                        }
                        ui.add(
                            egui::Label::new(
                                egui::RichText::new(
                                    "フルスクリーン中 F7/F8 で保存 1/2 を即適用",
                                )
                                .size(10.0)
                                .color(egui::Color32::from_gray(150)),
                            )
                            .wrap(),
                        );

                        ui.separator();

                        // ── マスク全削除 (赤系の destructive ボタン) ──
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
                            mask_delete_clicked = true;
                        }

                        ui.separator();

                        // ── ヘルプテキスト ──
                        // 長い 1 行はパネル幅キャップ (= PANEL_W) で折り返される。
                        // Label に `.wrap()` を付けないと TextWrapMode::Extend で
                        // 1 行が PANEL_W を超えて Frame::popup を広げる原因になる。
                        let help = "E/Esc:補完して終了 (選択中Esc:解除)\n\
                            Space+ドラッグ:一時パン\n\
                            ホイール:筆/直線のサイズ\n\
                            矢印:シフト [/]:回転 (Ctrl:10倍)\n\
                            Ctrl+ドラッグ:筆サイズ / 縦横線調整\n\
                            S:選択/ハンドル微調整\n\
                            Shift/Alt+ハンドル:拘束/中心固定\n\
                            Ctrl+Z:戻す  Del:選択削除";
                                        ui.add(
                                            egui::Label::new(
                                                egui::RichText::new(help)
                                                    .size(10.0)
                                                    .color(egui::Color32::from_gray(190)),
                                            )
                                            .wrap(),
                                        );
                                    }); // ScrollArea::show
                                let content_h = scroll_output.content_size.y;
                                if content_h.is_finite() && content_h > 0.0 {
                                    self.erase_panel_body_content_h = Some(content_h);
                                }
                            },
                        ); // allocate_ui_with_layout
                    }); // Frame::popup .show
                self.erase_panel_last_rect = Some(frame_response.response.rect);
            }); // Area::show

        // ── closure 外でディスパッチ (& mut self の借用衝突を避ける) ──
        // プレビューボタンの **押下 transition** (false → true) を検出して、
        // MI-GAN inpaint を **保存なしで** 投入する。これにより、新規に shape を
        // 描いた直後にプレビュー押下するだけで現在のマスク全体に対する inpaint
        // 結果が見える (= 隠蔽の preview と同じ UX、実機 FB R4)。
        //
        // 注意:
        // - MI-GAN は async (= worker thread)。1-3 秒かかる間は通常レイヤが見える。
        //   完了次第 `erase_preview_cache` が更新され、毎フレ repaint で reflect される
        // - 既に走っている job は `run_inpaint_and_cache` 内で cancel + restart
        // - 連続 press は許容 (= 押すたびに最新マスクで再投入される)
        // - 空マスクなら何も起きない (run_inpaint_for_preview が false を返す)
        let preview_just_pressed = !self.erase_preview_active && preview_pressed;
        self.erase_preview_active = preview_pressed;
        if preview_just_pressed && let Some(fs_idx) = self.fullscreen_idx {
            if self.run_inpaint_for_preview(ctx, fs_idx) {
                self.show_feedback_toast("[プレビュー計算中…]".to_string());
            } else {
                // 入力未読込 / サイズ不一致などで投入できなかった場合は、マスクだけ
                // 消えたように見える preview 状態へ入らない。
                self.erase_preview_active = self.erase_preview_cache.contains_key(&fs_idx);
            }
        }
        if let Some(target) = switch_to {
            self.switch_erase_target_in_spread(ctx, target);
        }
        if mask_delete_clicked {
            let [w, h] = self.erase_mask_size;
            self.push_undo_snapshot();
            self.erase_mask = Some(vec![false; w * h]);
            self.erase_shapes.clear();
            self.erase_selected_shape = None;
            self.erase_drag = None;
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
            self.erase_shape_drag_start = None;
            self.erase_shape_drag_end = None;
            if let Some(fs_idx) = self.fullscreen_idx {
                // Preview cache を完全破棄 (Codex P1 R4 #1: 全削除後に preview が
                // 残らないことを保証する)。
                self.clear_erase_preview(fs_idx);
                // DB + サイドカーからも削除
                self.delete_mask_with_sidecar(fs_idx);
                // fs_cache は raw decode 専用。マスク削除では消しゴム結果レイヤだけを
                // 破棄し、表示は下層 (adjustment > AI > raw) へ自然にフォールバックさせる。
                self.clear_erase_result_caches_for_idx(fs_idx);
                self.clear_conceal_caches(fs_idx);
                self.erase_base_tex_cache.remove(&fs_idx);
            }
        }
        if close_clicked {
            // × ボタンは Esc キーと同じ動作: マスクがあれば inpaint を実行してから
            // モード退出、マスクが空なら何もせず退出 (`execute_erase_inpaint` 内で
            // ハンドリング)。
            if let Some(idx) = self.fullscreen_idx {
                self.execute_erase_inpaint(ctx, idx);
            } else {
                self.reset_erase_mode();
            }
        }

        ctx.request_repaint();
    }

    // ── Inpaint 実行 ──────────────────────────────────────────────

    /// 消しゴム確定結果の入力になる、pre-erase の表示ピクセルを取得する。
    /// 通常は `adjustment_cache > ai_upscale_cache > fs_cache`。透過元画像だけは
    /// 黒固定の作業ベースを守るため `adjustment_cache` を再利用せず、bg=0 の
    /// AI / raw を黒不透明化してから補正を掛け直す。
    fn resolve_erase_input_pixels(&self, fs_idx: usize) -> Option<Arc<egui::ColorImage>> {
        self.resolve_erase_input_pixels_matching(fs_idx, None)
            .map(|(pixels, _)| pixels)
    }

    fn resolve_erase_input_pixels_matching(
        &self,
        fs_idx: usize,
        expected_size: Option<[usize; 2]>,
    ) -> Option<(Arc<egui::ColorImage>, &'static str)> {
        let size_ok =
            |pixels: &Arc<egui::ColorImage>| expected_size.map_or(true, |size| pixels.size == size);
        let force_black = self.fs_static_has_alpha(fs_idx);
        let mut already_adjusted = false;
        let mut source_name = "none";

        let source = if !force_black {
            match self.adjustment_cache.get(&fs_idx) {
                Some(FsCacheEntry::Static { pixels, .. }) if size_ok(pixels) => {
                    already_adjusted = true;
                    source_name = "adjustment";
                    Some(Arc::clone(pixels))
                }
                _ => None,
            }
        } else {
            None
        };

        let source = match source {
            Some(pixels) => Some(pixels),
            None => {
                let bg = self.erase_upscale_bg_mode(fs_idx);
                let ai_matching = self
                    .ai_upscale_cache
                    .get(&(fs_idx, bg))
                    .and_then(|e| match e {
                        FsCacheEntry::Static { pixels, .. } if size_ok(pixels) => {
                            Some(Arc::clone(pixels))
                        }
                        _ => None,
                    });
                if let Some(pixels) = ai_matching {
                    source_name = "ai";
                    Some(pixels)
                } else if let Some(pixels) = self
                    .erase_base_cache
                    .get(&fs_idx)
                    .filter(|pixels| size_ok(pixels))
                    .map(Arc::clone)
                {
                    source_name = "erase_base";
                    Some(pixels)
                } else {
                    match self.fs_cache.get(&fs_idx) {
                        Some(FsCacheEntry::Static { pixels, .. }) if size_ok(pixels) => {
                            source_name = "fs_cache";
                            Some(Arc::clone(pixels))
                        }
                        _ => None,
                    }
                }
            }
        }?;

        let source = self.black_flatten_erase_source_if_needed(fs_idx, source);
        if already_adjusted {
            Some((source, source_name))
        } else {
            Some((
                self.apply_erase_adjustments_to_source(fs_idx, source),
                source_name,
            ))
        }
    }

    fn commit_pending_matches_current_erase_key(&self, fs_idx: usize) -> bool {
        let key = self.current_erase_result_key(fs_idx);
        let pending_key = EraseInpaintPendingKey {
            idx: fs_idx,
            kind: EraseInpaintKind::Commit,
        };
        self.erase_inpaint_pending
            .get(&pending_key)
            .is_some_and(|p| {
                p.input_generation == key.input_gen && p.mask_generation == key.mask_gen
            })
    }

    /// 表示パイプライン用: 保存済み消しゴムマスクがあり、現在世代の確定結果が無ければ
    /// MI-GAN を非同期起動する。結果が既にあれば texture を返す。
    pub(crate) fn ensure_erase_result_texture(
        &mut self,
        ctx: &egui::Context,
        fs_idx: usize,
    ) -> Option<egui::TextureHandle> {
        if self.erase_mode || !self.mask_pages.contains(&fs_idx) {
            return None;
        }
        if let Some(tex) = self.current_erase_result_texture(fs_idx) {
            return Some(tex);
        }
        if self.commit_pending_matches_current_erase_key(fs_idx) {
            return None;
        }

        let source = self.resolve_erase_input_pixels(fs_idx)?;
        let [w, h] = source.size;
        if w == 0 || h == 0 {
            return None;
        }
        let key = self.page_path_key(fs_idx)?;
        let (bitmap, shapes) = match self.mask_db.as_ref().and_then(|db| db.get_full(&key, w, h)) {
            Some(v) => v,
            None => {
                self.mask_pages.remove(&fs_idx);
                return None;
            }
        };
        let mut composite = bitmap;
        crate::mask_db::rasterize_shapes_into(&mut composite, &shapes, w, h);
        if !composite.iter().any(|&m| m) {
            self.mask_pages.remove(&fs_idx);
            return None;
        }
        self.run_inpaint_and_cache(ctx, fs_idx, source, composite, w, h, "ensure-result", false);
        None
    }

    /// マスク保存と MI-GAN inpaint 投入だけ行い、`reset_erase_mode` は呼ばない内部版。
    /// 戻り値: `ApplyInpaintOutcome` で「何もしなかった / 保存はしたが commit 保留 /
    /// commit 投入」を区別する。
    fn apply_inpaint_only(&mut self, ctx: &egui::Context, fs_idx: usize) -> ApplyInpaintOutcome {
        let composite = match self.composite_mask() {
            Some(c) if c.iter().any(|&m| m) => c,
            _ => return ApplyInpaintOutcome::NoMask,
        };
        let Some(bitmap) = self.erase_mask.clone() else {
            return ApplyInpaintOutcome::NoMask;
        };
        let [w, h] = self.erase_mask_size;
        // ビットマップとベクタを別々に永続化することで、再編集時に両者を分離して読み直せる。
        let vectors_snapshot = self.erase_shapes.clone();
        self.save_mask_with_sidecar(fs_idx, &bitmap, &vectors_snapshot, w, h);
        let Some(original) = self.resolve_erase_input_pixels(fs_idx) else {
            return ApplyInpaintOutcome::Deferred;
        };
        if original.size != [w, h] {
            // 消しゴム入場後に AI 完了などで入力解像度が変わった場合は、ここで
            // 無理に古いサイズの source を流さず、保存済みマスクから通常表示側の
            // ensure_erase_result_texture に再生成させる。
            crate::logger::log(format!(
                "erase: commit deferred (size mismatch: source={}x{} mask={}x{})",
                original.size[0], original.size[1], w, h
            ));
            self.clear_erase_result_caches_for_idx(fs_idx);
            return ApplyInpaintOutcome::Deferred;
        }
        let masked_count = composite.iter().filter(|&&m| m).count();
        crate::logger::log(format!(
            "erase: inpaint start, masked pixels={masked_count}"
        ));
        self.run_inpaint_and_cache(ctx, fs_idx, original, composite, w, h, "exec", false);
        ApplyInpaintOutcome::Launched
    }

    /// 指定 idx の preview MI-GAN 状態を破棄する。
    ///
    /// - 進行中の preview ジョブがあれば cancel
    /// - `erase_preview_cache[idx]` を削除
    ///
    /// 呼び出し箇所: mask 変更 (shape commit / brush / lasso) / 全削除 /
    /// `reset_erase_mode`。commit 経路の MI-GAN ジョブ (= is_preview=false) は
    /// 触らない (= 別の処理として独立)。
    pub(crate) fn clear_erase_preview(&mut self, idx: usize) {
        // pending: is_preview=true のときだけ cancel + remove
        let key = EraseInpaintPendingKey {
            idx,
            kind: EraseInpaintKind::Preview,
        };
        if let Some(prev) = self.erase_inpaint_pending.remove(&key) {
            prev.cancel.store(true, Ordering::Relaxed);
        }
        self.erase_preview_cache.remove(&idx);
    }

    /// **保存なし** で MI-GAN inpaint を投入する (プレビュー press 用)。
    ///
    /// `apply_inpaint_only` と違って `save_mask_with_sidecar` を呼ばない。
    /// プレビューは「現在の編集を一時的に見る」用途なので DB / サイドカーを
    /// 触らないほうがユーザーの直感に近い (= ESC で undo 戻し可能)。
    ///
    /// `run_inpaint_and_cache` は同 idx の進行中ジョブを cancel して新規開始
    /// するので、プレビュー押下を繰り返しても安全 (= 連続 press = cancel + restart)。
    ///
    /// 戻り値: ジョブを投入したら `true` (= 空マスクでないとき)。
    pub(crate) fn run_inpaint_for_preview(&mut self, ctx: &egui::Context, fs_idx: usize) -> bool {
        let composite = match self.composite_mask() {
            Some(c) if c.iter().any(|&m| m) => c,
            _ => return false,
        };
        let [w, h] = self.erase_mask_size;
        let Some((adjusted, source_name)) =
            self.resolve_erase_input_pixels_matching(fs_idx, Some([w, h]))
        else {
            crate::logger::log(format!(
                "erase: preview skipped (no matching source for mask={}x{})",
                w, h
            ));
            return false;
        };
        crate::logger::log(format!("erase: preview source={source_name} {w}x{h}"));
        // is_preview = true: 完了時 fs_cache を書き換えず preview_cache に流す。
        self.run_inpaint_and_cache(ctx, fs_idx, adjusted, composite, w, h, "preview", true);
        true
    }

    /// MI-GAN inpaint を実行 ([E] 二回押し / Apply 経路)。
    /// 投入後は `reset_erase_mode` を呼んで消しゴムモード自体を終了する。
    /// 見開きから入っていた場合は reset 内で見開きが復元される。
    pub(crate) fn execute_erase_inpaint(&mut self, ctx: &egui::Context, fs_idx: usize) {
        match self.apply_inpaint_only(ctx, fs_idx) {
            ApplyInpaintOutcome::Launched => {
                self.show_feedback_toast("[補完中…]".to_string());
            }
            ApplyInpaintOutcome::Deferred => {
                self.show_feedback_toast("[補完待機中…]".to_string());
            }
            ApplyInpaintOutcome::NoMask => {}
        }
        self.reset_erase_mode();
    }

    /// 画像ロード完了後に保存済みマスクがあれば自動で inpaint を適用する。
    /// `poll_prefetch` から呼ばれる。
    pub(crate) fn auto_apply_saved_mask(&mut self, ctx: &egui::Context, idx: usize) {
        // erase mode 中は手動操作に任せる
        if self.erase_mode {
            return;
        }

        let key = match self.page_path_key(idx) {
            Some(k) => k,
            None => return,
        };

        // DB にマスクがあるか確認。入力は raw 固定ではなく、現在の pre-erase 表示レイヤ
        // (補正 > AI > raw) を使う。
        let Some(pixels) = self.resolve_erase_input_pixels(idx) else {
            return;
        };
        let [w, h] = pixels.size;

        let (bitmap, shapes) = match self.mask_db.as_ref().and_then(|db| db.get_full(&key, w, h)) {
            Some(m) => m,
            None => return,
        };
        let mut composite = bitmap.clone();
        crate::mask_db::rasterize_shapes_into(&mut composite, &shapes, w, h);
        if !composite.iter().any(|&m| m) {
            // ensure_erase_result_texture と同じく、composite 空のときは mask_pages
            // からも外しておく (badge 一貫性、Phase 1-5 code-review CONFIRMED)。
            self.mask_pages.remove(&idx);
            return;
        }

        crate::logger::log(format!("erase: auto-applying saved mask for idx={idx}"));

        // 元画像を base_cache に保存（サイズが変わった場合は更新）
        let need_update = self
            .erase_base_cache
            .get(&idx)
            .map(|old| old.size != pixels.size)
            .unwrap_or(true);
        if need_update {
            self.erase_base_cache.insert(idx, Arc::clone(&pixels));
        }

        self.run_inpaint_and_cache(ctx, idx, pixels, composite, w, h, "auto-apply", false);
    }

    /// フルスクリーン表示中 (消しゴムモード外) で F7/F8 から呼ばれる。
    /// スロット N のマスクを現ページに**差し替えて**保存し、inpaint を実行する。
    /// 消しゴムモードに入る必要なく 1 キーでマスクを適用できる。
    /// 偶数/奇数ページ取り違えで旧マスクと合成されないよう上書き仕様。
    pub(crate) fn apply_slot_in_viewing_mode(&mut self, ctx: &egui::Context, slot: usize) {
        if self.erase_mode {
            return;
        }
        let Some(fs_idx) = self.fullscreen_idx else {
            return;
        };

        let pixels = match self.resolve_erase_input_pixels(fs_idx) {
            Some(pixels) => pixels,
            None => {
                self.show_feedback_toast("[画像読込中]".to_string());
                return;
            }
        };
        let [w, h] = pixels.size;

        // スロットのマスクとベクタを取得
        let slot_data = self
            .mask_db
            .as_ref()
            .and_then(|db| db.get_slot_full(slot, w, h));
        let Some((new_mask, new_vectors)) = slot_data else {
            self.show_feedback_toast(format!("[スロット{slot}は空です]"));
            return;
        };
        if !new_mask.iter().any(|&m| m) && new_vectors.is_empty() {
            self.show_feedback_toast(format!("[スロット{slot}は空です]"));
            return;
        }

        // 消しゴム編集に入ったときの pre-erase 表示用に base_cache も更新しておく。
        // MI-GAN 入力は現在の pre-erase 表示レイヤ (`pixels`) を直接使う。
        let need_update = self
            .erase_base_cache
            .get(&fs_idx)
            .map(|old| old.size != pixels.size)
            .unwrap_or(true);
        if need_update {
            self.erase_base_cache.insert(fs_idx, Arc::clone(&pixels));
        }

        let mut composite = new_mask.clone();
        crate::mask_db::rasterize_shapes_into(&mut composite, &new_vectors, w, h);
        if !composite.iter().any(|&m| m) {
            return;
        }

        self.save_mask_with_sidecar(fs_idx, &new_mask, &new_vectors, w, h);

        crate::logger::log(format!(
            "erase: apply_slot_in_viewing_mode slot={slot} idx={fs_idx}"
        ));

        self.run_inpaint_and_cache(ctx, fs_idx, pixels, composite, w, h, "viewing-mode", false);
        self.show_feedback_toast(format!("[スロット{slot}適用]"));
    }

    /// MI-GAN (失敗時は拡散フォールバック) で inpaint を走らせ、結果は worker thread から
    /// `App.erase_inpaint_pending` 経由で UI スレッドに届く。完了反映は `poll_erase_inpaint`。
    /// E キーの確定・保存済みマスクの自動適用・F7/F8 の 3 つから呼ばれる。
    /// `log_prefix` はログ行を区別するための識別子。
    /// `is_preview = true` のときは preview 専用ジョブとして登録される
    /// (= 完了時 fs_cache を書き換えず preview_cache を更新、Codex P1 R4 #1)。
    fn run_inpaint_and_cache(
        &mut self,
        ctx: &egui::Context,
        idx: usize,
        original: Arc<egui::ColorImage>,
        composite: Vec<bool>,
        w: usize,
        h: usize,
        log_prefix: &'static str,
        is_preview: bool,
    ) {
        self.ensure_ai_runtime();
        let runtime = self.ai_runtime.clone();
        let manager = self.ai_model_manager.clone();
        let kind = if is_preview {
            EraseInpaintKind::Preview
        } else {
            EraseInpaintKind::Commit
        };
        let pending_key = EraseInpaintPendingKey { idx, kind };

        // 同じ idx + kind に対して進行中のジョブがあれば cancel (= 連打 / 同ページへの再 apply)。
        // preview と commit は別 kind なので、プレビュー押下で確定ジョブを潰さない。
        // 別 idx のジョブはそのまま並走させる (見開き消しゴムで両ページの inpaint を
        // 同時に処理するため)。
        if let Some(prev) = self.erase_inpaint_pending.remove(&pending_key) {
            prev.cancel.store(true, Ordering::Relaxed);
        }

        let cancel = Arc::new(AtomicBool::new(false));
        let cancel_for_thread = Arc::clone(&cancel);
        let (tx, rx) = mpsc::channel::<egui::ColorImage>();
        let ctx_clone = ctx.clone();

        let spawn_result = std::thread::Builder::new()
            .name("erase-inpaint".to_string())
            .spawn(move || {
                let result = run_inpaint_pure(
                    runtime.as_ref(),
                    &manager,
                    &original,
                    &composite,
                    w,
                    h,
                    &cancel_for_thread,
                    log_prefix,
                );
                if cancel_for_thread.load(Ordering::Relaxed) {
                    return;
                }
                let _ = tx.send(result);
                ctx_clone.request_repaint();
            });
        if let Err(e) = spawn_result {
            // spawn 失敗 (handle / アドレス空間枯渇) でプロセスを crash させず graceful に
            // 劣化させる。pending を入れずに抜ける (tx は closure と共に drop 済みなので
            // rx 側 disconnect、ここでは pending 未挿入なので poll もされない)
            // (v1.0.0 安定性レビュー P3-9)。
            crate::logger::log(format!("erase: inpaint worker spawn FAILED: {e}"));
            self.show_feedback_toast("[補完失敗]".to_string());
            return;
        }

        let items_generation = self.items_generation;
        let path_key = self.page_path_key(idx);
        let input_generation = self.input_generation.get(&idx).copied().unwrap_or(0);
        let mask_generation = self.erase_mask_generation.get(&idx).copied().unwrap_or(0);
        self.erase_inpaint_pending.insert(
            pending_key,
            EraseInpaintPending {
                idx,
                items_generation,
                path_key,
                rx,
                cancel,
                started_at: std::time::Instant::now(),
                input_generation,
                mask_generation,
                log_prefix,
                is_preview,
            },
        );
    }

    /// `App::update` 先頭から毎フレーム呼ばれ、worker から結果が届いていれば
    /// fs_cache を差し替える。完了通知は worker 側の `ctx.request_repaint()` に任せる。
    /// idx 別 pending を全部走査し、ready なものから 1 件取り込む (見開き消しゴムで
    /// 複数ページが同時並走するため)。texture アップロードの I/O 集中を避けるため
    /// 1 フレーム最大 1 件、残りは次フレームで処理する。
    pub(crate) fn poll_erase_inpaint(&mut self, ctx: &egui::Context) {
        // 各 pending を try_recv で peek。Ok ならその idx と結果を持って break、
        // Disconnected なら worker が死んでいるので削除して次へ進む。Empty は次へ。
        // (try_recv は &Receiver で借用するだけだが値を返すと所有権が移るので、
        //  Some を返した時点でループを抜けて map から remove する。)
        let mut completed: Option<(EraseInpaintPendingKey, egui::ColorImage)> = None;
        let keys: Vec<EraseInpaintPendingKey> =
            self.erase_inpaint_pending.keys().copied().collect();
        for key in keys {
            let Some(pending) = self.erase_inpaint_pending.get(&key) else {
                continue;
            };
            match pending.rx.try_recv() {
                Ok(result) => {
                    completed = Some((key, result));
                    break;
                }
                Err(mpsc::TryRecvError::Disconnected) => {
                    // ワーカーが panic 等で終了 — pending は破棄。
                    self.erase_inpaint_pending.remove(&key);
                }
                Err(mpsc::TryRecvError::Empty) => {}
            }
        }
        let Some((target_key, result)) = completed else {
            return;
        };
        let pending = self.erase_inpaint_pending.remove(&target_key).unwrap();
        let elapsed = pending.started_at.elapsed();
        let idx = pending.idx;
        // 投入時と比べて items_generation / path_key が変わっていれば結果は捨てる
        // (別 item に着地するのを防ぐ)。サムネイル世代問題と同型の防御。
        if pending.items_generation != self.items_generation
            || pending.path_key != self.page_path_key(idx)
        {
            crate::logger::log(format!(
                "erase: inpaint result discarded (stale: gen {} → {}, prefix={})",
                pending.items_generation, self.items_generation, pending.log_prefix
            ));
            return;
        }
        let current_input_generation = self.input_generation.get(&idx).copied().unwrap_or(0);
        let current_mask_generation = self.erase_mask_generation.get(&idx).copied().unwrap_or(0);
        if pending.input_generation != current_input_generation
            || pending.mask_generation != current_mask_generation
        {
            crate::logger::log(format!(
                "erase: {} inpaint result discarded (stale generation: input {} -> {}, mask {} -> {}, prefix={})",
                if pending.is_preview {
                    "preview"
                } else {
                    "commit"
                },
                pending.input_generation,
                current_input_generation,
                pending.mask_generation,
                current_mask_generation,
                pending.log_prefix
            ));
            return;
        }
        // pixels と texture で同じ ColorImage を共有することで、UI スレッド上の
        // 100MB 級 memcpy (4K 画像 RGBA) を回避する。
        let pixels = Arc::new(result);
        let tex = ctx.load_texture(
            if pending.is_preview {
                format!("fs_preview_inpaint_{idx}")
            } else {
                format!("fs_inpainted_{idx}")
            },
            egui::ImageData::Color(Arc::clone(&pixels)),
            egui::TextureOptions::LINEAR,
        );
        if pending.is_preview {
            // **Preview 経路**: fs_cache を一切触らず、preview_cache だけを更新する
            // (Codex P1 R4 #1)。ESC / 全削除 / mask 変更で preview_cache を捨てれば
            // commit せずに元の状態へ戻れる。
            self.erase_preview_cache.insert(
                idx,
                crate::app::ErasePreviewCacheEntry {
                    pixels,
                    texture: tex,
                },
            );
            crate::logger::log(format!(
                "erase: preview inpaint complete ({} ms, prefix={})",
                elapsed.as_millis(),
                pending.log_prefix
            ));
        } else {
            // **Commit 経路**: fs_cache には戻さず、消しゴム確定結果専用 cache に
            // 格納する。これで AI OFF / 補正変更時に raw fs_cache へ戻れる。
            let key = crate::app::EraseResultKey {
                idx,
                input_gen: pending.input_generation,
                mask_gen: pending.mask_generation,
            };
            self.erase_result_cache.insert(
                key,
                crate::app::EraseResultCacheEntry {
                    pixels,
                    texture: tex,
                },
            );
            self.invalidate_compare_prepared_for_idx(idx);
            self.clear_conceal_caches(idx);
            crate::logger::log(format!(
                "erase: inpaint complete ({} ms, prefix={})",
                elapsed.as_millis(),
                pending.log_prefix
            ));
        }
    }
}

/// worker thread で走る inpaint 本体。`AiRuntime` が利用可能なら MI-GAN、
/// 失敗 / runtime 不在なら拡散 fallback。`&mut self` を取らないことで
/// UI スレッドに戻らずに完結できる。
fn run_inpaint_pure(
    runtime: Option<&Arc<crate::ai::runtime::AiRuntime>>,
    manager: &Arc<crate::ai::model_manager::ModelManager>,
    original: &egui::ColorImage,
    composite: &[bool],
    w: usize,
    h: usize,
    cancel: &Arc<AtomicBool>,
    log_prefix: &str,
) -> egui::ColorImage {
    if let Some(rt) = runtime {
        let kind = crate::ai::ModelKind::InpaintMiGan;
        match manager.model_path(kind) {
            Some(model_path) => {
                if !rt.is_loaded(kind) {
                    if let Err(e) = rt.load_model(kind, &model_path) {
                        crate::logger::log(format!(
                            "[erase] {log_prefix} MI-GAN load failed: {e}, falling back to diffusion"
                        ));
                        return inpaint_diffuse(original, composite, w, h);
                    }
                }
                match inpaint_migan(rt, original, composite, w, h, cancel) {
                    Ok(r) => return r,
                    Err(e) => {
                        crate::logger::log(format!(
                            "[erase] {log_prefix} MI-GAN failed: {e}, falling back to diffusion"
                        ));
                    }
                }
            }
            None => {
                crate::logger::log(format!(
                    "[erase] {log_prefix} MI-GAN model not found, falling back to diffusion"
                ));
            }
        }
    } else {
        crate::logger::log(format!(
            "[erase] {log_prefix} AI runtime not available, falling back to diffusion"
        ));
    }
    inpaint_diffuse(original, composite, w, h)
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
        return Err(crate::ai::AiError::ImageProcessing(
            "No masked pixels".to_string(),
        ));
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
        if !has_mask {
            continue;
        }

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
                    rgb[dst] = ((raw.get(iy * s + ix).copied().unwrap_or(0.0) * 0.5 + 0.5) * 255.0)
                        .clamp(0.0, 255.0);
                    rgb[dst + 1] =
                        ((raw.get(1 * s * s + iy * s + ix).copied().unwrap_or(0.0) * 0.5 + 0.5)
                            * 255.0)
                            .clamp(0.0, 255.0);
                    rgb[dst + 2] =
                        ((raw.get(2 * s * s + iy * s + ix).copied().unwrap_or(0.0) * 0.5 + 0.5)
                            * 255.0)
                            .clamp(0.0, 255.0);
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
                if rx >= region_w || ry >= region_h {
                    continue;
                }

                let gx = region_x0 + rx;
                let gy = region_y0 + ry;
                if gx >= w || gy >= h {
                    continue;
                }

                // マスクされたピクセルのみ inpaint 結果を使用
                if !mask[gy * w + gx] {
                    continue;
                }

                // 辺からの距離ベースの重み
                let dist_left = if is_first_x { ramp } else { tx as f32 };
                let dist_right = if is_last_x {
                    ramp
                } else {
                    (tile.w - 1 - tx) as f32
                };
                let dist_top = if is_first_y { ramp } else { ty as f32 };
                let dist_bot = if is_last_y {
                    ramp
                } else {
                    (tile.h - 1 - ty) as f32
                };
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
            if gx >= w || gy >= h {
                continue;
            }
            let src_idx = gy * w + gx;
            if !mask[src_idx] {
                continue;
            }

            let ri = ry * region_w + rx;
            let wt = accum_w[ri].max(1e-6);
            let r = (accum_r[ri] / wt).clamp(0.0, 255.0) as u8;
            let g = (accum_g[ri] / wt).clamp(0.0, 255.0) as u8;
            let b = (accum_b[ri] / wt).clamp(0.0, 255.0) as u8;
            // 元画素の alpha を保持する。from_rgb は alpha=255 固定なので、透過 PNG の
            // マスク域が不透明化し diffusion fallback (alpha 保持) と非一貫になる
            // (v1.0.0 安定性レビュー P3-8)。MI-GAN は RGB 出力なので alpha は元画像由来。
            let a = original.pixels[src_idx].a();
            pixels[src_idx] = egui::Color32::from_rgba_unmultiplied(r, g, b, a);
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
        if th == 0 {
            break;
        }

        let mut x = 0usize;
        loop {
            let tx = x;
            let tw = tile_size.min(region_w.saturating_sub(tx));
            if tw == 0 {
                break;
            }
            tiles.push(TileRect {
                x: tx,
                y: ty,
                w: tw,
                h: th,
            });

            if tx + tw >= region_w {
                break;
            }
            x += step;
            if x + tile_size > region_w {
                x = region_w.saturating_sub(tile_size);
            }
        }

        if ty + th >= region_h {
            break;
        }
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

fn inpaint_diffuse(
    original: &egui::ColorImage,
    mask: &[bool],
    w: usize,
    h: usize,
) -> egui::ColorImage {
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

    let mut pixels: Vec<[f32; 4]> = original
        .pixels
        .iter()
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
                if filled[idx] {
                    continue;
                }
                let mut sum = [0.0f32; 4];
                let mut count = 0u32;
                for (dx, dy) in &neighbors {
                    let nx = px as isize + dx;
                    let ny = py as isize + dy;
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
                    buf_pixels[idx] = [
                        sum[0] / count as f32,
                        sum[1] / count as f32,
                        sum[2] / count as f32,
                        sum[3] / count as f32,
                    ];
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
        if !any_filled {
            break;
        }
    }
    let rgba: Vec<u8> = pixels
        .iter()
        .flat_map(|p| {
            [
                p[0].round().clamp(0.0, 255.0) as u8,
                p[1].round().clamp(0.0, 255.0) as u8,
                p[2].round().clamp(0.0, 255.0) as u8,
                p[3].round().clamp(0.0, 255.0) as u8,
            ]
        })
        .collect();
    egui::ColorImage::from_rgba_unmultiplied([w, h], &rgba)
}

/// ビットマップマスクを (dx, dy) ピクセル平行移動する (はみ出た部分は false)。
/// 小さなシフトのみを想定 (ゴミ位置補正)。
pub(crate) fn shift_bitmap(mask: &mut [bool], w: usize, h: usize, dx: f32, dy: f32) {
    let shift_x = dx.round() as isize;
    let shift_y = dy.round() as isize;
    if shift_x == 0 && shift_y == 0 {
        return;
    }
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
pub(crate) fn rotate_bitmap(mask: &mut [bool], w: usize, h: usize, cx: f32, cy: f32, angle: f32) {
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

/// 偶奇規則の点多角形判定。
pub(crate) fn point_in_polygon(pt: (f32, f32), poly: &[(f32, f32)]) -> bool {
    let n = poly.len();
    if n < 3 {
        return false;
    }
    let mut inside = false;
    let mut j = n - 1;
    for i in 0..n {
        let (xi, yi) = poly[i];
        let (xj, yj) = poly[j];
        if (yi > pt.1) != (yj > pt.1) {
            let x_intersect = (xj - xi) * (pt.1 - yi) / (yj - yi + 1e-12) + xi;
            if pt.0 < x_intersect {
                inside = !inside;
            }
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
        painter.line_segment(
            [center - egui::vec2(s, 0.0), center + egui::vec2(s, 0.0)],
            outer,
        );
        painter.line_segment(
            [center - egui::vec2(0.0, s), center + egui::vec2(0.0, s)],
            outer,
        );
        painter.line_segment(
            [center - egui::vec2(s, 0.0), center + egui::vec2(s, 0.0)],
            inner,
        );
        painter.line_segment(
            [center - egui::vec2(0.0, s), center + egui::vec2(0.0, s)],
            inner,
        );
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
