//! テキスト注釈 (comic) のフルスクリーン編集モード。
//!
//! 消しゴム ([`crate::ui_erase`]) / 隠蔽 ([`crate::ui_conceal`]) と同系列の「4 つ目の
//! 編集モード」。Ctrl+T で入退場する (D2、モード名「テキスト」)。注釈ロジック自体は
//! egui 非依存の `comic-core` に置き、本モジュールは mIV 側の UI / 入力 / 永続化配線
//! だけを担う。
//!
//! ## スコープ (Inc 3b/3c/3d + 基本 Inc 4 編集)
//!
//! - 入退場 (`enter_text_mode` / `reset_text_mode`): 見開き → Single ピボット、
//!   `comic_docs` (page_path_key 別の作業セット) のロード、退場時に comic.db + サイドカー
//!   へ保存。
//! - 座標逆写像 (`text_img_view` / `TextImgView`): 回転 (rotation_db 90°単位 + フリー回転) と
//!   zoom/pan を逆変換して、画面座標 ↔ canonical ソース画素座標を相互変換する (D8)。
//! - 選択 / ドラッグ移動 (`handle_text_canvas_input`): クリックでオブジェクト選択
//!   (当たり判定 = comic-core ジオメトリ `object_bounds`)、ドラッグで pivot を移動。
//! - パネル (`draw_text_panel`): オブジェクト追加 / 一覧 (選択・複製・削除・前後) /
//!   選択中オブジェクトの種別別インライン編集 (テキスト内容・サイズ・色・向き・袋文字・
//!   吹き出し形状・塗り・ウィンドウ枠 等)。IME 安全なテキスト入力。
//!
//! 変形ハンドル (四隅スケール / 回転ノブ / しっぽ) と Undo/Redo は Inc 6、スタンプ
//! ピッカー (絵文字アセット) は Inc 4c、プリセット / 追加ダイアログは Inc 5。

use crate::app::{App, TextDrag};
use crate::ui_fullscreen::{FsKeyAction, SpreadPair};
use comic_core::{
    AnnotationKind, AnnotationObject, BubbleObject, BubbleShape, FillMode, FontSet, FrameStyle,
    MessageWindowObject, Orientation, Rgba, StampObject, StrokeStyle, TailKind, TextAlign,
    TextBlock, WindowPosition,
};

/// パネル幅 (編集コントロールが入るので conceal より少し広い)。
const PANEL_W: f32 = 268.0;
const PANEL_MARGIN_X: f32 = 16.0;
const PANEL_MARGIN_Y: f32 = 60.0;

/// 選択枠の色 (環境依存グリフを使わない純描画)。
fn sel_color() -> egui::Color32 {
    egui::Color32::from_rgb(80, 180, 255)
}

// ── 座標変換 (回転 D8 対応) ──────────────────────────────────────────────

/// 表示中の画像レイアウト + 回転情報。画面座標 ↔ canonical ソース画素座標を相互変換する。
///
/// 表示テクスチャ (comic composite) は AI アップスケール後の解像度になりうるが、その
/// アスペクト比はソースと同じなので、**ソース寸法 `(sw, sh)` で fit を計算しても
/// 画面上の画像矩形 `img_rect` は一致する** (アップスケール係数は相殺される)。よって
/// 本 view はソース寸法だけで完結し、得られる画素は常に canonical ソース px になる。
pub(crate) struct TextImgView {
    img_rect: egui::Rect,
    center: egui::Pos2,
    rotation: crate::rotation_db::Rotation,
    free_rot: f32,
    sw: f32,
    sh: f32,
}

/// `draw_rotated_image_ex` の UV 割り当てに対応する forward 写像
/// (canonical 正規化 `(s,t)` → 画像矩形内正規化 `(u,v)`)。
fn forward_uv(rot: crate::rotation_db::Rotation, s: f32, t: f32) -> (f32, f32) {
    use crate::rotation_db::Rotation::*;
    match rot {
        None => (s, t),
        Cw90 => (1.0 - t, s),
        Cw180 => (1.0 - s, 1.0 - t),
        Cw270 => (t, 1.0 - s),
    }
}

/// `forward_uv` の逆写像 (`(u,v)` → `(s,t)`)。
fn inverse_uv(rot: crate::rotation_db::Rotation, u: f32, v: f32) -> (f32, f32) {
    use crate::rotation_db::Rotation::*;
    match rot {
        None => (u, v),
        Cw90 => (v, 1.0 - u),
        Cw180 => (1.0 - u, 1.0 - v),
        Cw270 => (1.0 - v, u),
    }
}

/// `p` を `center` まわりに `theta` (rad, 画面 y 下向き = CW) 回転する。
/// `draw_rotated_image_ex` のフリー回転と同式。
fn rotate_about_pos(p: egui::Pos2, center: egui::Pos2, theta: f32) -> egui::Pos2 {
    if theta.abs() < 1e-6 {
        return p;
    }
    let (sin, cos) = theta.sin_cos();
    let dx = p.x - center.x;
    let dy = p.y - center.y;
    egui::pos2(
        center.x + dx * cos - dy * sin,
        center.y + dx * sin + dy * cos,
    )
}

impl TextImgView {
    /// 画面座標 → canonical ソース画素座標。
    pub(crate) fn screen_to_image(&self, p: egui::Pos2) -> (f32, f32) {
        let q = rotate_about_pos(p, self.center, -self.free_rot);
        let w = self.img_rect.width().max(1e-3);
        let h = self.img_rect.height().max(1e-3);
        let u = (q.x - self.img_rect.min.x) / w;
        let v = (q.y - self.img_rect.min.y) / h;
        let (s, t) = inverse_uv(self.rotation, u, v);
        (s * self.sw, t * self.sh)
    }

    /// canonical ソース画素座標 → 画面座標。
    pub(crate) fn image_to_screen(&self, px: f32, py: f32) -> egui::Pos2 {
        let s = px / self.sw.max(1e-3);
        let t = py / self.sh.max(1e-3);
        let (u, v) = forward_uv(self.rotation, s, t);
        let bx = self.img_rect.min.x + u * self.img_rect.width();
        let by = self.img_rect.min.y + v * self.img_rect.height();
        rotate_about_pos(egui::pos2(bx, by), self.center, self.free_rot)
    }
}

// ── 当たり判定 (comic-core ジオメトリと一致) ──────────────────────────────

/// `p` を `pivot` まわりに `theta` (rad, 画像空間 CW) 回転する。
fn rotate_about(p: (f32, f32), pivot: (f32, f32), theta: f32) -> (f32, f32) {
    let (s, c) = theta.sin_cos();
    let rx = p.0 - pivot.0;
    let ry = p.1 - pivot.1;
    (pivot.0 + rx * c - ry * s, pivot.1 + rx * s + ry * c)
}

/// オブジェクトの canonical 空間 AABB (回転を内包した軸並行境界箱)。
/// ラボ `object_bounds` と同じ計算 (= 当たり判定が見た目と一致)。`fonts` が無ければ
/// pivot 周りの控えめな箱でフォールバックする。
fn object_bounds(o: &AnnotationObject, fonts: Option<&FontSet>) -> egui::Rect {
    use comic_core::{bubble_geometry, effective_bubble_shape, effective_window_half_extents};
    let pivot = o.pivot;
    let rot = o.rotation_rad;
    let mut min = (f32::INFINITY, f32::INFINITY);
    let mut max = (f32::NEG_INFINITY, f32::NEG_INFINITY);
    let mut acc = |x: f32, y: f32| {
        min.0 = min.0.min(x);
        min.1 = min.1.min(y);
        max.0 = max.0.max(x);
        max.1 = max.1.max(y);
    };
    match &o.kind {
        AnnotationKind::Bubble(b) => {
            if let Some(fonts) = fonts {
                let eff = effective_bubble_shape(b, fonts);
                let tail = b
                    .tail
                    .as_ref()
                    .filter(|_| comic_core::shape_renders_tail(&eff));
                let geo = bubble_geometry(&eff, pivot, tail);
                for &(x, y) in &geo.outline {
                    let (px, py) = rotate_about((x, y), pivot, rot);
                    acc(px, py);
                }
                for &(cx, cy, r) in &geo.thought {
                    let (rcx, rcy) = rotate_about((cx, cy), pivot, rot);
                    acc(rcx - r, rcy - r);
                    acc(rcx + r, rcy + r);
                }
                if let Some(t) = tail {
                    let (px, py) = rotate_about(t.tip, pivot, rot);
                    acc(px, py);
                }
                let m = b.outline.width_px.max(0.0) * 0.5 + 2.0;
                if min.0 <= max.0 {
                    return egui::Rect::from_min_max(
                        egui::pos2(min.0 - m, min.1 - m),
                        egui::pos2(max.0 + m, max.1 + m),
                    );
                }
            }
            egui::Rect::from_center_size(egui::pos2(pivot.0, pivot.1), egui::vec2(80.0, 60.0))
        }
        AnnotationKind::Text(t) => {
            let (bw, bh) = fonts
                .and_then(|f| f.get(&t.font_key))
                .map(|font| {
                    let l = comic_core::layout_text(t, font);
                    (l.bounds.0.max(8.0), l.bounds.1.max(8.0))
                })
                .unwrap_or((100.0, 40.0));
            // テキストは pivot = 左上 (ラボ準拠)。回転は中心まわりで包む。
            let cx = pivot.0 + bw * 0.5;
            let cy = pivot.1 + bh * 0.5;
            for &(lx, ly) in &[
                (pivot.0, pivot.1),
                (pivot.0 + bw, pivot.1),
                (pivot.0 + bw, pivot.1 + bh),
                (pivot.0, pivot.1 + bh),
            ] {
                let (px, py) = rotate_about((lx, ly), (cx, cy), rot);
                acc(px, py);
            }
            egui::Rect::from_min_max(egui::pos2(min.0, min.1), egui::pos2(max.0, max.1))
        }
        AnnotationKind::MessageWindow(w) => {
            let (hw, hh) = fonts
                .map(|f| effective_window_half_extents(w, f))
                .unwrap_or((w.half_w, w.half_h));
            for &(lx, ly) in &[(-hw, -hh), (hw, -hh), (hw, hh), (-hw, hh)] {
                let (px, py) = rotate_about((pivot.0 + lx, pivot.1 + ly), pivot, rot);
                acc(px, py);
            }
            let m = w.outline.width_px.max(0.0) * 0.5 + 2.0;
            egui::Rect::from_min_max(
                egui::pos2(min.0 - m, min.1 - m),
                egui::pos2(max.0 + m, max.1 + m),
            )
        }
        AnnotationKind::Stamp(s) => {
            let (hw, hh) = (s.half_w, s.half_h);
            for &(lx, ly) in &[(-hw, -hh), (hw, -hh), (hw, hh), (-hw, hh)] {
                let (px, py) = rotate_about((pivot.0 + lx, pivot.1 + ly), pivot, rot);
                acc(px, py);
            }
            let m = s.outline.map(|o| o.width_px).unwrap_or(0.0).max(0.0) + 2.0;
            egui::Rect::from_min_max(
                egui::pos2(min.0 - m, min.1 - m),
                egui::pos2(max.0 + m, max.1 + m),
            )
        }
    }
}

/// canonical 画素 `img_pt` に最も手前 (z 最大) で当たる有効オブジェクトの id。
fn hit_test(
    objects: &[AnnotationObject],
    img_pt: (f32, f32),
    fonts: Option<&FontSet>,
) -> Option<u64> {
    let p = egui::pos2(img_pt.0, img_pt.1);
    let mut order: Vec<usize> = (0..objects.len()).filter(|&i| objects[i].enabled).collect();
    order.sort_by_key(|&i| std::cmp::Reverse(objects[i].z));
    for &i in &order {
        if object_bounds(&objects[i], fonts).contains(p) {
            return Some(objects[i].id);
        }
    }
    None
}

/// オブジェクト全体 (pivot と吹き出しのしっぽ tip) を `(dx, dy)` 平行移動する。
fn translate_object(o: &mut AnnotationObject, dx: f32, dy: f32) {
    o.pivot.0 += dx;
    o.pivot.1 += dy;
    if let AnnotationKind::Bubble(b) = &mut o.kind {
        if let Some(t) = &mut b.tail {
            t.tip.0 += dx;
            t.tip.1 += dy;
        }
    }
}

/// 新規オブジェクトに割り当てる id (既存最大 + 1)。
fn next_id(objs: &[AnnotationObject]) -> u64 {
    objs.iter().map(|o| o.id).max().unwrap_or(0) + 1
}

/// z を vec 順に正規化する (z == index)。bake は z 昇順で描くので vec 順 = 描画順。
fn normalize_z(objs: &mut [AnnotationObject]) {
    for (i, o) in objs.iter_mut().enumerate() {
        o.z = i as i32;
    }
}

fn kind_label(o: &AnnotationObject) -> &'static str {
    match &o.kind {
        AnnotationKind::Bubble(_) => "吹き出し",
        AnnotationKind::Text(_) => "テキスト",
        AnnotationKind::MessageWindow(_) => "ウィンドウ",
        AnnotationKind::Stamp(_) => "スタンプ",
    }
}

fn to_c32(c: Rgba) -> egui::Color32 {
    egui::Color32::from_rgba_unmultiplied(c.r, c.g, c.b, c.a)
}
fn from_c32(c: egui::Color32) -> Rgba {
    let [r, g, b, a] = c.to_srgba_unmultiplied();
    Rgba::new(r, g, b, a)
}

impl App {
    // ── モード入退場 ────────────────────────────────────────────────

    /// テキスト編集モードに入る。見開き中は左ページへ Single ピボットする
    /// (消しゴム / 隠蔽の enter と同じ作法)。`comic_docs` に作業セットをロードする。
    pub(crate) fn enter_text_mode(&mut self, fs_idx: usize) {
        let spread_pair = match self.resolve_spread_pair(fs_idx) {
            SpreadPair::Double { left, right } => Some((left, right)),
            SpreadPair::Single => None,
        };
        let target_idx = spread_pair.map(|(l, _)| l).unwrap_or(fs_idx);

        let Some(key) = self.page_path_key(target_idx) else {
            return;
        };

        if let Some(pair) = spread_pair {
            self.text_spread_ctx = Some(crate::app::EraseSpreadCtx {
                saved_mode: self.spread_mode,
                pair,
            });
            self.spread_mode = crate::settings::SpreadMode::Single;
            self.fullscreen_idx = Some(target_idx);
            self.fs_zoom = 1.0;
            self.fs_pan = egui::Vec2::ZERO;
        }

        self.text_mode = true;
        self.text_selected = None;
        self.text_drag = None;
        self.clear_meta_undo();
        self.ensure_comic_doc_loaded(&key);

        let obj_count = self.comic_docs.get(&key).map(Vec::len).unwrap_or(0);
        crate::logger::log(format!("text: enter mode, key={key}, objects={obj_count}"));
    }

    /// テキスト編集モードを抜ける。作業セットを comic.db + サイドカーへ保存してから
    /// 状態をクリアし、見開きから入っていた場合は spread を復元する。
    pub(crate) fn reset_text_mode(&mut self) {
        let restore_idx = self.fullscreen_idx;
        let was_text_mode = self.text_mode;

        if was_text_mode {
            if let Some(idx) = restore_idx {
                if let Some(key) = self.page_path_key(idx) {
                    let objs = self.comic_docs.get(&key).cloned().unwrap_or_default();
                    self.save_comic_objects(idx, &key, &objs);
                }
            }
        }

        self.text_mode = false;
        self.text_selected = None;
        self.text_drag = None;
        if was_text_mode {
            self.clear_meta_undo();
        }

        if let Some(ctx) = self.text_spread_ctx.take() {
            self.spread_mode = ctx.saved_mode;
            self.fullscreen_idx = Some(ctx.pair.0);
            self.fs_zoom = 1.0;
            self.fs_pan = egui::Vec2::ZERO;
        }
        crate::logger::log("text: reset mode".to_string());
    }

    // ── キー入力 ────────────────────────────────────────────────────

    /// テキストモード中のキー処理。`ui_fullscreen` のキーハンドラ冒頭で
    /// `if self.text_mode { return self.handle_text_keys(...) }` として委譲される。
    ///
    /// IME 安全性: Escape は `dialog_escape_pressed` (IME 変換中は false) で判定し、
    /// テキストフィールドにフォーカスがある間は egui に委ねて (= モード退場しない) 変換
    /// 確定 / キャンセルを壊さない。Ctrl+T は修飾キー付きなので IME と衝突しない。
    pub(crate) fn handle_text_keys(&mut self, ctx: &egui::Context, _fs_idx: usize) -> FsKeyAction {
        let action = FsKeyAction::default();

        // Ctrl+T: 再押下で退場。
        let ctrl_t = ctx.input_mut(|i| i.consume_key(egui::Modifiers::CTRL, egui::Key::T));
        if ctrl_t {
            self.reset_text_mode();
            return action;
        }

        // Delete / Backspace: 選択オブジェクトを削除 (テキストフィールド非フォーカス時のみ)。
        let editing_text = ctx.memory(|m| m.focused().is_some());
        if !editing_text {
            let del = ctx.input_mut(|i| {
                i.consume_key(egui::Modifiers::NONE, egui::Key::Delete)
                    || i.consume_key(egui::Modifiers::NONE, egui::Key::Backspace)
            });
            if del {
                self.delete_selected_text_object();
                return action;
            }
        }

        // Escape: フォーカス中はフィールド defocus を egui に委ねる (消費しない)。
        // 非フォーカス時のみ選択解除 → モード退場。
        let esc = self.dialog_escape_pressed(ctx);
        if esc && !editing_text {
            ctx.input_mut(|i| {
                let _ = i.consume_key(egui::Modifiers::NONE, egui::Key::Escape);
            });
            if self.text_selected.is_some() {
                self.text_selected = None;
                return action;
            }
            self.reset_text_mode();
            return action;
        }

        action
    }

    /// 選択中オブジェクトを削除して z を正規化、comic を再ベイク + 保存。
    fn delete_selected_text_object(&mut self) {
        let Some(id) = self.text_selected else {
            return;
        };
        let Some(fs_idx) = self.fullscreen_idx else {
            return;
        };
        let Some(key) = self.page_path_key(fs_idx) else {
            return;
        };
        let Some(objs) = self.comic_docs.get_mut(&key) else {
            return;
        };
        let before = objs.len();
        objs.retain(|o| o.id != id);
        if objs.len() == before {
            return;
        }
        normalize_z(objs);
        let snapshot = objs.clone();
        self.text_selected = None;
        self.save_comic_objects(fs_idx, &key, &snapshot);
        self.mark_comic_dirty();
    }

    // ── 座標 view ────────────────────────────────────────────────────

    /// 現在表示中ページの座標 view を作る。ソース寸法が取れない (未ロード等) なら None。
    pub(crate) fn text_img_view(
        &mut self,
        idx: usize,
        image_rect: egui::Rect,
        zoom_pan: Option<(f32, egui::Vec2)>,
    ) -> Option<TextImgView> {
        let (sw, sh) = self.source_dims_for_idx(idx)?;
        let rotation = self.get_rotation(idx);
        let free_rot = self.fs_free_rotation;
        let display_size = match rotation {
            crate::rotation_db::Rotation::Cw90 | crate::rotation_db::Rotation::Cw270 => {
                egui::vec2(sh, sw)
            }
            _ => egui::vec2(sw, sh),
        };
        let fit = (image_rect.width() / display_size.x).min(image_rect.height() / display_size.y);
        if !fit.is_finite() || fit <= 0.0 {
            return None;
        }
        let (total_scale, center) = match zoom_pan {
            Some((zoom, pan)) => (fit * zoom, image_rect.center() + pan),
            None => (fit, image_rect.center()),
        };
        let img_rect = egui::Rect::from_center_size(center, display_size * total_scale);
        Some(TextImgView {
            img_rect,
            center,
            rotation,
            free_rot,
            sw,
            sh,
        })
    }

    // ── キャンバス入力 (選択 + ドラッグ移動) ─────────────────────────

    /// キャンバスのポインタ入力を処理する。`ui_fullscreen` の描画シーケンスから
    /// `draw_text_overlay` の前に毎フレーム呼ばれる。
    pub(crate) fn handle_text_canvas_input(
        &mut self,
        ctx: &egui::Context,
        image_rect: egui::Rect,
        zoom_pan: Option<(f32, egui::Vec2)>,
    ) {
        let Some(fs_idx) = self.fullscreen_idx else {
            return;
        };
        let Some(key) = self.page_path_key(fs_idx) else {
            return;
        };
        let Some(view) = self.text_img_view(fs_idx, image_rect, zoom_pan) else {
            return;
        };
        let panel_rect = self.text_panel_rect(image_rect);
        let fonts = self.ensure_comic_fonts();

        let (pressed, down, released, pos) = ctx.input(|i| {
            (
                i.pointer.primary_pressed(),
                i.pointer.primary_down(),
                i.pointer.primary_released(),
                i.pointer.interact_pos(),
            )
        });

        if pressed {
            if let Some(pos) = pos {
                if panel_rect.contains(pos) {
                    return; // パネル上のクリックはキャンバス操作にしない
                }
                let img = view.screen_to_image(pos);
                let hit = self
                    .comic_docs
                    .get(&key)
                    .and_then(|objs| hit_test(objs, img, fonts.as_deref()));
                self.text_selected = hit;
                self.text_drag = hit.map(|id| TextDrag {
                    id,
                    last_img: img,
                    moved: false,
                });
            }
        } else if down {
            if let (Some(pos), Some(mut drag)) = (pos, self.text_drag) {
                let img = view.screen_to_image(pos);
                let dx = img.0 - drag.last_img.0;
                let dy = img.1 - drag.last_img.1;
                if dx != 0.0 || dy != 0.0 {
                    if let Some(objs) = self.comic_docs.get_mut(&key) {
                        if let Some(o) = objs.iter_mut().find(|o| o.id == drag.id) {
                            translate_object(o, dx, dy);
                            drag.moved = true;
                        }
                    }
                    drag.last_img = img;
                    self.mark_comic_dirty();
                }
                self.text_drag = Some(drag);
            }
        }

        if released {
            if let Some(drag) = self.text_drag.take() {
                if drag.moved {
                    // 移動確定で comic.db + サイドカーへ保存 (退場時保存に加えて即時永続化)。
                    let objs = self.comic_docs.get(&key).cloned().unwrap_or_default();
                    self.save_comic_objects(fs_idx, &key, &objs);
                }
            }
        }
    }

    // ── オーバーレイ描画 ──────────────────────────────────────────────

    /// テキストモードのパネル領域 (クリック吸収判定用)。
    pub(crate) fn text_panel_rect(&self, image_rect: egui::Rect) -> egui::Rect {
        let pos = egui::pos2(
            image_rect.min.x + PANEL_MARGIN_X,
            image_rect.min.y + PANEL_MARGIN_Y,
        );
        let h = (image_rect.height() - PANEL_MARGIN_Y - 24.0).clamp(220.0, 760.0);
        egui::Rect::from_min_size(pos, egui::vec2(PANEL_W + 16.0, h))
    }

    /// テキストモードのオーバーレイ描画 (選択枠 + パネル)。
    pub(crate) fn draw_text_overlay(
        &mut self,
        ui: &mut egui::Ui,
        ctx: &egui::Context,
        image_rect: egui::Rect,
        zoom_pan: Option<(f32, egui::Vec2)>,
    ) {
        self.draw_text_selection(ui, image_rect, zoom_pan);
        self.draw_text_panel(ctx, image_rect);
    }

    /// 選択中オブジェクトの境界枠を画面に描く (回転を反映した四角形)。
    fn draw_text_selection(
        &mut self,
        ui: &mut egui::Ui,
        image_rect: egui::Rect,
        zoom_pan: Option<(f32, egui::Vec2)>,
    ) {
        let Some(fs_idx) = self.fullscreen_idx else {
            return;
        };
        let Some(id) = self.text_selected else {
            return;
        };
        let Some(key) = self.page_path_key(fs_idx) else {
            return;
        };
        let Some(view) = self.text_img_view(fs_idx, image_rect, zoom_pan) else {
            return;
        };
        let fonts = self.ensure_comic_fonts();
        let bounds = self
            .comic_docs
            .get(&key)
            .and_then(|objs| objs.iter().find(|o| o.id == id))
            .map(|o| object_bounds(o, fonts.as_deref()));
        let Some(b) = bounds else {
            return;
        };
        let corners = [
            b.left_top(),
            b.right_top(),
            b.right_bottom(),
            b.left_bottom(),
        ];
        let pts: Vec<egui::Pos2> = corners
            .iter()
            .map(|c| view.image_to_screen(c.x, c.y))
            .collect();
        let painter = ui.painter().with_clip_rect(image_rect);
        let stroke = egui::Stroke::new(2.0, sel_color());
        for i in 0..4 {
            painter.line_segment([pts[i], pts[(i + 1) % 4]], stroke);
        }
        for p in &pts {
            painter.rect_filled(
                egui::Rect::from_center_size(*p, egui::vec2(8.0, 8.0)),
                1.0,
                sel_color(),
            );
        }
    }

    /// テキストモードのパネル。`egui::Area` + `Frame::popup` + クリック吸収 sink。
    /// オブジェクト追加 / 一覧 / 選択中オブジェクトの種別別編集を表示する。
    fn draw_text_panel(&mut self, ctx: &egui::Context, image_rect: egui::Rect) {
        if !self.text_mode {
            return;
        }
        let Some(fs_idx) = self.fullscreen_idx else {
            return;
        };
        let Some(key) = self.page_path_key(fs_idx) else {
            return;
        };
        let (sw, sh) = self.source_dims_for_idx(fs_idx).unwrap_or((1000.0, 1000.0));
        let font_key = crate::comic_overlay::COMIC_FONT_KEY.to_string();

        let panel_pos = egui::pos2(
            image_rect.min.x + PANEL_MARGIN_X,
            image_rect.min.y + PANEL_MARGIN_Y,
        );
        let sink_rect = self.text_panel_rect(image_rect);
        let body_height = (image_rect.height() - PANEL_MARGIN_Y - 48.0).clamp(200.0, 720.0);

        // 借用衝突を避けるため作業セットを一旦取り出し、ローカルだけを編集する。
        let mut objects = self.comic_docs.remove(&key).unwrap_or_default();
        let mut selected = self.text_selected;
        let mut changed = false;
        let mut close = false;

        egui::Area::new(egui::Id::new("text_panel"))
            .fixed_pos(panel_pos)
            .order(egui::Order::Foreground)
            .interactable(true)
            .show(ctx, |ui| {
                ui.interact(
                    sink_rect,
                    egui::Id::new("text_panel_click_sink"),
                    egui::Sense::click_and_drag(),
                );
                egui::Frame::popup(ui.style())
                    .fill(egui::Color32::from_rgba_unmultiplied(20, 20, 20, 235))
                    .stroke(egui::Stroke::new(
                        1.0,
                        egui::Color32::from_rgba_unmultiplied(255, 255, 255, 40),
                    ))
                    .corner_radius(6.0)
                    .show(ui, |ui| {
                        ui.set_min_width(PANEL_W);
                        ui.set_max_width(PANEL_W);
                        ui.horizontal(|ui| {
                            ui.strong("テキスト注釈");
                            ui.with_layout(
                                egui::Layout::right_to_left(egui::Align::Center),
                                |ui| {
                                    // × は U+00D7 (multiplication sign、Yu Gothic に存在)。
                                    if ui.button("×").on_hover_text("閉じる (Esc)").clicked() {
                                        close = true;
                                    }
                                },
                            );
                        });
                        ui.separator();

                        // ── 追加 ──
                        ui.horizontal(|ui| {
                            ui.label("追加:");
                            if ui.button("テキスト").clicked() {
                                let id = next_id(&objects);
                                let z = objects.len() as i32;
                                let tb = TextBlock {
                                    text: "テキスト".to_string(),
                                    size_px: (sh * 0.04).clamp(24.0, 96.0),
                                    color: Rgba::BLACK,
                                    outline: Some(StrokeStyle {
                                        color: Rgba::WHITE,
                                        width_px: 4.0,
                                    }),
                                    font_key: font_key.clone(),
                                    ..TextBlock::default()
                                };
                                let mut o =
                                    AnnotationObject::new_text(id, (sw * 0.3, sh * 0.3), tb);
                                o.z = z;
                                objects.push(o);
                                selected = Some(id);
                                changed = true;
                            }
                            if ui.button("吹き出し").clicked() {
                                let id = next_id(&objects);
                                let z = objects.len() as i32;
                                let mut b = BubbleObject::default();
                                b.text.text = "セリフ".to_string();
                                b.text.font_key = font_key.clone();
                                b.text.size_px = (sh * 0.035).clamp(22.0, 80.0);
                                let mut o =
                                    AnnotationObject::new_bubble(id, (sw * 0.5, sh * 0.5), b);
                                o.z = z;
                                objects.push(o);
                                selected = Some(id);
                                changed = true;
                            }
                            if ui.button("ウィンドウ").clicked() {
                                let id = next_id(&objects);
                                let z = objects.len() as i32;
                                let mut w = MessageWindowObject {
                                    position: WindowPosition::Free,
                                    half_w: (sw * 0.4).max(120.0),
                                    half_h: (sh * 0.12).max(60.0),
                                    ..MessageWindowObject::default()
                                };
                                w.text.text = "本文".to_string();
                                w.text.font_key = font_key.clone();
                                w.text.size_px = (sh * 0.03).clamp(20.0, 64.0);
                                let mut o = AnnotationObject::new_message_window(
                                    id,
                                    (sw * 0.5, sh * 0.8),
                                    w,
                                );
                                o.z = z;
                                objects.push(o);
                                selected = Some(id);
                                changed = true;
                            }
                        });
                        ui.separator();

                        ui.allocate_ui_with_layout(
                            egui::vec2(PANEL_W, body_height),
                            egui::Layout::top_down(egui::Align::Min),
                            |ui| {
                                egui::ScrollArea::vertical()
                                    .id_salt("text_panel_scroll")
                                    .max_height(body_height)
                                    .auto_shrink([false, false])
                                    .show(ui, |ui| {
                                        object_list_ui(
                                            ui,
                                            &mut objects,
                                            &mut selected,
                                            &mut changed,
                                        );
                                        ui.separator();
                                        if let Some(id) = selected {
                                            if let Some(o) = objects.iter_mut().find(|o| o.id == id)
                                            {
                                                edit_object_ui(ui, o, &mut changed);
                                            }
                                        } else {
                                            ui.label(
                                                egui::RichText::new(
                                                    "オブジェクトをクリックで選択 / ドラッグで移動",
                                                )
                                                .small()
                                                .color(egui::Color32::from_gray(170)),
                                            );
                                        }
                                    });
                            },
                        );
                    });
            });

        // 書き戻し。
        self.text_selected = selected;
        if changed {
            self.save_comic_objects(fs_idx, &key, &objects);
            self.mark_comic_dirty();
        } else {
            self.comic_docs.insert(key, objects);
        }
        if close {
            self.reset_text_mode();
        }
    }
}

// ── パネル UI ヘルパー (self を借りない純 UI 関数) ──────────────────────

/// オブジェクト一覧 (選択 / 前後 / 複製 / 表示トグル / 削除)。
fn object_list_ui(
    ui: &mut egui::Ui,
    objects: &mut Vec<AnnotationObject>,
    selected: &mut Option<u64>,
    changed: &mut bool,
) {
    ui.label(egui::RichText::new(format!("オブジェクト ({})", objects.len())).small());
    let n = objects.len();
    let mut move_up: Option<usize> = None;
    let mut move_down: Option<usize> = None;
    let mut duplicate: Option<usize> = None;
    let mut delete: Option<usize> = None;

    for i in 0..n {
        let id = objects[i].id;
        let label = format!("{}: {}", i + 1, kind_label(&objects[i]));
        ui.horizontal(|ui| {
            let mut enabled = objects[i].enabled;
            if ui
                .checkbox(&mut enabled, "")
                .on_hover_text("表示")
                .changed()
            {
                objects[i].enabled = enabled;
                *changed = true;
            }
            if ui.selectable_label(*selected == Some(id), label).clicked() {
                *selected = Some(id);
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.small_button("×").on_hover_text("削除").clicked() {
                    delete = Some(i);
                }
                if ui.small_button("複製").on_hover_text("複製").clicked() {
                    duplicate = Some(i);
                }
                if ui.small_button("↓").on_hover_text("背面へ").clicked() {
                    move_down = Some(i);
                }
                if ui.small_button("↑").on_hover_text("前面へ").clicked() {
                    move_up = Some(i);
                }
            });
        });
    }

    if let Some(i) = move_up {
        if i + 1 < objects.len() {
            objects.swap(i, i + 1);
            normalize_z(objects);
            *changed = true;
        }
    }
    if let Some(i) = move_down {
        if i > 0 {
            objects.swap(i, i - 1);
            normalize_z(objects);
            *changed = true;
        }
    }
    if let Some(i) = duplicate {
        if i < objects.len() {
            let mut dup = objects[i].clone();
            dup.id = next_id(objects);
            dup.pivot.0 += 24.0;
            dup.pivot.1 += 24.0;
            if let AnnotationKind::Bubble(b) = &mut dup.kind {
                if let Some(t) = &mut b.tail {
                    t.tip.0 += 24.0;
                    t.tip.1 += 24.0;
                }
            }
            *selected = Some(dup.id);
            objects.push(dup);
            normalize_z(objects);
            *changed = true;
        }
    }
    if let Some(i) = delete {
        if i < objects.len() {
            let removed = objects.remove(i);
            if *selected == Some(removed.id) {
                *selected = None;
            }
            normalize_z(objects);
            *changed = true;
        }
    }
}

/// 選択中オブジェクトの種別別インライン編集。
fn edit_object_ui(ui: &mut egui::Ui, o: &mut AnnotationObject, changed: &mut bool) {
    ui.strong(kind_label(o));
    // 回転 (全種共通)。
    let mut deg = o.rotation_rad.to_degrees();
    if ui
        .add(egui::Slider::new(&mut deg, -180.0..=180.0).text("回転°"))
        .changed()
    {
        o.rotation_rad = deg.to_radians();
        *changed = true;
    }

    match &mut o.kind {
        AnnotationKind::Text(t) => {
            text_block_ui(ui, t, changed, true);
        }
        AnnotationKind::Bubble(b) => {
            bubble_ui(ui, b, changed);
        }
        AnnotationKind::MessageWindow(w) => {
            window_ui(ui, w, changed);
        }
        AnnotationKind::Stamp(s) => {
            stamp_ui(ui, s, changed);
        }
    }
}

/// TextBlock の編集 (テキスト内容・サイズ・色・向き・整列・袋文字・太字/斜体)。
/// `with_text` が false のときは本文編集欄を出さない (名前プレート等の流用余地)。
fn text_block_ui(ui: &mut egui::Ui, t: &mut TextBlock, changed: &mut bool, with_text: bool) {
    if with_text {
        let resp = ui.add(
            egui::TextEdit::multiline(&mut t.text)
                .desired_rows(2)
                .desired_width(PANEL_W - 16.0)
                .hint_text("テキスト"),
        );
        if resp.changed() {
            *changed = true;
        }
    }
    if ui
        .add(egui::Slider::new(&mut t.size_px, 8.0..=400.0).text("サイズ"))
        .changed()
    {
        *changed = true;
    }
    ui.horizontal(|ui| {
        ui.label("色");
        let mut col = to_c32(t.color);
        if ui.color_edit_button_srgba(&mut col).changed() {
            t.color = from_c32(col);
            *changed = true;
        }
        let mut bold = t.bold;
        if ui.checkbox(&mut bold, "太").changed() {
            t.bold = bold;
            *changed = true;
        }
        let mut italic = t.italic;
        if ui.checkbox(&mut italic, "斜").changed() {
            t.italic = italic;
            *changed = true;
        }
    });
    ui.horizontal(|ui| {
        ui.label("向き");
        let mut o = t.orientation;
        if ui
            .selectable_label(o == Orientation::Horizontal, "横")
            .clicked()
        {
            o = Orientation::Horizontal;
        }
        if ui
            .selectable_label(o == Orientation::Vertical, "縦")
            .clicked()
        {
            o = Orientation::Vertical;
        }
        if o != t.orientation {
            t.orientation = o;
            *changed = true;
        }
        ui.separator();
        ui.label("整列");
        let mut a = t.align;
        for (val, lbl) in [
            (TextAlign::Start, "始"),
            (TextAlign::Center, "中"),
            (TextAlign::End, "終"),
        ] {
            if ui.selectable_label(a == val, lbl).clicked() {
                a = val;
            }
        }
        if a != t.align {
            t.align = a;
            *changed = true;
        }
    });
    // 袋文字。
    let mut has_outline = t.outline.is_some();
    if ui.checkbox(&mut has_outline, "袋文字").changed() {
        t.outline = if has_outline {
            Some(StrokeStyle {
                color: Rgba::WHITE,
                width_px: 4.0,
            })
        } else {
            None
        };
        *changed = true;
    }
    if let Some(o) = &mut t.outline {
        ui.horizontal(|ui| {
            ui.label("縁色");
            let mut col = to_c32(o.color);
            if ui.color_edit_button_srgba(&mut col).changed() {
                o.color = from_c32(col);
                *changed = true;
            }
            if ui
                .add(egui::Slider::new(&mut o.width_px, 0.0..=30.0).text("太さ"))
                .changed()
            {
                *changed = true;
            }
        });
    }
    let mut tcy = t.auto_tcy;
    if ui
        .checkbox(&mut tcy, "自動縦中横")
        .on_hover_text("縦書きで数字 2-3 桁や !? を横向きに組む")
        .changed()
    {
        t.auto_tcy = tcy;
        *changed = true;
    }
}

/// 吹き出しの編集 (形状・塗り・輪郭・自動サイズ・しっぽ・本文)。
fn bubble_ui(ui: &mut egui::Ui, b: &mut BubbleObject, changed: &mut bool) {
    // 形状コンボ。
    let cur = ShapeKind::from_shape(&b.shape);
    let (hw, hh) = shape_half(&b.shape);
    let mut next = cur;
    egui::ComboBox::from_label("形状")
        .selected_text(cur.label())
        .show_ui(ui, |ui| {
            for k in ShapeKind::ALL {
                ui.selectable_value(&mut next, k, k.label());
            }
        });
    if next != cur {
        b.shape = next.to_shape(hw, hh);
        b.shape_preset_link = None;
        *changed = true;
    }

    // 塗り。
    let mut has_fill = b.fill.is_some();
    if ui.checkbox(&mut has_fill, "塗り").changed() {
        b.fill = if has_fill { Some(Rgba::WHITE) } else { None };
        *changed = true;
    }
    if let Some(f) = &mut b.fill {
        ui.horizontal(|ui| {
            ui.label("塗り色");
            let mut col = to_c32(*f);
            if ui.color_edit_button_srgba(&mut col).changed() {
                *f = from_c32(col);
                *changed = true;
            }
            if ui
                .add(egui::Slider::new(&mut b.fill_opacity, 0.0..=1.0).text("不透明"))
                .changed()
            {
                *changed = true;
            }
        });
    }
    // 輪郭。
    ui.horizontal(|ui| {
        ui.label("線色");
        let mut col = to_c32(b.outline.color);
        if ui.color_edit_button_srgba(&mut col).changed() {
            b.outline.color = from_c32(col);
            *changed = true;
        }
        if ui
            .add(egui::Slider::new(&mut b.outline.width_px, 0.0..=30.0).text("線幅"))
            .changed()
        {
            *changed = true;
        }
    });
    let mut auto = b.auto_size;
    if ui
        .checkbox(&mut auto, "自動サイズ")
        .on_hover_text("文字に合わせて形状サイズを決める")
        .changed()
    {
        b.auto_size = auto;
        *changed = true;
    }
    if ui
        .add(egui::Slider::new(&mut b.padding_px, 0.0..=120.0).text("余白"))
        .changed()
    {
        *changed = true;
    }
    // しっぽ。
    let mut has_tail = b.tail.is_some();
    if ui.checkbox(&mut has_tail, "しっぽ").changed() {
        b.tail = if has_tail {
            Some(comic_core::Tail::default())
        } else {
            None
        };
        *changed = true;
    }
    if let Some(t) = &mut b.tail {
        ui.horizontal(|ui| {
            ui.label("種別");
            let mut k = t.kind;
            if ui.selectable_label(k == TailKind::Spike, "会話").clicked() {
                k = TailKind::Spike;
            }
            if ui
                .selectable_label(k == TailKind::Thought, "思考")
                .clicked()
            {
                k = TailKind::Thought;
            }
            if k != t.kind {
                t.kind = k;
                *changed = true;
            }
            if ui
                .add(egui::Slider::new(&mut t.width_px, 4.0..=120.0).text("幅"))
                .changed()
            {
                *changed = true;
            }
        });
    }
    ui.separator();
    ui.label("本文");
    text_block_ui(ui, &mut b.text, changed, true);
}

/// メッセージウィンドウの編集 (枠・塗り・配置・本文)。
fn window_ui(ui: &mut egui::Ui, w: &mut MessageWindowObject, changed: &mut bool) {
    // 枠。
    let mut frame = w.frame;
    egui::ComboBox::from_label("枠")
        .selected_text(frame_label(frame))
        .show_ui(ui, |ui| {
            for f in [
                FrameStyle::None,
                FrameStyle::SolidRounded,
                FrameStyle::DoubleLine,
            ] {
                ui.selectable_value(&mut frame, f, frame_label(f));
            }
        });
    if frame != w.frame {
        w.frame = frame;
        *changed = true;
    }
    // 塗りモード。
    let mut fm = w.fill_mode;
    egui::ComboBox::from_label("塗り")
        .selected_text(fill_label(fm))
        .show_ui(ui, |ui| {
            for f in [
                FillMode::None,
                FillMode::Solid,
                FillMode::Translucent,
                FillMode::GradientScrim,
                FillMode::LinearGradient,
            ] {
                ui.selectable_value(&mut fm, f, fill_label(f));
            }
        });
    if fm != w.fill_mode {
        w.fill_mode = fm;
        *changed = true;
    }
    if let Some(f) = &mut w.fill {
        ui.horizontal(|ui| {
            ui.label("塗り色");
            let mut col = to_c32(*f);
            if ui.color_edit_button_srgba(&mut col).changed() {
                *f = from_c32(col);
                *changed = true;
            }
        });
    }
    ui.horizontal(|ui| {
        ui.label("線色");
        let mut col = to_c32(w.outline.color);
        if ui.color_edit_button_srgba(&mut col).changed() {
            w.outline.color = from_c32(col);
            *changed = true;
        }
        if ui
            .add(egui::Slider::new(&mut w.outline.width_px, 0.0..=12.0).text("線幅"))
            .changed()
        {
            *changed = true;
        }
    });
    ui.separator();
    ui.label("本文");
    text_block_ui(ui, &mut w.text, changed, true);
}

/// スタンプの編集 (ピッカーは Inc 4c。ここでは不透明度 / 反転のみ)。
fn stamp_ui(ui: &mut egui::Ui, s: &mut StampObject, changed: &mut bool) {
    ui.label(
        egui::RichText::new("スタンプ画像の選択は今後のバージョンで対応します")
            .small()
            .color(egui::Color32::from_gray(160)),
    );
    if ui
        .add(egui::Slider::new(&mut s.opacity, 0.0..=1.0).text("不透明"))
        .changed()
    {
        *changed = true;
    }
    ui.horizontal(|ui| {
        let mut fh = s.flip_h;
        if ui.checkbox(&mut fh, "左右反転").changed() {
            s.flip_h = fh;
            *changed = true;
        }
        let mut fv = s.flip_v;
        if ui.checkbox(&mut fv, "上下反転").changed() {
            s.flip_v = fv;
            *changed = true;
        }
    });
}

fn frame_label(f: FrameStyle) -> &'static str {
    match f {
        FrameStyle::None => "なし",
        FrameStyle::SolidRounded => "角丸",
        FrameStyle::DoubleLine => "二重線",
    }
}

fn fill_label(f: FillMode) -> &'static str {
    match f {
        FillMode::None => "なし",
        FillMode::Solid => "ベタ",
        FillMode::Translucent => "半透明",
        FillMode::GradientScrim => "グラデ(スクリム)",
        FillMode::LinearGradient => "グラデ(2色)",
    }
}

// ── 吹き出し形状の種別 (コンボ用) ────────────────────────────────────────

#[derive(Clone, Copy, PartialEq, Eq)]
enum ShapeKind {
    Ellipse,
    RoundRect,
    Burst,
    Cloud,
    Polygon,
    Diamond,
    Heart,
    Arrow,
    Soft,
    MotionLines,
    SpeedLines,
    Concentration,
    Strokes,
    DoubleStroke,
    TextOnly,
}

impl ShapeKind {
    const ALL: [ShapeKind; 15] = [
        ShapeKind::Ellipse,
        ShapeKind::RoundRect,
        ShapeKind::Burst,
        ShapeKind::Cloud,
        ShapeKind::Polygon,
        ShapeKind::Diamond,
        ShapeKind::Heart,
        ShapeKind::Arrow,
        ShapeKind::Soft,
        ShapeKind::MotionLines,
        ShapeKind::SpeedLines,
        ShapeKind::Concentration,
        ShapeKind::Strokes,
        ShapeKind::DoubleStroke,
        ShapeKind::TextOnly,
    ];

    fn label(self) -> &'static str {
        match self {
            ShapeKind::Ellipse => "楕円",
            ShapeKind::RoundRect => "角丸四角",
            ShapeKind::Burst => "爆発",
            ShapeKind::Cloud => "雲(思考)",
            ShapeKind::Polygon => "多角形",
            ShapeKind::Diamond => "ダイヤ",
            ShapeKind::Heart => "ハート",
            ShapeKind::Arrow => "矢印",
            ShapeKind::Soft => "やわらか",
            ShapeKind::MotionLines => "集中線",
            ShapeKind::SpeedLines => "流線",
            ShapeKind::Concentration => "意識",
            ShapeKind::Strokes => "線",
            ShapeKind::DoubleStroke => "二重線",
            ShapeKind::TextOnly => "なし",
        }
    }

    fn from_shape(s: &BubbleShape) -> ShapeKind {
        match s {
            BubbleShape::Ellipse { .. } => ShapeKind::Ellipse,
            BubbleShape::RoundRect { .. } => ShapeKind::RoundRect,
            BubbleShape::Burst { .. } => ShapeKind::Burst,
            BubbleShape::Cloud { .. } => ShapeKind::Cloud,
            BubbleShape::Polygon { .. } => ShapeKind::Polygon,
            BubbleShape::Diamond { .. } => ShapeKind::Diamond,
            BubbleShape::Heart { .. } => ShapeKind::Heart,
            BubbleShape::Arrow { .. } => ShapeKind::Arrow,
            BubbleShape::Soft { .. } => ShapeKind::Soft,
            BubbleShape::MotionLines { .. } => ShapeKind::MotionLines,
            BubbleShape::SpeedLines { .. } => ShapeKind::SpeedLines,
            BubbleShape::Concentration { .. } => ShapeKind::Concentration,
            BubbleShape::Strokes { .. } => ShapeKind::Strokes,
            BubbleShape::DoubleStroke { .. } => ShapeKind::DoubleStroke,
            BubbleShape::TextOnly { .. } => ShapeKind::TextOnly,
        }
    }

    /// half 範囲 `(hw, hh)` を保ったまま当該形状を構築する (パラメータは既定値)。
    fn to_shape(self, hw: f32, hh: f32) -> BubbleShape {
        match self {
            ShapeKind::Ellipse => BubbleShape::Ellipse { rx: hw, ry: hh },
            ShapeKind::RoundRect => BubbleShape::RoundRect {
                half_w: hw,
                half_h: hh,
                corner_px: 28.0,
            },
            ShapeKind::Burst => BubbleShape::Burst {
                rx: hw,
                ry: hh,
                spikes: 16,
                jag: 0.55,
                shape_seed: 0,
            },
            ShapeKind::Cloud => BubbleShape::Cloud {
                rx: hw,
                ry: hh,
                lobes: 12,
                amp: 0.28,
                shape_seed: 0,
            },
            ShapeKind::Polygon => BubbleShape::Polygon {
                rx: hw,
                ry: hh,
                sides: 6,
            },
            ShapeKind::Diamond => BubbleShape::Diamond {
                half_w: hw,
                half_h: hh,
            },
            ShapeKind::Heart => BubbleShape::Heart { rx: hw, ry: hh },
            ShapeKind::Arrow => BubbleShape::Arrow {
                half_w: hw,
                half_h: hh,
                dir_rad: -std::f32::consts::FRAC_PI_2,
            },
            ShapeKind::Soft => BubbleShape::Soft {
                half_w: hw,
                half_h: hh,
                corner_px: 28.0,
                shape_seed: 0,
            },
            ShapeKind::MotionLines => BubbleShape::MotionLines {
                rx: hw,
                ry: hh,
                count: 64,
                shape_seed: 0,
            },
            ShapeKind::SpeedLines => BubbleShape::SpeedLines {
                half_w: hw,
                half_h: hh,
                dir_rad: 0.0,
                count: 64,
                shape_seed: 0,
            },
            ShapeKind::Concentration => BubbleShape::Concentration {
                rx: hw,
                ry: hh,
                shape_seed: 0,
            },
            ShapeKind::Strokes => BubbleShape::Strokes {
                half_w: hw,
                half_h: hh,
                corner_px: 28.0,
                shape_seed: 0,
            },
            ShapeKind::DoubleStroke => BubbleShape::DoubleStroke {
                half_w: hw,
                half_h: hh,
                corner_px: 28.0,
                gap_px: 8.0,
            },
            ShapeKind::TextOnly => BubbleShape::TextOnly {
                half_w: hw,
                half_h: hh,
            },
        }
    }
}

/// 形状の half 範囲 (comic-core 内部の `shape_half_extents` と同値)。
fn shape_half(shape: &BubbleShape) -> (f32, f32) {
    match *shape {
        BubbleShape::Ellipse { rx, ry } => (rx, ry),
        BubbleShape::RoundRect { half_w, half_h, .. } => (half_w, half_h),
        BubbleShape::Burst { rx, ry, .. } => (rx, ry),
        BubbleShape::Cloud { rx, ry, .. } => (rx, ry),
        BubbleShape::Polygon { rx, ry, .. } => (rx, ry),
        BubbleShape::Diamond { half_w, half_h } => (half_w, half_h),
        BubbleShape::Heart { rx, ry } => (rx, ry),
        BubbleShape::Arrow { half_w, half_h, .. } => (half_w, half_h),
        BubbleShape::Soft { half_w, half_h, .. } => (half_w, half_h),
        BubbleShape::MotionLines { rx, ry, .. } => (rx, ry),
        BubbleShape::SpeedLines { half_w, half_h, .. } => (half_w, half_h),
        BubbleShape::TextOnly { half_w, half_h } => (half_w, half_h),
        BubbleShape::Concentration { rx, ry, .. } => (rx, ry),
        BubbleShape::Strokes { half_w, half_h, .. } => (half_w, half_h),
        BubbleShape::DoubleStroke { half_w, half_h, .. } => (half_w, half_h),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rotation_db::Rotation;

    #[test]
    fn uv_inverse_round_trips() {
        for rot in [
            Rotation::None,
            Rotation::Cw90,
            Rotation::Cw180,
            Rotation::Cw270,
        ] {
            for &(s, t) in &[(0.0, 0.0), (1.0, 0.0), (0.0, 1.0), (0.3, 0.7), (0.9, 0.2)] {
                let (u, v) = forward_uv(rot, s, t);
                let (s2, t2) = inverse_uv(rot, u, v);
                assert!(
                    (s - s2).abs() < 1e-5 && (t - t2).abs() < 1e-5,
                    "{rot:?}: ({s},{t}) -> ({s2},{t2})"
                );
            }
        }
    }

    fn make_view(rot: Rotation, free: f32) -> TextImgView {
        // source 400x300 を 800x800 の画面矩形へ fit。
        let (sw, sh) = (400.0_f32, 300.0_f32);
        let image_rect = egui::Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(800.0, 800.0));
        let display = match rot {
            Rotation::Cw90 | Rotation::Cw270 => egui::vec2(sh, sw),
            _ => egui::vec2(sw, sh),
        };
        let fit = (image_rect.width() / display.x).min(image_rect.height() / display.y);
        let center = image_rect.center();
        let img_rect = egui::Rect::from_center_size(center, display * fit);
        TextImgView {
            img_rect,
            center,
            rotation: rot,
            free_rot: free,
            sw,
            sh,
        }
    }

    #[test]
    fn screen_image_round_trips_all_rotations() {
        for rot in [
            Rotation::None,
            Rotation::Cw90,
            Rotation::Cw180,
            Rotation::Cw270,
        ] {
            for free in [0.0_f32, 0.3, -0.7] {
                let v = make_view(rot, free);
                for &(px, py) in &[(0.0, 0.0), (400.0, 300.0), (123.0, 77.0), (399.0, 1.0)] {
                    let s = v.image_to_screen(px, py);
                    let (px2, py2) = v.screen_to_image(s);
                    assert!(
                        (px - px2).abs() < 1e-2 && (py - py2).abs() < 1e-2,
                        "{rot:?} free={free}: ({px},{py}) -> ({px2},{py2})"
                    );
                }
            }
        }
    }

    #[test]
    fn cw90_maps_source_topleft_to_screen_topright() {
        // Cw90: canonical (0,0) は画面上画像矩形の右上に来る
        // (draw_rotated_image_ex の UV [LT→(0,1) … RT→(0,0)] と一致)。
        let v = make_view(Rotation::Cw90, 0.0);
        let s = v.image_to_screen(0.0, 0.0);
        assert!((s.x - v.img_rect.right()).abs() < 1.0, "x={}", s.x);
        assert!((s.y - v.img_rect.top()).abs() < 1.0, "y={}", s.y);
    }

    #[test]
    fn hit_test_picks_topmost_then_misses() {
        let mk = |id: u64, z: i32, px: f32, py: f32| {
            let mut o = AnnotationObject::new_stamp(
                id,
                (px, py),
                StampObject {
                    half_w: 50.0,
                    half_h: 50.0,
                    ..StampObject::default()
                },
            );
            o.z = z;
            o
        };
        let objs = vec![mk(1, 0, 100.0, 100.0), mk(2, 1, 110.0, 110.0)];
        // 両方に入る点 → z 最大 (id 2)。
        assert_eq!(hit_test(&objs, (105.0, 105.0), None), Some(2));
        // obj1 のみ。
        assert_eq!(hit_test(&objs, (55.0, 55.0), None), Some(1));
        // どれにも当たらない。
        assert_eq!(hit_test(&objs, (5.0, 5.0), None), None);
    }

    #[test]
    fn translate_moves_pivot_and_tail() {
        let mut o = AnnotationObject::new_bubble(
            1,
            (100.0, 100.0),
            BubbleObject {
                tail: Some(comic_core::Tail {
                    tip: (40.0, 200.0),
                    ..comic_core::Tail::default()
                }),
                ..BubbleObject::default()
            },
        );
        translate_object(&mut o, 10.0, -5.0);
        assert_eq!(o.pivot, (110.0, 95.0));
        if let AnnotationKind::Bubble(b) = &o.kind {
            assert_eq!(b.tail.as_ref().unwrap().tip, (50.0, 195.0));
        } else {
            panic!("not a bubble");
        }
    }
}
