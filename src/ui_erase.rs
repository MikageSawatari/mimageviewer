//! 消しゴム (Erase) モード: フルスクリーン画像の任意領域をマスクし、
//! MI-GAN で補完 (inpaint) する。
//!
//! ツール (Phase 2b で 8 種に拡張、隠蔽加工と統一): 選択 (Select) / 筆 (Brush) /
//! 囲み (Lasso) / 多角形 (Polygon) / 直線 (Line) / 縦線 / 横線 / 矩形 (Rect) / 楕円 (Ellipse)
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
use std::collections::VecDeque;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;

use crate::app::{App, EraseSnapshot, EraseTool, MaskDirtyRect};
use crate::displayed_image_transform::DisplayedImageTransform;
use crate::fs_animation::FsCacheEntry;
use crate::keymap::KeyAction;
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

/// worker から UI へ通知する消しゴム補完の進行段階。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum EraseInpaintProgress {
    /// モデルの確認・ロード、段階数の計算中。
    Preparing,
    /// MI-GAN 推論中。`tile_index` は完了済みタイル数なので、開始時は 0。
    Running {
        pass_index: usize,
        pass_count: usize,
        tile_index: usize,
        tile_count: usize,
    },
    /// 現在パスのタイル出力を画像へ合成中。
    Compositing {
        pass_index: usize,
        pass_count: usize,
    },
    /// MI-GAN を利用できない場合の拡散フォールバック中。
    DiffusionFallback,
}

pub(crate) fn report_inpaint_progress(
    progress_tx: Option<&mpsc::Sender<EraseInpaintProgress>>,
    progress: EraseInpaintProgress,
) {
    if let Some(tx) = progress_tx {
        // UI 側がジョブをキャンセルして receiver を破棄した場合は通知不要。
        let _ = tx.send(progress);
    }
}

fn erase_inpaint_progress_label(
    kind: EraseInpaintKind,
    progress: EraseInpaintProgress,
    job_count: usize,
) -> String {
    let preview = kind == EraseInpaintKind::Preview;
    let mut label = match progress {
        EraseInpaintProgress::Preparing if preview => "AI補完プレビューを準備中".to_string(),
        EraseInpaintProgress::Preparing => "AI補完を準備中".to_string(),
        EraseInpaintProgress::Running {
            pass_index,
            pass_count,
            tile_index,
            tile_count,
        } => {
            let operation = if preview {
                "AI補完プレビュー中"
            } else {
                "AI補完中"
            };
            format!(
                "{operation} {}/{}（タイル {}/{}）",
                pass_index.max(1),
                pass_count.max(1),
                tile_index.min(tile_count),
                tile_count.max(1)
            )
        }
        EraseInpaintProgress::Compositing {
            pass_index,
            pass_count,
        } => {
            let operation = if preview {
                "AI補完プレビューを合成中"
            } else {
                "AI補完を合成中"
            };
            format!("{operation} {}/{}", pass_index.max(1), pass_count.max(1))
        }
        EraseInpaintProgress::DiffusionFallback if preview => {
            "補完プレビューの代替処理中".to_string()
        }
        EraseInpaintProgress::DiffusionFallback => "補完の代替処理中".to_string(),
    };
    if job_count > 1 {
        label.push_str(&format!(" / 全{job_count}件"));
    }
    label
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
    /// 既に計算済みのプレビュー結果を、そのまま確定結果へ昇格した。
    CompletedFromPreview,
    /// MI-GAN ジョブを実際にキューへ投入した。
    Launched,
}

/// 進行中の MI-GAN inpaint 推論。`App.erase_inpaint_pending` で保持され、
/// 推論完了 (もしくは新規投入で前ジョブをキャンセル) するまで生存する。
/// 推論本体は worker thread で走り、結果は `rx`、途中経過は `progress_rx` 経由で
/// UI スレッドへ届ける。
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
    /// worker から届くパス / タイル進捗。UI 側で全件 drain し、`progress` に最新値を保持する。
    pub progress_rx: mpsc::Receiver<EraseInpaintProgress>,
    /// 最後に受信した進捗。worker の最初の通知より前も準備中表示を出せるよう初期値を持つ。
    pub progress: EraseInpaintProgress,
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

/// 透明部を黒背景へ合成して全面不透明にする消しゴム入力の共通変換。
pub(crate) fn black_flatten_if_transparent(img: &egui::ColorImage) -> Option<egui::ColorImage> {
    if img.pixels.iter().all(|pixel| pixel.a() == 255) {
        return None;
    }
    let pixels = img
        .pixels
        .iter()
        .map(|pixel| egui::Color32::from_rgba_premultiplied(pixel.r(), pixel.g(), pixel.b(), 255))
        .collect();
    Some(egui::ColorImage::new(img.size, pixels))
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
        black_flatten_if_transparent(img)
    }

    pub(crate) fn enter_erase_mode(&mut self, fs_idx: usize) {
        if !self.fullscreen_edit_mode_entry_allowed(fs_idx) {
            return;
        }
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
        let from_cache = self
            .fs_cache
            .get(&target_idx)
            .and_then(|entry| match entry {
                FsCacheEntry::Static { pixels, .. } => Some(Arc::clone(pixels)),
                _ => None,
            });
        let Some(p) = from_cache else {
            return;
        };
        // 消しゴム入場時の作業ベースは raw 専用の fs_cache から毎回作り直す。
        // erase_base_cache には自動適用/F7/F8 経由で補正済み pixels が入ることがあり、
        // それを再利用すると ensure_erase_base_texture で補正が二重にかかる。
        let pixels = match Self::black_flatten_if_transparent(&p) {
            Some(flat) => Arc::new(flat),
            None => p,
        };
        self.erase_base_cache
            .insert(target_idx, Arc::clone(&pixels));
        // ピクセル取得成功 → ここから state mutation。
        if let Some(pair) = spread_pair {
            self.erase_spread_ctx = Some(crate::app::EraseSpreadCtx {
                saved_mode: self.spread_mode,
                pair,
            });
            self.set_single_page_view(target_idx);
        }
        let fs_idx = target_idx;
        let [w, h] = self.erase_working_size(fs_idx, pixels.size);
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
            if self.effective_params(fs_idx).post_filter != crate::adjustment::PostFilter::None {
                self.clear_adjustment_render_caches_for_bypass(fs_idx);
            }
        }
        self.erase_mask_size = [w, h];
        self.erase_mask_texture = None;
        self.erase_mask_texture_dirty_rect = None;
        self.erase_last_paint_pos = None;

        self.erase_lasso_points.clear();
        self.erase_line_start = None;
        self.erase_line_end = None;
        self.erase_line_tilt = 0.0;
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
            let needs_post_filter_restore = restore_idx
                .map(|idx| {
                    self.effective_params(idx).post_filter != crate::adjustment::PostFilter::None
                })
                .unwrap_or(false);
            self.post_filter_bypassed = false;
            if needs_post_filter_restore && let Some(idx) = restore_idx {
                self.clear_adjustment_render_caches_for_bypass(idx);
            }
        }
        self.erase_mask = None;
        self.erase_mask_size = [0, 0];
        self.erase_mask_texture = None;
        self.erase_mask_texture_dirty_rect = None;
        self.erase_last_paint_pos = None;

        self.erase_lasso_points.clear();
        self.erase_line_start = None;
        self.erase_line_end = None;
        self.erase_line_tilt = 0.0;
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
            self.show_feedback_toast(format!("[消しゴムスロット{}に保存]", slot));
        } else {
            self.show_feedback_toast(format!("[消しゴムスロット{}保存失敗]", slot));
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
            self.show_feedback_toast(format!("[消しゴムスロット{}は空です]", slot));
            return;
        };
        if !slot_mask.iter().any(|&m| m) && slot_vectors.is_empty() {
            self.show_feedback_toast(format!("[消しゴムスロット{}は空です]", slot));
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
        self.show_feedback_toast(format!("[消しゴムスロット{}をロード]", slot));
    }

    // ── キー入力 ──────────────────────────────────────────────────

    /// 消しゴムモード中のキー入力を処理する。
    /// 通常のフルスクリーンショートカットをブロックし、消しゴム専用キーのみ有効にする。
    pub(crate) fn handle_erase_keys(&mut self, ctx: &egui::Context, fs_idx: usize) -> FsKeyAction {
        let action = FsKeyAction {
            close: false,
            close_to_page_list: false,
            page_nav: crate::ui_fullscreen::FsPageNav::None,
            ctrl_nav: None,
            sibling_nav: None,
            mouse_nav: None,
            jump_to: None,
        };

        if !self.ime_input_active(ctx) && self.consume_context_shortcuts_help_key(ctx) {
            self.show_context_shortcuts_help = true;
            return action;
        }

        // ESC: 選択があればまず解除、無ければマスクを適用 (E と同じ挙動) して終了
        //
        // 旧版は ESC でマスクを DB に保存するだけで inpaint を実行しなかったため、
        // 画像には反映されていないのに次回開くとマスクは残っている、という分かりにくい
        // 状態になっていた。明示破棄したい場合はマスク自体を削除してから抜ける。
        let esc = ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Escape));
        if esc {
            if self.erase_tool == EraseTool::Polygon && !self.erase_lasso_points.is_empty() {
                self.erase_lasso_points.clear();
                self.show_feedback_toast("[多角形をキャンセル]".to_string());
                return action;
            }
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
        let key_e = self.keymap.consume_action(ctx, KeyAction::EraseConfirm);
        if key_e {
            self.execute_erase_inpaint(ctx, fs_idx);
            return action;
        }

        // 既定 Enter: 多角形ツールの頂点列を確定。
        let confirm_polygon = self
            .keymap
            .consume_action(ctx, KeyAction::EraseConfirmPolygon);
        if confirm_polygon
            && self.erase_tool == EraseTool::Polygon
            && let Some(pts) =
                crate::manual_mask_tools::take_completed_polygon(&mut self.erase_lasso_points)
        {
            self.push_undo_snapshot();
            self.paint_polygon(&pts, self.erase_paint_mode);
            self.show_feedback_toast("[多角形を確定]".to_string());
            return action;
        }

        // Ctrl+Z: Undo
        let ctrl_z = self.keymap.consume_action(ctx, KeyAction::EraseUndo);
        if ctrl_z {
            if self.erase_tool == EraseTool::Polygon && self.erase_lasso_points.pop().is_some() {
                self.show_feedback_toast("[頂点を戻す]".to_string());
                return action;
            }
            if self.undo_erase() {
                self.show_feedback_toast("[元に戻す]".to_string());
            } else {
                self.show_feedback_toast("[履歴なし]".to_string());
            }
        }

        // Delete: 選択中のベクタオブジェクトを削除
        let key_del = self.keymap.consume_action(ctx, KeyAction::EraseDeleteShape);
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
        // フルスクリーンビューポートでは egui の modifier 状態が stale になり得るため、
        // 補正レイヤーと同じく Windows では OS の実キー状態を正にする。
        #[cfg(windows)]
        let ctrl_held = crate::keyboard_input::focused_key_state_permit(ctx)
            .is_some_and(crate::ui_fullscreen::ctrl_held_via_os);
        #[cfg(not(windows))]
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

        // S/B/K/L/P/V/H/I/R/O: ツール切替
        let key_s_tool = self.keymap.consume_action(ctx, KeyAction::EraseToolSelect);
        let key_b = self.keymap.consume_action(ctx, KeyAction::EraseToolBrush);
        let key_k = self.keymap.consume_action(ctx, KeyAction::EraseToolBucket);
        let key_l = self.keymap.consume_action(ctx, KeyAction::EraseToolLasso);
        let key_p = self.keymap.consume_action(ctx, KeyAction::EraseToolPolygon);
        let key_v = self.keymap.consume_action(ctx, KeyAction::EraseToolVLine);
        let key_h = self.keymap.consume_action(ctx, KeyAction::EraseToolHLine);
        let key_i = self.keymap.consume_action(ctx, KeyAction::EraseToolLine);
        let key_r_tool = self.keymap.consume_action(ctx, KeyAction::EraseToolRect);
        let key_o_tool = self.keymap.consume_action(ctx, KeyAction::EraseToolEllipse);
        if key_s_tool {
            let toast = self.erase_tool_toast("選択", EraseTool::Select);
            self.switch_erase_tool(EraseTool::Select, &toast);
        }
        if key_b {
            let toast = self.erase_tool_toast("筆", EraseTool::Brush);
            self.switch_erase_tool(EraseTool::Brush, &toast);
        }
        if key_k {
            let toast = self.erase_tool_toast("バケツ", EraseTool::Bucket);
            self.switch_erase_tool(EraseTool::Bucket, &toast);
        }
        if key_l {
            let toast = self.erase_tool_toast("囲み", EraseTool::Lasso);
            self.switch_erase_tool(EraseTool::Lasso, &toast);
        }
        if key_p {
            let toast = self.erase_tool_toast("多角形", EraseTool::Polygon);
            self.switch_erase_tool(EraseTool::Polygon, &toast);
        }
        if key_v {
            let toast = self.erase_tool_toast("縦線", EraseTool::VertLine);
            self.switch_erase_tool(EraseTool::VertLine, &toast);
        }
        if key_h {
            let toast = self.erase_tool_toast("横線", EraseTool::HorizLine);
            self.switch_erase_tool(EraseTool::HorizLine, &toast);
        }
        if key_i {
            let toast = self.erase_tool_toast("直線", EraseTool::Line);
            self.switch_erase_tool(EraseTool::Line, &toast);
        }
        if key_r_tool {
            let toast = self.erase_tool_toast("矩形", EraseTool::Rect);
            self.switch_erase_tool(EraseTool::Rect, &toast);
        }
        if key_o_tool {
            let toast = self.erase_tool_toast("楕円", EraseTool::Ellipse);
            self.switch_erase_tool(EraseTool::Ellipse, &toast);
        }

        // D: 描画モード, F: 消去モード
        let key_d = self.keymap.consume_action(ctx, KeyAction::ErasePaintMode);
        let key_f = self.keymap.consume_action(ctx, KeyAction::EraseEraseMode);
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
        // ツール切替に使うキーは SINGLE_KEYS から除外。
        const SINGLE_KEYS: &[egui::Key] = &[
            egui::Key::Space,
            egui::Key::Tab,
            egui::Key::Z,
            egui::Key::G,
            egui::Key::M,
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
    fn erase_image_layout(&self, transform: &DisplayedImageTransform) -> Option<(f32, egui::Rect)> {
        let [iw, ih] = self.erase_mask_size;
        if iw == 0 || ih == 0 {
            return None;
        }
        Some((
            transform.screen_px_per_source_px(egui::vec2(iw as f32, ih as f32)),
            transform.full_image_rect,
        ))
    }

    /// スクリーン座標を画像ピクセル座標 (f32) に変換する。画像外座標も返す。
    fn screen_to_image_f32_unclamped(
        &self,
        screen_pos: egui::Pos2,
        transform: &DisplayedImageTransform,
    ) -> Option<(f32, f32)> {
        let [iw, ih] = self.erase_mask_size;
        (iw > 0 && ih > 0).then(|| {
            let p = transform.screen_to_source_normalized(screen_pos);
            (p.x * iw as f32, p.y * ih as f32)
        })
    }

    /// 画像ピクセル座標をスクリーン座標に変換する。
    fn image_to_screen(
        &self,
        img_x: f32,
        img_y: f32,
        transform: &DisplayedImageTransform,
    ) -> egui::Pos2 {
        let [iw, ih] = self.erase_mask_size;
        transform.source_normalized_to_screen(egui::pos2(
            img_x / iw.max(1) as f32,
            img_y / ih.max(1) as f32,
        ))
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
        let dirty = crate::mask_db::brush_line_bbox(w, h, from, to, radius)
            .and_then(|(x0, y0, x1, y1)| MaskDirtyRect::new(x0, y0, x1, y1));

        if crate::mask_db::paint_brush_line_bitmap(mask, w, h, from, to, radius, paint) {
            self.mark_erase_mask_texture_dirty(dirty);
            // mask 変化 → preview cache を破棄。
            if let Some(fs_idx) = self.fullscreen_idx {
                self.clear_erase_preview(fs_idx);
            }
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
        self.erase_mask_texture_dirty_rect = None;
        // mask 変化 → preview cache を破棄。
        if let Some(fs_idx) = self.fullscreen_idx {
            self.clear_erase_preview(fs_idx);
        }
    }

    fn apply_erase_bitmap_morph_1px(&mut self, dilate: bool) {
        let [w, h] = self.erase_mask_size;
        let Some(mask) = self.erase_mask.as_ref() else {
            return;
        };
        let next = crate::mask_db::morph_bitmap_mask_1px(mask, w, h, dilate);
        if mask.as_slice() == next.as_slice() {
            return;
        }

        self.push_undo_snapshot();
        self.erase_mask = Some(next);
        self.mark_erase_mask_texture_dirty(None);
        if let Some(fs_idx) = self.fullscreen_idx {
            self.clear_erase_preview(fs_idx);
        }
        self.show_feedback_toast(
            if dilate {
                "手動マスクを1px拡張しました。"
            } else {
                "手動マスクを1px縮小しました。"
            }
            .to_string(),
        );
    }

    fn apply_erase_bucket(&mut self, fs_idx: usize, seed_x: usize, seed_y: usize) {
        let Some(source) = self.erase_base_cache.get(&fs_idx).cloned() else {
            self.show_feedback_toast(
                "バケツに使う画像を準備中です。少し待ってからもう一度クリックしてください。"
                    .to_string(),
            );
            return;
        };
        let [w, h] = self.erase_mask_size;
        if source.size != [w, h] {
            self.show_feedback_toast(
                "バケツに使う画像の準備が完了していません。もう一度クリックしてください。"
                    .to_string(),
            );
            return;
        }
        let Some(mask) = self.erase_mask.as_ref() else {
            self.show_feedback_toast(
                "バケツに使うマスクを準備中です。少し待ってからもう一度クリックしてください。"
                    .to_string(),
            );
            return;
        };

        let mut next = mask.clone();
        let outcome = crate::mask_db::flood_fill_bitmap_mask(
            &mut next,
            &source,
            seed_x,
            seed_y,
            crate::mask_db::BucketFill {
                tolerance: self.erase_bucket_tolerance.round().clamp(0.0, 255.0) as u8,
                connected: self.erase_bucket_connected,
                value: self.erase_paint_mode,
                leak_stop: self.erase_bucket_leak_stop.max(0.0),
            },
        );
        match outcome {
            crate::mask_db::BucketFillOutcome::Filled => {}
            crate::mask_db::BucketFillOutcome::SeedTooThin => {
                self.show_feedback_toast(
                    "漏れ止めより細い場所です。漏れ止めを小さくしてください。".to_string(),
                );
                return;
            }
            crate::mask_db::BucketFillOutcome::NoChange
            | crate::mask_db::BucketFillOutcome::Invalid => return,
        }

        self.push_undo_snapshot();
        self.erase_mask = Some(next);
        self.mark_erase_mask_texture_dirty(None);
        self.clear_erase_preview(fs_idx);
    }

    // ── マスクテクスチャ ──────────────────────────────────────────

    fn mark_erase_mask_texture_dirty(&mut self, dirty: Option<MaskDirtyRect>) {
        match (self.erase_mask_texture.is_some(), dirty) {
            (true, Some(rect)) => {
                self.erase_mask_texture_dirty_rect = Some(
                    self.erase_mask_texture_dirty_rect
                        .map_or(rect, |prev| prev.union(rect)),
                );
            }
            _ => {
                self.erase_mask_texture = None;
                self.erase_mask_texture_dirty_rect = None;
            }
        }
    }

    fn erase_mask_region_image(&self, rect: MaskDirtyRect) -> Option<egui::ColorImage> {
        let mask = self.erase_mask.as_ref()?;
        let [w, h] = self.erase_mask_size;
        let composite = crate::mask_db::composite_mask_region(
            mask,
            &self.erase_shapes,
            w,
            h,
            (rect.x0, rect.y0, rect.x1, rect.y1),
        )?;
        let [rw, rh] = rect.size();
        let mut rgba = vec![0u8; rw * rh * 4];
        for (i, masked) in composite.iter().copied().enumerate() {
            if masked {
                rgba[i * 4] = 255;
                rgba[i * 4 + 1] = 60;
                rgba[i * 4 + 2] = 60;
                rgba[i * 4 + 3] = 140;
            }
        }
        Some(egui::ColorImage::from_rgba_unmultiplied([rw, rh], &rgba))
    }

    fn ensure_mask_texture(&mut self, ctx: &egui::Context) {
        let [w, h] = self.erase_mask_size;
        if self
            .erase_mask_texture
            .as_ref()
            .is_some_and(|tex| tex.size() != [w, h])
        {
            self.erase_mask_texture = None;
            self.erase_mask_texture_dirty_rect = None;
        }
        if let Some(rect) = self.erase_mask_texture_dirty_rect.take() {
            if self.erase_mask_texture.is_some() {
                if let Some(ci) = self.erase_mask_region_image(rect)
                    && let Some(tex) = self.erase_mask_texture.as_mut()
                {
                    tex.set_partial([rect.x0, rect.y0], ci, egui::TextureOptions::NEAREST);
                    return;
                }
            }
            self.erase_mask_texture = None;
            self.erase_mask_texture_dirty_rect = None;
        }
        if self.erase_mask_texture.is_some() {
            return;
        }
        self.erase_mask_texture_dirty_rect = None;
        let Some(composite) = self.composite_mask() else {
            return;
        };
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
        self.erase_last_paint_pos = None;
        self.erase_lasso_points.clear();
        self.erase_line_start = None;
        self.erase_line_end = None;
        self.erase_shape_drag_start = None;
        self.erase_shape_drag_end = None;
        self.erase_mask_texture = None;
        // ツールごとに slider 行の有無が変わるためパネル本文高も変わる。前ツール
        // の measured 値を残すと 1 frame だけ slider が clip されたり余白が出る
        // (Phase 1-5 code-review CONFIRMED)。
        self.erase_panel_body_content_h = None;
        self.show_feedback_toast(toast.to_string());
    }

    fn erase_tool_key_action(tool: EraseTool) -> KeyAction {
        match tool {
            EraseTool::Select => KeyAction::EraseToolSelect,
            EraseTool::Brush => KeyAction::EraseToolBrush,
            EraseTool::Bucket => KeyAction::EraseToolBucket,
            EraseTool::Lasso => KeyAction::EraseToolLasso,
            EraseTool::Polygon => KeyAction::EraseToolPolygon,
            EraseTool::VertLine => KeyAction::EraseToolVLine,
            EraseTool::HorizLine => KeyAction::EraseToolHLine,
            EraseTool::Line => KeyAction::EraseToolLine,
            EraseTool::Rect => KeyAction::EraseToolRect,
            EraseTool::Ellipse => KeyAction::EraseToolEllipse,
        }
    }

    fn erase_tool_label(&self, label: &str, tool: EraseTool) -> String {
        self.keymap
            .compact_action_label(label, Self::erase_tool_key_action(tool))
    }

    fn erase_tool_toast(&self, label: &str, tool: EraseTool) -> String {
        format!("[{}]", self.erase_tool_label(label, tool))
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
        transform: &DisplayedImageTransform,
        modifiers: egui::Modifiers,
    ) -> bool {
        // ① 進行中のドラッグがあれば最優先で処理
        if let Some(drag) = self.erase_drag {
            let img_pos_opt =
                pointer_pos.and_then(|p| self.screen_to_image_f32_unclamped(p, transform));
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
        let Some((scale, _img_rect)) = self.erase_image_layout(transform) else {
            return false;
        };
        let Some(img_pos) = self.screen_to_image_f32_unclamped(screen, transform) else {
            return false;
        };
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
        transform: &DisplayedImageTransform,
        modifiers: egui::Modifiers,
    ) {
        let Some(screen) = pointer_pos else {
            return;
        };
        let Some((_total_scale, _img_rect)) = self.erase_image_layout(transform) else {
            return;
        };
        let Some(cur) = self.screen_to_image_f32_unclamped(screen, transform) else {
            return;
        };

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
    pub(crate) fn erase_panel_rect(&self, full_rect: egui::Rect) -> egui::Rect {
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
        transform: &DisplayedImageTransform,
    ) {
        let full_rect = transform.viewport_rect;
        // フォーカス復帰クリック中は塗り・選択操作を一切発生させない
        // (handle_fs_wheel_and_click で検出・セットされる)
        if self.fs_primary_suppression.is_active() {
            return;
        }
        let primary_down = ctx.input(|i| i.pointer.primary_down());
        let primary_pressed = ctx.input(|i| i.pointer.primary_pressed());
        let primary_released = ctx.input(|i| i.pointer.primary_released());
        let secondary_pressed = ctx.input(|i| i.pointer.secondary_pressed())
            && self
                .settings
                .ring_shortcuts
                .right_drag_mode(crate::ring_shortcut::RightDragContext::EditMode)
                != crate::ring_shortcut::RightDragMode::MouseGesture;
        let pointer_pos = ctx.input(|i| i.pointer.hover_pos());
        let paint = self.erase_paint_mode;
        // KeyHold は keymap 経由。Windows では内部で OS 状態も読むので、
        // FS ビューポートの stale key_down 問題を避けられる。
        let space_held =
            crate::keyboard_input::focused_key_state_permit(ctx).is_some_and(|permit| {
                self.keymap
                    .key_held_action(ctx, permit, KeyAction::EraseSpacePan)
            });

        // パネル上のクリックはツール操作に使わない。
        //
        // ⚠ ただし `primary_released` のフレームだけは通す。これがないと、
        // canvas でハンドル/線/シェイプドラッグ中にパネル上で離した場合、
        // primary_released がツールハンドラに届かず `erase_drag` / `erase_line_*` /
        // `erase_shape_drag_*` などの中間状態が残ったままになる
        // (Codex P2 R3 #2、隠蔽側 `ui_conceal.rs::handle_conceal_paint` の同条件
        // と揃える)。
        let panel_rect = self.erase_panel_rect(full_rect);
        // 描画ドラッグ進行中は Space を無視し、現在の描画を最後まで完結させる。
        // (途中で Space 検知 → パンに切替するとマスクが中途半端に確定するため)
        let drawing_in_progress = self.erase_last_paint_pos.is_some()
            || self.erase_line_start.is_some()
            || self.erase_shape_drag_start.is_some()
            || self.erase_drag.is_some()
            || !self.erase_lasso_points.is_empty();
        let pointer_over_panel = pointer_pos.is_some_and(|pos| panel_rect.contains(pos));
        if !drawing_in_progress
            && self.handle_overlay_space_pan_drag(
                ctx,
                space_held,
                !pointer_over_panel,
                primary_pressed,
                primary_down,
                primary_released,
                pointer_pos,
            )
        {
            return;
        }

        if pointer_over_panel && !primary_released {
            return;
        }

        // ── ベクタオブジェクト編集パス (選択ツール時のみ) ───────────
        // 選択ツール中はドロー系の操作を行わず、クリック=選択/ハンドルドラッグ=編集
        // に徹する。Phase 2b で vector_edit に統一: 角・辺中点・回転ハンドル + 端点。
        // Shift = 軸拘束/等比/15°snap、Alt = 中心固定。
        let modifiers = ctx.input(|i| i.modifiers);
        if self.erase_tool == EraseTool::Select {
            if primary_pressed {
                if let Some((scale, _img_rect)) = self.erase_image_layout(transform) {
                    if let Some(screen) = pointer_pos {
                        let Some(img_pos) = self.screen_to_image_f32_unclamped(screen, transform)
                        else {
                            return;
                        };
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
                self.update_erase_drag(pointer_pos, transform, modifiers);
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

        // バケツは明示クリック 1 回だけを処理する。共通ハンドル処理より先に分岐し、
        // 選択中のベクタ図形を障壁にも編集対象にもしない。
        if self.erase_tool == EraseTool::Bucket {
            if primary_pressed
                && let Some(pos) = pointer_pos
                && transform.contains_screen(pos)
                && let Some(img_pos) = self.screen_to_image_f32_unclamped(pos, transform)
            {
                if let Some(fs_idx) = self.fullscreen_idx {
                    self.apply_erase_bucket(
                        fs_idx,
                        img_pos.0.floor() as usize,
                        img_pos.1.floor() as usize,
                    );
                } else {
                    self.show_feedback_toast(
                        "バケツの対象画像を確認できません。画像を開き直してからもう一度クリックしてください。"
                            .to_string(),
                    );
                }
            }
            return;
        }

        // ── 共通ハンドル処理 (ツール非依存): 直近 shape のハンドルが操作中なら
        //    そちらを優先処理して、新規 shape 作成側に流さない。
        let polygon_in_progress =
            self.erase_tool == EraseTool::Polygon && !self.erase_lasso_points.is_empty();
        if !polygon_in_progress
            && self.try_handle_active_erase_drag_or_handle_hit(
                primary_pressed,
                primary_released,
                pointer_pos,
                transform,
                modifiers,
            )
        {
            return;
        }

        match self.erase_tool {
            EraseTool::Select => {
                // Select は上で処理済み。到達しないはず。
            }
            EraseTool::Brush => {
                if primary_down {
                    if let Some(pos) = pointer_pos {
                        if let Some(img_pos) = self.screen_to_image_f32_unclamped(pos, transform) {
                            if self.erase_last_paint_pos.is_none() {
                                self.push_undo_snapshot();
                            }
                            let prev = self
                                .erase_last_paint_pos
                                .and_then(|p| self.screen_to_image_f32_unclamped(p, transform))
                                .unwrap_or(img_pos);
                            self.paint_brush_line(prev, img_pos, paint);
                        }
                        self.erase_last_paint_pos = Some(pos);
                    }
                } else {
                    self.erase_last_paint_pos = None;
                }
            }
            EraseTool::Bucket => {}
            EraseTool::Lasso => {
                if primary_down {
                    if let Some(pos) = pointer_pos {
                        if let Some(img_pos) = self.screen_to_image_f32_unclamped(pos, transform) {
                            // サンプリング間引き
                            crate::manual_mask_tools::push_freehand_point(
                                &mut self.erase_lasso_points,
                                img_pos,
                            );
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
            EraseTool::Polygon => {
                if secondary_pressed {
                    if let Some(pts) = crate::manual_mask_tools::take_completed_polygon(
                        &mut self.erase_lasso_points,
                    ) {
                        self.push_undo_snapshot();
                        self.paint_polygon(&pts, paint);
                        self.show_feedback_toast("[多角形を確定]".to_string());
                    }
                } else if primary_pressed
                    && let Some(pos) = pointer_pos
                    && let Some((scale, _img_rect)) = self.erase_image_layout(transform)
                {
                    let Some(img_pos) = self.screen_to_image_f32_unclamped(pos, transform) else {
                        return;
                    };
                    if crate::manual_mask_tools::should_close_polygon(
                        &self.erase_lasso_points,
                        img_pos,
                        scale,
                    ) {
                        if let Some(pts) = crate::manual_mask_tools::take_completed_polygon(
                            &mut self.erase_lasso_points,
                        ) {
                            self.push_undo_snapshot();
                            self.paint_polygon(&pts, paint);
                            self.show_feedback_toast("[多角形を確定]".to_string());
                        }
                    } else {
                        crate::manual_mask_tools::push_polygon_vertex(
                            &mut self.erase_lasso_points,
                            img_pos,
                            scale,
                        );
                    }
                }
            }
            EraseTool::VertLine => {
                self.handle_line_tool_paint(
                    primary_down,
                    primary_released,
                    pointer_pos,
                    paint,
                    transform,
                    true,
                );
            }
            EraseTool::HorizLine => {
                self.handle_line_tool_paint(
                    primary_down,
                    primary_released,
                    pointer_pos,
                    paint,
                    transform,
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
                        if let Some(img_pos) = self.screen_to_image_f32_unclamped(pos, transform) {
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
                        if let Some(img_pos) = self.screen_to_image_f32_unclamped(pos, transform) {
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
    fn handle_line_tool_paint(
        &mut self,
        primary_down: bool,
        primary_released: bool,
        pointer_pos: Option<egui::Pos2>,
        paint: bool,
        transform: &DisplayedImageTransform,
        is_vertical: bool,
    ) {
        if primary_down {
            if let Some(pos) = pointer_pos {
                if let Some(img_pos) = self.screen_to_image_f32_unclamped(pos, transform) {
                    if self.erase_line_start.is_none() {
                        self.erase_line_start = Some(img_pos);
                        self.erase_line_tilt = 0.0;
                    }
                    self.erase_line_end = Some(img_pos);
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
        transform: &DisplayedImageTransform,
    ) {
        let full_rect = transform.viewport_rect;
        // マスクオーバーレイ描画。プレビュー押下中は inpaint 反映後の結果を見せたいので
        // マスク表示はオフにする (= ユーザー要望: プレビュー中はマスク非表示)。
        if !self.erase_preview_active {
            self.ensure_mask_texture(ctx);
            if let Some(ref tex) = self.erase_mask_texture {
                let Some((_total_scale, _img_rect)) = self.erase_image_layout(transform) else {
                    return;
                };
                let painter = ui.painter().with_clip_rect(full_rect);
                transform.paint_texture(&painter, tex.id(), egui::Color32::WHITE);
            }
        }

        // ドラッグ中のプレビュー
        self.draw_tool_preview(ui, transform);

        // カーソル
        ctx.output_mut(|o| o.cursor_icon = egui::CursorIcon::Crosshair);
        self.draw_brush_cursor(ui, ctx, transform);

        // ツールパネル
        self.draw_erase_panel(ui, ctx, full_rect);
    }

    /// ドラッグ中のプレビュー表示。
    fn draw_tool_preview(&self, ui: &mut egui::Ui, transform: &DisplayedImageTransform) {
        let full_rect = transform.viewport_rect;
        self.draw_shape_outlines(ui, transform);

        // 選択中の Shape のハンドル (Phase 2b: vector_edit::draw_handles に委譲)
        if let Some(idx) = self.erase_selected_shape {
            if let Some(shape) = self.erase_shapes.get(idx) {
                if let Some((scale, _img_rect)) = self.erase_image_layout(transform) {
                    let layout = vector_edit::compute_handle_layout(shape, scale);
                    let painter = ui.painter().with_clip_rect(full_rect);
                    let hovered = ui.ctx().input(|i| i.pointer.hover_pos()).and_then(|p| {
                        let img_pos = self.screen_to_image_f32_unclamped(p, transform)?;
                        vector_edit::hit_test(&layout, img_pos, scale)
                    });
                    let to_screen = |p: (f32, f32)| self.image_to_screen(p.0, p.1, transform);
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
            tool @ (EraseTool::Lasso | EraseTool::Polygon)
                if !self.erase_lasso_points.is_empty() =>
            {
                let pts: Vec<egui::Pos2> = self
                    .erase_lasso_points
                    .iter()
                    .map(|&(x, y)| self.image_to_screen(x, y, transform))
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
                if tool == EraseTool::Polygon {
                    for (idx, &p) in pts.iter().enumerate() {
                        let fill = if idx == 0 {
                            egui::Color32::from_rgb(255, 245, 120)
                        } else {
                            stroke_color
                        };
                        ui.painter().circle_filled(p, 4.0, fill);
                        ui.painter().circle_stroke(
                            p,
                            4.0,
                            egui::Stroke::new(1.0, egui::Color32::BLACK),
                        );
                    }
                }
            }
            EraseTool::VertLine => {
                self.draw_line_tool_preview(ui, transform, color, stroke_color, true);
            }
            EraseTool::HorizLine => {
                self.draw_line_tool_preview(ui, transform, color, stroke_color, false);
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
                            self.image_to_screen(x0 + nx * half_w, y0 + ny * half_w, transform),
                            self.image_to_screen(x1 + nx * half_w, y1 + ny * half_w, transform),
                            self.image_to_screen(x1 - nx * half_w, y1 - ny * half_w, transform),
                            self.image_to_screen(x0 - nx * half_w, y0 - ny * half_w, transform),
                        ];
                        ui.painter().add(egui::Shape::convex_polygon(
                            pts,
                            color,
                            egui::Stroke::new(1.0, stroke_color),
                        ));
                        // 中心線も重ねて表示
                        let p0 = self.image_to_screen(x0, y0, transform);
                        let p1 = self.image_to_screen(x1, y1, transform);
                        ui.painter()
                            .line_segment([p0, p1], egui::Stroke::new(1.0, stroke_color));
                    }
                }
            }
            EraseTool::Rect | EraseTool::Ellipse => {
                if let (Some(start), Some(end)) =
                    (self.erase_shape_drag_start, self.erase_shape_drag_end)
                {
                    let source_center = ((start.0 + end.0) * 0.5, (start.1 + end.1) * 0.5);
                    let source_radius =
                        ((end.0 - start.0).abs() * 0.5, (end.1 - start.1).abs() * 0.5);
                    match self.erase_tool {
                        EraseTool::Rect => {
                            let pts = vec![
                                self.image_to_screen(start.0, start.1, transform),
                                self.image_to_screen(end.0, start.1, transform),
                                self.image_to_screen(end.0, end.1, transform),
                                self.image_to_screen(start.0, end.1, transform),
                                self.image_to_screen(start.0, start.1, transform),
                            ];
                            ui.painter()
                                .add(egui::Shape::line(pts, egui::Stroke::new(2.0, stroke_color)));
                        }
                        EraseTool::Ellipse => {
                            // 楕円: 36 角形近似で描画
                            const N: usize = 36;
                            let mut pts = Vec::with_capacity(N + 1);
                            for i in 0..=N {
                                let theta = i as f32 * std::f32::consts::TAU / N as f32;
                                pts.push(self.image_to_screen(
                                    source_center.0 + source_radius.0 * theta.cos(),
                                    source_center.1 + source_radius.1 * theta.sin(),
                                    transform,
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
    fn draw_shape_outlines(&self, ui: &mut egui::Ui, transform: &DisplayedImageTransform) {
        if self.erase_preview_active
            || self.erase_tool != EraseTool::Select
            || self.erase_shapes.is_empty()
        {
            return;
        }
        let Some((scale, _img_rect)) = self.erase_image_layout(transform) else {
            return;
        };

        let painter = ui.painter().with_clip_rect(transform.viewport_rect);
        let to_screen = |p: (f32, f32)| self.image_to_screen(p.0, p.1, transform);
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
        transform: &DisplayedImageTransform,
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
                self.image_to_screen(axis + tilt_offset, span, transform)
            } else {
                self.image_to_screen(span, axis + tilt_offset, transform)
            }
        };

        if tilt.abs() < 0.5 {
            let pts = vec![
                corner(a0, span_min, 0.0),
                corner(a1, span_min, 0.0),
                corner(a1, span_max, 0.0),
                corner(a0, span_max, 0.0),
            ];
            ui.painter().add(egui::Shape::convex_polygon(
                pts,
                color,
                egui::Stroke::new(1.0, stroke_color),
            ));
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
        transform: &DisplayedImageTransform,
    ) {
        if self.erase_tool != EraseTool::Brush {
            return;
        }
        if let Some(pos) = ctx.input(|i| i.pointer.hover_pos()) {
            if transform.contains_screen(pos) {
                let Some((total_scale, _)) = self.erase_image_layout(transform) else {
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
                        crate::os_theme::apply_dark_ui(ui);

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
                                self.keymap
                                    .compact_action_label("描画", KeyAction::ErasePaintMode),
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
                                self.keymap
                                    .compact_action_label("消去", KeyAction::EraseEraseMode),
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
                        let bitmap_rows: &[&[(&str, EraseTool)]] = &[
                            &[("筆", EraseTool::Brush), ("バケツ", EraseTool::Bucket)],
                            &[("囲み", EraseTool::Lasso), ("多角形", EraseTool::Polygon)],
                        ];
                        for row in bitmap_rows {
                            ui.horizontal(|ui| {
                                ui.spacing_mut().item_spacing.x = 4.0;
                                for &(base_label, tool) in *row {
                                    let label = self.erase_tool_label(base_label, tool);
                                    if panel_toggle_button(
                                        ui,
                                        label.as_str(),
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
                        ui.horizontal(|ui| {
                            ui.spacing_mut().item_spacing.x = 4.0;
                            if ui
                                .add_sized(btn_size, egui::Button::new("1px拡張"))
                                .clicked()
                            {
                                self.apply_erase_bitmap_morph_1px(true);
                            }
                            if ui
                                .add_sized(btn_size, egui::Button::new("1px縮小"))
                                .clicked()
                            {
                                self.apply_erase_bitmap_morph_1px(false);
                            }
                        });
                        ui.add(
                            egui::Label::new(
                                egui::RichText::new(
                                    "ビットマップ本体のみ。オブジェクトには影響しません。",
                                )
                                .size(10.0)
                                .color(egui::Color32::from_gray(150)),
                            )
                            .wrap(),
                        );
                        ui.label(
                            egui::RichText::new("オブジェクト:")
                                .color(egui::Color32::from_gray(200)),
                        );
                        let object_rows: [[(&str, EraseTool); 2]; 3] = [
                            [
                                ("選択", EraseTool::Select),
                                ("直線", EraseTool::Line),
                            ],
                            [
                                ("縦線", EraseTool::VertLine),
                                ("横線", EraseTool::HorizLine),
                            ],
                            [
                                ("矩形", EraseTool::Rect),
                                ("楕円", EraseTool::Ellipse),
                            ],
                        ];
                        for row in object_rows.iter() {
                            ui.horizontal(|ui| {
                                ui.spacing_mut().item_spacing.x = 4.0;
                                for &(base_label, tool) in row.iter() {
                                    let label = self.erase_tool_label(base_label, tool);
                                    if panel_toggle_button(
                                        ui,
                                        label.as_str(),
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
                        if self.erase_tool == EraseTool::Bucket {
                            ui.add(
                                egui::Slider::new(
                                    &mut self.erase_bucket_tolerance,
                                    0.0..=255.0,
                                )
                                .text("色差の許容値")
                                .step_by(1.0),
                            );
                            ui.checkbox(&mut self.erase_bucket_connected, "隣接のみ");
                            if self.erase_bucket_connected {
                                ui.add(
                                    egui::Slider::new(
                                        &mut self.erase_bucket_leak_stop,
                                        0.0..=16.0
                                    )
                                    .text("漏れ止め")
                                    .step_by(0.1),
                                )
                                .on_hover_text("細い線や小さな隙間から塗りが漏れるのを防ぎます。0 で無効。");
                            }
                            ui.add(
                                egui::Label::new(
                                    egui::RichText::new(
                                        "画像をクリックしたときに1回だけ塗りつぶします。",
                                    )
                                    .size(10.0)
                                    .color(egui::Color32::from_gray(150)),
                                )
                                .wrap(),
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
                                    "フルスクリーン中 F7/F8 で消しゴム保存 1/2 を即適用",
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
                            Ctrl+ホイール:ズーム\n\
                            矢印:シフト [/]:回転 (Ctrl:10倍)\n\
                            S:選択/ハンドル微調整\n\
                            Shift/Alt+ハンドル:拘束/中心固定\n\
                            多角形:始点クリック/右クリック/Enterで確定 Escで取消\n\
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

    fn erase_working_size(&self, fs_idx: usize, raw_size: [usize; 2]) -> [usize; 2] {
        if let Some(pixels) = self.current_raw_source_pixels(fs_idx) {
            return pixels.size;
        }
        raw_size
    }

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
        let source = self
            .current_raw_source_pixels(fs_idx)
            .filter(size_ok)
            .or_else(|| {
                self.erase_base_cache
                    .get(&fs_idx)
                    .filter(|pixels| size_ok(pixels))
                    .map(Arc::clone)
            })?;
        let source = self.black_flatten_erase_source_if_needed(fs_idx, source);
        Some((source, "source"))
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
                self.mask_page_keys.remove(&key);
                return None;
            }
        };
        let mut composite = bitmap;
        crate::mask_db::rasterize_shapes_into(&mut composite, &shapes, w, h);
        if !composite.iter().any(|&m| m) {
            self.mask_pages.remove(&fs_idx);
            self.mask_page_keys.remove(&key);
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
        // プレビュー済みの MI-GAN 結果があるなら、確定時に再推論せずそのまま
        // 通常表示用 cache へ昇格する。マスク/入力変更時は clear_erase_preview()
        // が走るため、ここに残っている cache は現在の編集状態に対応している。
        let preview_result = self
            .erase_preview_cache
            .get(&fs_idx)
            .filter(|entry| entry.pixels.size == [w, h])
            .map(|entry| (Arc::clone(&entry.pixels), entry.texture.clone()));
        // ビットマップとベクタを別々に永続化することで、再編集時に両者を分離して読み直せる。
        let vectors_snapshot = self.erase_shapes.clone();
        self.save_mask_with_sidecar(fs_idx, &bitmap, &vectors_snapshot, w, h);
        if let Some((pixels, texture)) = preview_result {
            let key = self.current_erase_result_key(fs_idx);
            self.erase_result_cache
                .insert(key, crate::app::EraseResultCacheEntry { pixels, texture });
            self.invalidate_compare_prepared_for_idx(fs_idx);
            self.clear_conceal_caches(fs_idx);
            crate::logger::log(format!(
                "erase: promoted preview result to commit cache idx={fs_idx}"
            ));
            return ApplyInpaintOutcome::CompletedFromPreview;
        }
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
            ApplyInpaintOutcome::CompletedFromPreview => {
                self.show_feedback_toast("[補完適用]".to_string());
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
            self.mask_page_keys.remove(&key);
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
            self.show_feedback_toast(format!("[消しゴムスロット{slot}は空です]"));
            return;
        };
        if !new_mask.iter().any(|&m| m) && new_vectors.is_empty() {
            self.show_feedback_toast(format!("[消しゴムスロット{slot}は空です]"));
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
        self.show_feedback_toast(format!("[消しゴムスロット{slot}適用]"));
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
        if self.remote_session_blocks_local_control() {
            return;
        }
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
        let (progress_tx, progress_rx) = mpsc::channel::<EraseInpaintProgress>();
        let ctx_clone = ctx.clone();
        let local_ai_activity = self.local_ai_activity_lease();

        let spawn_result = std::thread::Builder::new()
            .name("erase-inpaint".to_string())
            .spawn(move || {
                let _local_ai_activity = local_ai_activity;
                let result = crate::edit_source::run_inpaint_pure(
                    runtime.as_ref(),
                    &manager,
                    &original,
                    &composite,
                    w,
                    h,
                    &cancel_for_thread,
                    log_prefix,
                    Some(&progress_tx),
                )
                .image;
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
                progress_rx,
                progress: EraseInpaintProgress::Preparing,
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
        // 結果とは別チャネルで届く軽量な進捗を全件取り込み、各 pending に最新値だけ残す。
        // UI thread は推論状態を共有ロックせず参照できる。
        for pending in self.erase_inpaint_pending.values_mut() {
            while let Ok(progress) = pending.progress_rx.try_recv() {
                pending.progress = progress;
            }
        }

        // 各 pending を try_recv で peek。Ok ならその idx と結果を持って break、
        // Disconnected なら worker が死んでいるので削除して次へ進む。Empty は次へ。
        // (try_recv は &Receiver で借用するだけだが値を返すと所有権が移るので、
        //  Some を返した時点でループを抜けて map から remove する。)
        let mut completed: Option<(EraseInpaintPendingKey, egui::ColorImage)> = None;
        let mut worker_failed = false;
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
                    let cancelled = pending.cancel.load(Ordering::Relaxed);
                    self.erase_inpaint_pending.remove(&key);
                    worker_failed |= !cancelled;
                }
                Err(mpsc::TryRecvError::Empty) => {}
            }
        }
        if worker_failed {
            self.show_feedback_toast("[補完失敗]".to_string());
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
            self.display_texture_options(idx),
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
            self.show_feedback_toast("[補完プレビュー完了]".to_string());
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
            self.clear_local_adjust_caches_for_idx(idx);
            self.clear_conceal_caches(idx);
            crate::logger::log(format!(
                "erase: inpaint complete ({} ms, prefix={})",
                elapsed.as_millis(),
                pending.log_prefix
            ));
            self.show_feedback_toast("[補完完了]".to_string());
        }
    }

    /// 消しゴム補完 worker が生存している間だけ表示する持続型ステータス。
    ///
    /// 一時トーストとは寿命を分け、保存済みマスクの自動再生成 (`ensure-result`) も含めて
    /// 「処理中なのに表示が消えた」状態を作らない。推論時間のばらつきが大きいため ETA は
    /// 出さず、パス / タイル番号と動くインジケーターで生存を伝える。
    pub(crate) fn draw_erase_inpaint_progress(
        &mut self,
        ui: &mut egui::Ui,
        full_rect: egui::Rect,
        ctx: &egui::Context,
        drawing_surface: crate::app::ActionSurface,
    ) {
        let target_surface = if self.fullscreen_idx.is_some() {
            crate::app::ActionSurface::Viewer
        } else {
            crate::app::ActionSurface::MainWindow
        };
        if drawing_surface != target_surface || self.erase_inpaint_pending.is_empty() {
            return;
        }

        let job_count = self.erase_inpaint_pending.len();
        let current_idx = self.fullscreen_idx;
        let selected = self
            .erase_inpaint_pending
            .iter()
            .filter(|(key, _)| current_idx.is_none_or(|idx| key.idx == idx))
            .max_by_key(|(_, pending)| pending.started_at)
            .or_else(|| {
                self.erase_inpaint_pending
                    .iter()
                    .max_by_key(|(_, pending)| pending.started_at)
            })
            .map(|(key, pending)| (key.kind, pending.progress, pending.started_at));
        let Some((kind, progress, started_at)) = selected else {
            return;
        };

        // 高速に終わる処理では一瞬だけ点滅させない。表示待ちの間も次フレームを予約する。
        let elapsed = started_at.elapsed();
        if elapsed < std::time::Duration::from_millis(150) {
            ctx.request_repaint_after(std::time::Duration::from_millis(33));
            return;
        }

        let text = erase_inpaint_progress_label(kind, progress, job_count);
        let font = egui::FontId::proportional(17.0);
        let galley = ui
            .painter()
            .layout_no_wrap(text.clone(), font.clone(), egui::Color32::WHITE);
        let padding = egui::vec2(14.0, 9.0);
        let indicator_h = 3.0;
        let box_size = egui::vec2(
            galley.size().x + padding.x * 2.0,
            galley.size().y + padding.y * 2.0 + indicator_h + 4.0,
        );
        let min_x = (full_rect.max.x - box_size.x - 20.0).max(full_rect.min.x + 8.0);
        let min_y = (full_rect.min.y + 110.0)
            .min((full_rect.max.y - box_size.y - 8.0).max(full_rect.min.y + 8.0));
        let rect = egui::Rect::from_min_size(egui::pos2(min_x, min_y), box_size);
        ui.painter().rect_filled(
            rect,
            8.0,
            egui::Color32::from_rgba_unmultiplied(30, 30, 30, 225),
        );
        ui.painter().text(
            egui::pos2(rect.center().x, rect.center().y - 3.0),
            egui::Align2::CENTER_CENTER,
            text,
            font,
            egui::Color32::WHITE,
        );

        // 1 タイルの所要時間も環境依存なので、数値更新の間も動いて見える indeterminate bar。
        let track = egui::Rect::from_min_max(
            egui::pos2(rect.min.x + padding.x, rect.max.y - padding.y),
            egui::pos2(rect.max.x - padding.x, rect.max.y - padding.y + indicator_h),
        );
        ui.painter().rect_filled(
            track,
            indicator_h * 0.5,
            egui::Color32::from_rgba_unmultiplied(255, 255, 255, 45),
        );
        let segment_w = (track.width() * 0.28).max(20.0).min(track.width());
        let travel = (track.width() - segment_w).max(0.0);
        let phase = (elapsed.as_secs_f32() * 0.8) % 1.0;
        let segment = egui::Rect::from_min_size(
            egui::pos2(track.min.x + travel * phase, track.min.y),
            egui::vec2(segment_w, indicator_h),
        );
        ui.painter().rect_filled(
            segment,
            indicator_h * 0.5,
            egui::Color32::from_rgb(90, 170, 255),
        );

        ctx.request_repaint_after(std::time::Duration::from_millis(33));
    }
}

/// worker thread で走る inpaint 本体。`AiRuntime` が利用可能なら MI-GAN、
/// 失敗 / runtime 不在なら拡散 fallback。`&mut self` を取らないことで
/// UI スレッドに戻らずに完結できる。
pub(crate) struct InpaintOutcome {
    pub(crate) image: egui::ColorImage,
    pub(crate) used_diffusion_fallback: bool,
}

/// 保存済み mask snapshot を headless に再適用する製本向け入口。
/// Shape のラスタライズと透明画像の黒 flatten をここへ集約し、表示用 inpaint と
/// 同じ MI-GAN → diffusion fallback を使う。
pub(crate) fn erase_from_saved_mask(
    runtime: Option<&Arc<crate::ai::runtime::AiRuntime>>,
    manager: &Arc<crate::ai::model_manager::ModelManager>,
    base: &egui::ColorImage,
    bitmap: &[bool],
    shapes: &[crate::mask_db::Shape],
    cancel: &Arc<AtomicBool>,
    log_prefix: &str,
) -> Result<InpaintOutcome, String> {
    let [w, h] = base.size;
    if bitmap.len() != w.saturating_mul(h) {
        return Err(format!(
            "消しゴムマスクのサイズが一致しません: mask={}, expected={}",
            bitmap.len(),
            w.saturating_mul(h)
        ));
    }
    let mut composite = bitmap.to_vec();
    crate::mask_db::rasterize_shapes_into(&mut composite, shapes, w, h);
    if !composite.iter().any(|masked| *masked) {
        return Ok(InpaintOutcome {
            image: base.clone(),
            used_diffusion_fallback: false,
        });
    }
    let flattened = black_flatten_if_transparent(base);
    let input = flattened.as_ref().unwrap_or(base);
    Ok(crate::edit_source::run_inpaint_pure(
        runtime, manager, input, &composite, w, h, cancel, log_prefix, None,
    ))
}

// ═══════════════════════════════════════════════════════════════════════
// Free functions
// ═══════════════════════════════════════════════════════════════════════

/// タイルオーバーラップ幅（ピクセル）。
const TILE_OVERLAP: usize = 64;
/// 大きい穴を既知画素側から内側へ埋める 1 段あたりの深さ。
const INPAINT_STAGE_DEPTH: u32 = 48;
/// マスク周囲から MI-GAN 入力へ含める既知画素の目安。
const INPAINT_CONTEXT_PAD: usize = MIGAN_SIZE / 4;

/// MI-GAN による段階的タイル inpainting。
///
/// 実在する既知画素に近い領域から 48px ずつ内側へ確定する。各段では
/// 「まだ埋めていない全領域」をモデル入力上の hole として隠し、「現在の帯」だけを
/// 出力へ採用する。前段の生成結果は、次段で初めて既知画素として利用される。
pub(crate) fn inpaint_migan(
    runtime: &crate::ai::runtime::AiRuntime,
    original: &egui::ColorImage,
    mask: &[bool],
    w: usize,
    h: usize,
    cancel: &Arc<std::sync::atomic::AtomicBool>,
    progress_tx: Option<&mpsc::Sender<EraseInpaintProgress>>,
) -> Result<egui::ColorImage, crate::ai::AiError> {
    use std::sync::atomic::Ordering;

    if w == 0 || h == 0 || mask.len() < w.saturating_mul(h) {
        return Err(crate::ai::AiError::ImageProcessing(
            "Invalid MI-GAN image or mask dimensions".to_string(),
        ));
    }
    if !mask.iter().any(|masked| *masked) {
        return Err(crate::ai::AiError::ImageProcessing(
            "No masked pixels".to_string(),
        ));
    }

    let (distance, max_distance) = inpaint_distance_from_known(mask, w, h);
    let finite_passes = if max_distance == 0 {
        0
    } else {
        max_distance.div_ceil(INPAINT_STAGE_DEPTH) as usize
    };
    let pass_count = finite_passes.max(1);
    crate::logger::log(format!(
        "[erase] MI-GAN staged: max-known-depth={max_distance}px, passes={pass_count}, band={INPAINT_STAGE_DEPTH}px"
    ));

    let mut working = original.clone();
    let mut remaining = mask.to_vec();
    for pass_index in 0..finite_passes {
        if cancel.load(Ordering::Relaxed) {
            return Err(crate::ai::AiError::Cancelled);
        }
        let upper = ((pass_index as u32 + 1) * INPAINT_STAGE_DEPTH).min(max_distance);
        let commit: Vec<bool> = remaining
            .iter()
            .zip(distance.iter())
            .map(|(&pending, &depth)| pending && depth != u32::MAX && depth <= upper)
            .collect();
        if !commit.iter().any(|selected| *selected) {
            continue;
        }
        working = inpaint_migan_single_pass(
            runtime,
            &working,
            &remaining,
            &commit,
            w,
            h,
            cancel,
            pass_index + 1,
            pass_count,
            progress_tx,
        )?;
        remaining
            .iter_mut()
            .zip(commit.iter())
            .for_each(|(pending, selected)| {
                if *selected {
                    *pending = false;
                }
            });
    }

    if remaining.iter().any(|pending| *pending) {
        // 全面マスク等、実在する既知画素へ到達できない成分は従来相当の 1 pass に倒す。
        working = inpaint_migan_single_pass(
            runtime,
            &working,
            &remaining,
            &remaining,
            w,
            h,
            cancel,
            finite_passes + 1,
            finite_passes + 1,
            progress_tx,
        )?;
    }
    Ok(working)
}

/// MI-GAN の 1 段を実行する。
/// `input_mask` は未修復領域全体、`commit_mask` は今回確定する帯だけを表す。
#[allow(clippy::too_many_arguments)]
fn inpaint_migan_single_pass(
    runtime: &crate::ai::runtime::AiRuntime,
    original: &egui::ColorImage,
    input_mask: &[bool],
    commit_mask: &[bool],
    w: usize,
    h: usize,
    cancel: &Arc<std::sync::atomic::AtomicBool>,
    pass_index: usize,
    pass_count: usize,
    progress_tx: Option<&mpsc::Sender<EraseInpaintProgress>>,
) -> Result<egui::ColorImage, crate::ai::AiError> {
    use std::sync::atomic::Ordering;

    // マスクのバウンディングボックス
    let (mut min_x, mut min_y, mut max_x, mut max_y) = (w, h, 0usize, 0usize);
    for py in 0..h {
        for px in 0..w {
            if commit_mask[py * w + px] {
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

    // 512px より小さい切り出しを縦横別倍率で引き伸ばすと形状とコンテキストが
    // 歪む。画像が 512px 以上なら必ず原寸 512px 以上の領域へ広げる。
    let (region_x0, region_x1) = fit_inpaint_region_axis(min_x, max_x, w);
    let (region_y0, region_y1) = fit_inpaint_region_axis(min_y, max_y, h);
    let region_w = region_x1 - region_x0;
    let region_h = region_y1 - region_y0;

    // タイル分割を計算
    let tiles = compute_inpaint_tiles(region_w, region_h, MIGAN_SIZE, TILE_OVERLAP);

    crate::logger::log(format!(
        "[erase] MI-GAN pass {pass_index}/{pass_count}: region ({region_x0},{region_y0})-({region_x1},{region_y1}) = {region_w}x{region_h}, {} tiles",
        tiles.len()
    ));
    report_inpaint_progress(
        progress_tx,
        EraseInpaintProgress::Running {
            pass_index,
            pass_count,
            tile_index: 0,
            tile_count: tiles.len(),
        },
    );

    // 累積バッファ（region 座標系、RGB float + 重み）
    let rpixels = region_w * region_h;
    let mut accum_r = vec![0.0f32; rpixels];
    let mut accum_g = vec![0.0f32; rpixels];
    let mut accum_b = vec![0.0f32; rpixels];
    let mut accum_w = vec![0.0f32; rpixels];

    for (ti, tile) in tiles.iter().enumerate() {
        if cancel.load(Ordering::Relaxed) {
            return Err(crate::ai::AiError::Cancelled);
        }

        // タイル内にマスクピクセルがなければスキップ
        let has_mask = (tile.y..tile.y + tile.h).any(|ty| {
            (tile.x..tile.x + tile.w).any(|tx| {
                let gx = region_x0 + tx;
                let gy = region_y0 + ty;
                gx < w && gy < h && commit_mask[gy * w + gx]
            })
        });
        if !has_mask {
            report_inpaint_progress(
                progress_tx,
                EraseInpaintProgress::Running {
                    pass_index,
                    pass_count,
                    tile_index: ti + 1,
                    tile_count: tiles.len(),
                },
            );
            continue;
        }

        // 原寸 1:1 で 512×512 tensor へ配置する。512px 未満の画像だけは中央へ
        // letterbox し、余白を hole として扱う。マスク RGB は必ず 0 になる。
        let s = MIGAN_SIZE;
        let input_nchw = build_migan_input(original, input_mask, w, h, region_x0, region_y0, *tile);

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

        let canvas_x = (s - tile.w) / 2;
        let canvas_y = (s - tile.h) / 2;
        for ty in 0..tile.h {
            for tx in 0..tile.w {
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
                if !commit_mask[gy * w + gx] {
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
                let si = ((canvas_y + ty) * s + canvas_x + tx) * 3;
                accum_r[ri] += tile_rgb[si] * weight;
                accum_g[ri] += tile_rgb[si + 1] * weight;
                accum_b[ri] += tile_rgb[si + 2] * weight;
                accum_w[ri] += weight;
            }
        }

        crate::logger::log(format!(
            "[erase] MI-GAN pass {pass_index}/{pass_count} tile {}/{}",
            ti + 1,
            tiles.len()
        ));
        report_inpaint_progress(
            progress_tx,
            EraseInpaintProgress::Running {
                pass_index,
                pass_count,
                tile_index: ti + 1,
                tile_count: tiles.len(),
            },
        );
    }

    crate::logger::log(format!(
        "[erase] MI-GAN pass {pass_index}/{pass_count} done, compositing..."
    ));
    report_inpaint_progress(
        progress_tx,
        EraseInpaintProgress::Compositing {
            pass_index,
            pass_count,
        },
    );

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
            if !commit_mask[src_idx] {
                continue;
            }

            let ri = ry * region_w + rx;
            let wt = accum_w[ri];
            if wt <= f32::EPSILON {
                continue;
            }
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

/// mask bbox + context を画像内へ収め、可能なら少なくとも 512px の原寸領域にする。
fn fit_inpaint_region_axis(min: usize, max: usize, image_len: usize) -> (usize, usize) {
    if image_len == 0 {
        return (0, 0);
    }
    let mut start = min.saturating_sub(INPAINT_CONTEXT_PAD);
    let mut end = max.saturating_add(INPAINT_CONTEXT_PAD).min(image_len);
    let target = MIGAN_SIZE.min(image_len);
    if end.saturating_sub(start) < target {
        let mut missing = target - (end - start);
        let before = (missing / 2).min(start);
        start -= before;
        missing -= before;
        let after = missing.min(image_len - end);
        end += after;
        missing -= after;
        let before = missing.min(start);
        start -= before;
    }
    (start, end)
}

/// 実在する非マスク画素からの 4-neighbor 距離。
///
/// 画像外周は既知画素ではない。上端/右端等に接したマスクを外周から埋め始めると、
/// コンテキストが存在しない側の生成を先に確定してしまうためである。
fn inpaint_distance_from_known(mask: &[bool], w: usize, h: usize) -> (Vec<u32>, u32) {
    let len = w.saturating_mul(h);
    let mut distance = vec![u32::MAX; len];
    let mut queue = VecDeque::new();
    for y in 0..h {
        for x in 0..w {
            let index = y * w + x;
            if !mask.get(index).copied().unwrap_or(false) {
                continue;
            }
            let adjacent_known = inpaint_neighbors(x, y, w, h)
                .any(|neighbor| !mask.get(neighbor).copied().unwrap_or(false));
            if adjacent_known {
                distance[index] = 1;
                queue.push_back(index);
            }
        }
    }
    let mut max_distance = 0;
    while let Some(index) = queue.pop_front() {
        let current = distance[index];
        max_distance = max_distance.max(current);
        let x = index % w;
        let y = index / w;
        for neighbor in inpaint_neighbors(x, y, w, h) {
            if mask[neighbor] && distance[neighbor] == u32::MAX {
                distance[neighbor] = current.saturating_add(1);
                queue.push_back(neighbor);
            }
        }
    }
    (distance, max_distance)
}

fn inpaint_neighbors(x: usize, y: usize, w: usize, h: usize) -> impl Iterator<Item = usize> {
    let mut neighbors = [usize::MAX; 4];
    let mut count = 0;
    if x > 0 {
        neighbors[count] = y * w + x - 1;
        count += 1;
    }
    if x + 1 < w {
        neighbors[count] = y * w + x + 1;
        count += 1;
    }
    if y > 0 {
        neighbors[count] = (y - 1) * w + x;
        count += 1;
    }
    if y + 1 < h {
        neighbors[count] = (y + 1) * w + x;
        count += 1;
    }
    neighbors.into_iter().take(count)
}

#[allow(clippy::too_many_arguments)]
fn build_migan_input(
    original: &egui::ColorImage,
    input_mask: &[bool],
    w: usize,
    h: usize,
    region_x0: usize,
    region_y0: usize,
    tile: TileRect,
) -> ndarray::Array4<f32> {
    let s = MIGAN_SIZE;
    let mut input = ndarray::Array4::<f32>::zeros((1, 4, s, s));
    for iy in 0..s {
        for ix in 0..s {
            input[[0, 0, iy, ix]] = -0.5;
        }
    }
    let canvas_x = (s - tile.w) / 2;
    let canvas_y = (s - tile.h) / 2;
    for ty in 0..tile.h {
        for tx in 0..tile.w {
            let gx = region_x0 + tile.x + tx;
            let gy = region_y0 + tile.y + ty;
            if gx >= w || gy >= h {
                continue;
            }
            let source = gy * w + gx;
            let m = if input_mask.get(source).copied().unwrap_or(true) {
                0.0
            } else {
                1.0
            };
            let color = original.pixels[source];
            let iy = canvas_y + ty;
            let ix = canvas_x + tx;
            input[[0, 0, iy, ix]] = m - 0.5;
            input[[0, 1, iy, ix]] = (color.r() as f32 / 255.0 * 2.0 - 1.0) * m;
            input[[0, 2, iy, ix]] = (color.g() as f32 / 255.0 * 2.0 - 1.0) * m;
            input[[0, 3, iy, ix]] = (color.b() as f32 / 255.0 * 2.0 - 1.0) * m;
        }
    }
    input
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct TileRect {
    x: usize,
    y: usize,
    w: usize,
    h: usize,
}

pub(crate) fn inpaint_diffuse(
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
/// 消しゴム (erase) と隠蔽加工 (conceal) の筆ツールで共用する。
pub(crate) fn draw_dashed_circle(painter: &egui::Painter, center: egui::Pos2, radius: f32) {
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

#[cfg(test)]
mod book_erase_tests {
    use super::*;

    #[test]
    fn erase_bucket_tool_maps_to_bucket_key_action() {
        assert_eq!(
            App::erase_tool_key_action(EraseTool::Bucket),
            KeyAction::EraseToolBucket
        );
    }

    #[test]
    fn inpaint_progress_label_reports_pass_tile_and_parallel_job_count() {
        let label = erase_inpaint_progress_label(
            EraseInpaintKind::Commit,
            EraseInpaintProgress::Running {
                pass_index: 3,
                pass_count: 12,
                tile_index: 2,
                tile_count: 4,
            },
            2,
        );
        assert_eq!(label, "AI補完中 3/12（タイル 2/4） / 全2件");
    }

    #[test]
    fn inpaint_progress_label_distinguishes_preview_and_fallback() {
        assert_eq!(
            erase_inpaint_progress_label(
                EraseInpaintKind::Preview,
                EraseInpaintProgress::Preparing,
                1,
            ),
            "AI補完プレビューを準備中"
        );
        assert_eq!(
            erase_inpaint_progress_label(
                EraseInpaintKind::Commit,
                EraseInpaintProgress::DiffusionFallback,
                1,
            ),
            "補完の代替処理中"
        );
    }

    #[test]
    fn inpaint_progress_is_delivered_over_worker_channel() {
        let (tx, rx) = mpsc::channel();
        let progress = EraseInpaintProgress::Compositing {
            pass_index: 2,
            pass_count: 3,
        };
        report_inpaint_progress(Some(&tx), progress);
        assert_eq!(rx.try_recv().unwrap(), progress);
    }

    #[test]
    fn inpaint_region_expands_top_and_right_edge_masks_to_native_512_square() {
        // 時計例: bbox x=33..282, y=0..188。旧実装は 410x316 を 512x512 へ歪めた。
        assert_eq!(fit_inpaint_region_axis(33, 282, 896), (0, 512));
        assert_eq!(fit_inpaint_region_axis(0, 188, 1152), (0, 512));

        // 右下例: bbox x=693..896。右へ広げられない分を左へ寄せて原寸を保つ。
        assert_eq!(fit_inpaint_region_axis(693, 896, 896), (384, 896));
        assert_eq!(fit_inpaint_region_axis(794, 1060, 1152), (640, 1152));
    }

    #[test]
    fn inpaint_distance_does_not_treat_image_edge_as_known_context() {
        let (w, h) = (7, 6);
        let mut mask = vec![false; w * h];
        // 上端に接する 5x5 hole。上端中央は左右/下の実在背景まで 3px。
        for y in 0..5 {
            for x in 1..6 {
                mask[y * w + x] = true;
            }
        }
        let (distance, max_distance) = inpaint_distance_from_known(&mask, w, h);
        assert_eq!(distance[3], 3);
        assert_eq!(max_distance, 3);
    }

    #[test]
    fn inpaint_distance_splits_deep_hole_into_multiple_48px_stages() {
        let (w, h) = (132, 132);
        let mut mask = vec![false; w * h];
        for y in 1..131 {
            for x in 1..131 {
                mask[y * w + x] = true;
            }
        }
        let (distance, max_distance) = inpaint_distance_from_known(&mask, w, h);
        assert_eq!(max_distance, 65);
        assert_eq!(max_distance.div_ceil(INPAINT_STAGE_DEPTH), 2);

        let first = mask
            .iter()
            .zip(distance.iter())
            .filter(|(masked, depth)| **masked && **depth <= INPAINT_STAGE_DEPTH)
            .count();
        let second = mask
            .iter()
            .zip(distance.iter())
            .filter(|(masked, depth)| **masked && **depth > INPAINT_STAGE_DEPTH)
            .count();
        assert!(first > 0);
        assert!(second > 0);
        assert_eq!(
            first + second,
            mask.iter().filter(|masked| **masked).count()
        );
    }

    #[test]
    fn migan_input_zeroes_masked_rgb_and_letterboxes_without_rescaling() {
        let image = egui::ColorImage::new([2, 1], vec![egui::Color32::RED, egui::Color32::GREEN]);
        let input = build_migan_input(
            &image,
            &[true, false],
            2,
            1,
            0,
            0,
            TileRect {
                x: 0,
                y: 0,
                w: 2,
                h: 1,
            },
        );
        let x0 = (MIGAN_SIZE - 2) / 2;
        let y0 = (MIGAN_SIZE - 1) / 2;
        assert_eq!(input[[0, 0, y0, x0]], -0.5);
        assert_eq!(input[[0, 1, y0, x0]], 0.0);
        assert_eq!(input[[0, 2, y0, x0]], 0.0);
        assert_eq!(input[[0, 3, y0, x0]], 0.0);

        assert_eq!(input[[0, 0, y0, x0 + 1]], 0.5);
        assert_eq!(input[[0, 1, y0, x0 + 1]], -1.0);
        assert_eq!(input[[0, 2, y0, x0 + 1]], 1.0);
        assert_eq!(input[[0, 3, y0, x0 + 1]], -1.0);
        // letterbox の余白は hole 扱いで、画像を引き伸ばして埋めない。
        assert_eq!(input[[0, 0, 0, 0]], -0.5);
        assert_eq!(input[[0, 1, 0, 0]], 0.0);
    }

    #[test]
    fn saved_mask_without_runtime_uses_deterministic_diffusion_fallback() {
        let base = egui::ColorImage::new(
            [3, 1],
            vec![
                egui::Color32::RED,
                egui::Color32::GREEN,
                egui::Color32::BLUE,
            ],
        );
        let manager = Arc::new(crate::ai::model_manager::ModelManager::new());
        let cancel = Arc::new(AtomicBool::new(false));

        let result = erase_from_saved_mask(
            None,
            &manager,
            &base,
            &[false, true, false],
            &[],
            &cancel,
            "book-test",
        )
        .unwrap();

        assert!(result.used_diffusion_fallback);
        assert_ne!(result.image.pixels[1], base.pixels[1]);
        assert_eq!(result.image.pixels[0], base.pixels[0]);
        assert_eq!(result.image.pixels[2], base.pixels[2]);
    }
}
