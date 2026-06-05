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

use crate::app::{App, TextDrag, TextDragKind};
use crate::ui_fullscreen::{FsKeyAction, SpreadPair};
use comic_core::{
    AnnotationKind, AnnotationObject, BubbleObject, BubbleShape, FillMode, FontSet, FrameStyle,
    MessageWindowObject, Orientation, Rgba, SizeMode, StampObject, StrokeStyle, Tail, TailKind,
    TextAlign, TextBlock, VAnchor, WindowPosition,
};

/// パネル幅 (編集コントロールが入るので conceal より少し広い)。
const PANEL_W: f32 = 268.0;
const PANEL_MARGIN_X: f32 = 16.0;
const PANEL_MARGIN_Y: f32 = 60.0;
/// ハンドル (回転ノブ / 四隅 / しっぽ) の当たり判定半径 (画面 px)。ラボと同値。
const HANDLE_R: f32 = 7.0;

/// 右詳細パネルのカテゴリタブ。補正レイヤーの section-accent と同じく、各カテゴリに
/// アクセント色を割り当て、タブボタン + コンテンツ左端の色帯で「カラーの縦線での分類」を
/// 与える (ラボの `PropTab` 相当。mIV は飾り未対応なので セリフ/本体/しっぽ の 3 種)。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TextPropTab {
    /// セリフ (本文テキスト): 青。
    Serifu,
    /// 本体 (吹き出し形状・塗り / ウィンドウ枠): 緑。
    Body,
    /// しっぽ (吹き出しのしっぽ): 橙。
    Tail,
}

impl TextPropTab {
    /// カテゴリのアクセント色 (タブボタンと左色帯で共有)。
    fn color(self) -> egui::Color32 {
        match self {
            TextPropTab::Serifu => egui::Color32::from_rgb(90, 170, 255), // 青
            TextPropTab::Body => egui::Color32::from_rgb(95, 208, 140),   // 緑
            TextPropTab::Tail => egui::Color32::from_rgb(255, 160, 60),   // 橙
        }
    }

    fn label(self) -> &'static str {
        match self {
            TextPropTab::Serifu => "セリフ",
            TextPropTab::Body => "本体",
            TextPropTab::Tail => "しっぽ",
        }
    }
}

/// コンテンツを左端に細いアクセント色の縦帯付きフレームで囲む (補正レイヤーの
/// `draw_panel_section` / ラボの `draw_section_bar` と同流儀)。詳細タブを
/// カテゴリ色で分類するために使う。
fn draw_section_bar<R>(
    ui: &mut egui::Ui,
    color: egui::Color32,
    add_contents: impl FnOnce(&mut egui::Ui) -> R,
) -> R {
    let response = egui::Frame::new()
        .inner_margin(egui::Margin {
            left: 10,
            right: 2,
            top: 4,
            bottom: 4,
        })
        .show(ui, |ui| add_contents(ui));
    let rect = response.response.rect;
    let line_rect = egui::Rect::from_min_max(
        egui::pos2(rect.left(), rect.top() + 4.0),
        egui::pos2(rect.left() + 3.0, rect.bottom() - 4.0),
    );
    ui.painter().rect_filled(line_rect, 1.5, color);
    ui.add_space(2.0);
    response.inner
}

/// 常時カテゴリ色を帯びる詳細タブボタン。選択 = フル彩度 + 黒文字 + 白枠、
/// 非選択 = 減光、無効 = さらに減光 + 非操作。クリックで true (無効時は常に false)。
fn prop_tab_button(
    ui: &mut egui::Ui,
    tab: TextPropTab,
    selected: bool,
    enabled: bool,
    label: &str,
) -> bool {
    let base = tab.color();
    let fill = if !enabled {
        base.gamma_multiply(0.12)
    } else if selected {
        base
    } else {
        base.gamma_multiply(0.40)
    };
    let text_col = if !enabled {
        egui::Color32::from_gray(110)
    } else if selected {
        egui::Color32::BLACK
    } else {
        egui::Color32::from_gray(235)
    };
    let mut btn = egui::Button::new(egui::RichText::new(label).color(text_col)).fill(fill);
    if selected && enabled {
        btn = btn.stroke(egui::Stroke::new(1.5, egui::Color32::WHITE));
    }
    ui.add_enabled(enabled, btn).clicked()
}

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
    /// 画像 px → 画面 px の一様スケール (= fit * zoom)。回転ノブを画面上 28px 上に
    /// 置くための画像空間オフセット (`28 / scale`) 計算に使う。
    scale: f32,
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

// ── 変形ハンドル (回転 / サイズ / しっぽ) ─ ラボ `*_handle_points` 等を移植 ─────

/// テキストのレイアウト寸法 (canonical px)。フォント未ロード時は控えめな既定。
/// ラボ `ComicLab::text_layout_size` と同式。
fn text_layout_size(t: &TextBlock, fonts: Option<&FontSet>) -> (f32, f32) {
    fonts
        .and_then(|f| f.get(&t.font_key))
        .map(|font| {
            let layout = comic_core::layout_text(t, font);
            (layout.bounds.0.max(8.0), layout.bounds.1.max(8.0))
        })
        .unwrap_or((100.0, 40.0))
}

/// オブジェクトの回転中心 (canonical px)。単独テキストは pivot をレイアウト左上に
/// 持つので中心を導出する。それ以外は pivot が視覚中心。ラボ `rotation_center` と同式。
fn rotation_center(o: &AnnotationObject, fonts: Option<&FontSet>) -> (f32, f32) {
    match &o.kind {
        AnnotationKind::Text(t) => {
            let (w, h) = text_layout_size(t, fonts);
            (o.pivot.0 + w * 0.5, o.pivot.1 + h * 0.5)
        }
        _ => o.pivot,
    }
}

/// オブジェクトの半径 (hw, hh) と回転中心 pivot を返す (ハンドル計算用)。
/// ラボ `*_handle_points` の前半と同じ寸法取り。
fn handle_half_extents(
    o: &AnnotationObject,
    fonts: Option<&FontSet>,
) -> Option<((f32, f32), (f32, f32))> {
    match &o.kind {
        AnnotationKind::Bubble(b) => {
            let fonts = fonts?;
            let (hw, hh) = match comic_core::effective_bubble_shape(b, fonts) {
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
            };
            Some(((hw, hh), o.pivot))
        }
        AnnotationKind::MessageWindow(w) => {
            let fonts = fonts?;
            let (hw, hh) = comic_core::effective_window_half_extents(w, fonts);
            Some(((hw, hh), o.pivot))
        }
        AnnotationKind::Stamp(s) => Some(((s.half_w, s.half_h), o.pivot)),
        AnnotationKind::Text(t) => {
            let (w, h) = text_layout_size(t, fonts);
            let (hw, hh) = (w * 0.5, h * 0.5);
            Some(((hw, hh), (o.pivot.0 + hw, o.pivot.1 + hh)))
        }
    }
}

/// 四隅 (TL,TR,BR,BL) と回転ノブの画像空間座標。`obj.rotation_rad` を反映する。
/// 回転ノブは上端からズーム非依存で `28px` 上 (`offset_img = 28 / scale`)。
/// ラボ `*_handle_points` と同式。
fn handle_points(
    o: &AnnotationObject,
    fonts: Option<&FontSet>,
    scale: f32,
) -> Option<([(f32, f32); 4], (f32, f32))> {
    let ((hw, hh), p) = handle_half_extents(o, fonts)?;
    let (sin, cos) = o.rotation_rad.sin_cos();
    let rot = |lx: f32, ly: f32| (p.0 + lx * cos - ly * sin, p.1 + lx * sin + ly * cos);
    let corners = [rot(-hw, -hh), rot(hw, -hh), rot(hw, hh), rot(-hw, hh)];
    let offset_img = 28.0 / scale.max(1e-3);
    let rot_handle = rot(0.0, -(hh + offset_img));
    Some((corners, rot_handle))
}

/// 吹き出しのしっぽハンドル (base, tip) の画像空間座標。回転を反映する。
/// しっぽを描かない形状 / しっぽ無しなら None。
fn tail_handle_points(
    o: &AnnotationObject,
    fonts: Option<&FontSet>,
) -> Option<((f32, f32), (f32, f32))> {
    let AnnotationKind::Bubble(b) = &o.kind else {
        return None;
    };
    let fonts = fonts?;
    let tail = b
        .tail
        .as_ref()
        .filter(|_| comic_core::shape_renders_tail(&b.shape))?;
    let eff = comic_core::effective_bubble_shape(b, fonts);
    let rot = o.rotation_rad;
    let base = rotate_about(
        comic_core::resolve_tail_base(&eff, o.pivot, tail),
        o.pivot,
        rot,
    );
    let tip = rotate_about(tail.tip, o.pivot, rot);
    Some((base, tip))
}

/// 吹き出し形状の半径を設定する (corner-resize 用)。ラボ `set_bubble_half_extents`。
fn set_bubble_half_extents(b: &mut BubbleObject, hw: f32, hh: f32) {
    match &mut b.shape {
        BubbleShape::Ellipse { rx, ry }
        | BubbleShape::Burst { rx, ry, .. }
        | BubbleShape::Cloud { rx, ry, .. }
        | BubbleShape::Polygon { rx, ry, .. }
        | BubbleShape::Heart { rx, ry }
        | BubbleShape::MotionLines { rx, ry, .. }
        | BubbleShape::Concentration { rx, ry, .. } => {
            *rx = hw;
            *ry = hh;
        }
        BubbleShape::RoundRect { half_w, half_h, .. }
        | BubbleShape::SpeedLines { half_w, half_h, .. }
        | BubbleShape::Diamond { half_w, half_h }
        | BubbleShape::Arrow { half_w, half_h, .. }
        | BubbleShape::Soft { half_w, half_h, .. }
        | BubbleShape::TextOnly { half_w, half_h }
        | BubbleShape::Strokes { half_w, half_h, .. }
        | BubbleShape::DoubleStroke { half_w, half_h, .. } => {
            *half_w = hw;
            *half_h = hh;
        }
    }
}

/// drag-start: 選択中オブジェクトのどのハンドルを掴んだかを判定する。
/// 優先順: しっぽ先端 → しっぽ根元 → 回転ノブ → 四隅。いずれも当たらなければ None
/// (呼び出し側で本体 hit-test → Move にフォールバックする)。ラボ `handle_canvas_input`
/// の drag_started 分岐と同じ優先順。
fn pick_handle(
    o: &AnnotationObject,
    ptr: egui::Pos2,
    view: &TextImgView,
    fonts: Option<&FontSet>,
) -> Option<TextDragKind> {
    let r = HANDLE_R + 4.0;
    if let Some((base, tip)) = tail_handle_points(o, fonts) {
        let tip_s = view.image_to_screen(tip.0, tip.1);
        if (tip_s - ptr).length() <= r {
            return Some(TextDragKind::TailTip);
        }
        let base_s = view.image_to_screen(base.0, base.1);
        if (base_s - ptr).length() <= r {
            return Some(TextDragKind::TailBase);
        }
    }
    if let Some((corners, roth)) = handle_points(o, fonts, view.scale) {
        let roth_s = view.image_to_screen(roth.0, roth.1);
        if (roth_s - ptr).length() <= r {
            return Some(TextDragKind::Rotate);
        }
        for (i, c) in corners.iter().enumerate() {
            let cs = view.image_to_screen(c.0, c.1);
            if (cs - ptr).length() <= r {
                return Some(TextDragKind::Corner(i));
            }
        }
    }
    None
}

/// drag-continue: ハンドル種別に応じて対象オブジェクトを変形する。`img` は現フレームの
/// ポインタ画像座標 (canonical px)。何か変化したら true。ラボ `handle_canvas_input` の
/// dragged 分岐と同じ数式。
fn apply_text_drag(
    objs: &mut [AnnotationObject],
    drag: &TextDrag,
    img: (f32, f32),
    fonts: Option<&FontSet>,
) -> bool {
    // 借用衝突を避けるため、可変借用の前に不変参照から必要値を読む。
    let rot_center = objs
        .iter()
        .find(|o| o.id == drag.id)
        .map(|o| rotation_center(o, fonts));
    let text_resize = objs.iter().find(|o| o.id == drag.id).and_then(|o| {
        if let AnnotationKind::Text(t) = &o.kind {
            let (w, h) = text_layout_size(t, fonts);
            Some((w, h, (o.pivot.0 + w * 0.5, o.pivot.1 + h * 0.5)))
        } else {
            None
        }
    });

    let mut changed = false;
    // resize 後にテキストの pivot を中心固定で再計算するための予約 (借用解除後に行う)。
    let mut text_recenter: Option<(u64, (f32, f32))> = None;

    if let Some(o) = objs.iter_mut().find(|o| o.id == drag.id) {
        let id = o.id;
        let obj_rot = o.rotation_rad;
        match drag.kind {
            TextDragKind::Move => {
                let dx = img.0 - drag.last_img.0;
                let dy = img.1 - drag.last_img.1;
                if dx != 0.0 || dy != 0.0 {
                    translate_object(o, dx, dy);
                    changed = true;
                }
            }
            TextDragKind::Rotate => {
                let c = rot_center.unwrap_or(o.pivot);
                let relx = img.0 - c.0;
                let rely = img.1 - c.1;
                // ハンドルは rotation 0 で局所 -Y (上) を指す。
                o.rotation_rad = rely.atan2(relx) + std::f32::consts::FRAC_PI_2;
                changed = true;
            }
            TextDragKind::Corner(_) => {
                let pivot = o.pivot;
                let (sin, cos) = o.rotation_rad.sin_cos();
                let relx = img.0 - pivot.0;
                let rely = img.1 - pivot.1;
                // 局所軸へ逆回転 (pivot 対称リサイズ)。
                let lx = relx * cos + rely * sin;
                let ly = -relx * sin + rely * cos;
                match &mut o.kind {
                    AnnotationKind::Bubble(b) => {
                        set_bubble_half_extents(b, lx.abs().max(10.0), ly.abs().max(10.0));
                        b.auto_size = false;
                        b.shape_preset_link = None;
                        changed = true;
                    }
                    AnnotationKind::MessageWindow(w) => {
                        w.half_w = lx.abs().max(20.0);
                        w.half_h = ly.abs().max(12.0);
                        w.size_mode = SizeMode::Inset;
                        w.position = WindowPosition::Free;
                        w.style_preset_link = None;
                        changed = true;
                    }
                    AnnotationKind::Stamp(s) => {
                        // アスペクト比を保った一様スケール。
                        let aspect = if s.half_h > 1e-3 {
                            s.half_w / s.half_h
                        } else {
                            1.0
                        };
                        let cand_w = lx.abs().max(8.0);
                        let cand_h = ly.abs().max(8.0);
                        let new_w = cand_w.max(cand_h * aspect);
                        s.half_w = new_w;
                        s.half_h = (new_w / aspect.max(1e-3)).max(8.0);
                        changed = true;
                    }
                    AnnotationKind::Text(t) => {
                        if let Some((w, h, center)) = text_resize {
                            // レイアウト中心まわりで逆回転 → 局所スケール係数 → size_px。
                            let local = rotate_about(img, center, -obj_rot);
                            let lx = local.0 - center.0;
                            let ly = local.1 - center.1;
                            let sx = lx.abs() / (w * 0.5).max(1.0);
                            let sy = ly.abs() / (h * 0.5).max(1.0);
                            let scale = sx.max(sy).clamp(0.12, 12.0);
                            let new_size = (t.size_px * scale).clamp(6.0, 240.0);
                            if (new_size - t.size_px).abs() > 0.01 {
                                t.size_px = new_size;
                                t.preset_link = None;
                                changed = true;
                            }
                            // resize 後は新レイアウト寸法で pivot を中心固定再計算する。
                            text_recenter = Some((id, center));
                        }
                    }
                }
            }
            TextDragKind::TailTip => {
                let local = rotate_about(img, o.pivot, -o.rotation_rad);
                if let AnnotationKind::Bubble(b) = &mut o.kind {
                    if let Some(tail) = &mut b.tail {
                        tail.tip = local;
                        changed = true;
                    }
                }
            }
            TextDragKind::TailBase => {
                let pivot = o.pivot;
                let local = rotate_about(img, pivot, -o.rotation_rad);
                if let (AnnotationKind::Bubble(b), Some(fonts)) = (&mut o.kind, fonts) {
                    if b.tail.is_some() {
                        let eff = comic_core::effective_bubble_shape(b, fonts);
                        let t = comic_core::nearest_base_t(&eff, pivot, local);
                        if let Some(tail) = &mut b.tail {
                            tail.base_auto = false;
                            tail.base_t = t;
                            changed = true;
                        }
                    }
                }
            }
        }
    }

    // テキスト resize の pivot 再計算 (借用解除後)。
    if let Some((id, center)) = text_recenter {
        if let Some(o2) = objs.iter_mut().find(|o| o.id == id) {
            let size = if let AnnotationKind::Text(t) = &o2.kind {
                Some(text_layout_size(t, fonts))
            } else {
                None
            };
            if let Some((nw, nh)) = size {
                o2.pivot = (center.0 - nw * 0.5, center.1 - nh * 0.5);
            }
        }
    }

    changed
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
        self.text_dirty_at = None;
        self.text_add_bubble_dialog = false;
        self.text_add_window_dialog = false;
        self.text_add_onomatopoeia_dialog = false;
        // スタンプピッカーの差し替え対象は page-local id なので、モードをまたいで残すと
        // 別ページの同 id スタンプを誤って差し替える (Codex P2)。入場時に必ずクリアする。
        self.text_add_stamp_dialog = false;
        self.stamp_dialog_replace_target = None;
        self.clear_meta_undo();
        self.ensure_comic_doc_loaded(&key);

        // テキスト編集はシステムフォントでも動くが、初回入場時に追加パック (オノマトペ
        // 向けフォント + 被写体分離モデル) の取得を案内する (spec §4.1)。未導入かつ
        // 未辞退のときだけ確認モーダルを開く。
        self.maybe_prompt_editing_addon();

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
        self.text_dirty_at = None;
        self.text_add_bubble_dialog = false;
        self.text_add_window_dialog = false;
        self.text_add_onomatopoeia_dialog = false;
        // スタンプピッカーの差し替え対象を退場時にもクリア (Codex P2、enter 側と対)。
        self.text_add_stamp_dialog = false;
        self.stamp_dialog_replace_target = None;
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
        self.text_dirty_at = None;
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
            scale: total_scale,
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
        let detail_rect = self.text_detail_panel_rect(image_rect);
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
                if panel_rect.contains(pos) || detail_rect.contains(pos) {
                    return; // パネル上のクリックはキャンバス操作にしない
                }
                let img = view.screen_to_image(pos);
                // 優先: 選択中オブジェクトのハンドル (しっぽ/回転/四隅) → 本体 hit-test。
                let handle = self.text_selected.and_then(|sel| {
                    self.comic_docs
                        .get(&key)
                        .and_then(|objs| objs.iter().find(|o| o.id == sel))
                        .and_then(|o| pick_handle(o, pos, &view, fonts.as_deref()))
                });
                let (drag_id, kind) = match handle {
                    Some(k) => (self.text_selected, Some(k)),
                    None => {
                        let hit = self
                            .comic_docs
                            .get(&key)
                            .and_then(|objs| hit_test(objs, img, fonts.as_deref()));
                        self.text_selected = hit;
                        (hit, hit.map(|_| TextDragKind::Move))
                    }
                };
                self.text_drag = match (drag_id, kind) {
                    (Some(id), Some(kind)) => Some(TextDrag {
                        id,
                        kind,
                        start: pos,
                        last_img: img,
                        armed: false,
                        moved: false,
                    }),
                    _ => None,
                };
            }
        } else if down {
            if let (Some(pos), Some(mut drag)) = (pos, self.text_drag) {
                // 閾値を超えるまで変形を適用しない (単なるクリックでハンドルが微小に
                // 動く / 不要保存が出るのを防ぐ。ラボの resp.dragged() 相当)。
                const DRAG_ARM_PX: f32 = 4.0;
                if drag.armed || (pos - drag.start).length() >= DRAG_ARM_PX {
                    drag.armed = true;
                    let img = view.screen_to_image(pos);
                    let changed = self
                        .comic_docs
                        .get_mut(&key)
                        .map(|objs| apply_text_drag(objs, &drag, img, fonts.as_deref()))
                        .unwrap_or(false);
                    if changed {
                        drag.moved = true;
                        self.mark_comic_dirty();
                    }
                    drag.last_img = img;
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
                    self.text_dirty_at = None;
                }
            }
        }
    }

    // ── オーバーレイ描画 ──────────────────────────────────────────────

    /// テキストモードのパネル領域 (クリック吸収判定用)。
    /// 左右パネルの本体 (スクロール領域) の高さ。`draw_text_panel` と入力抑制矩形
    /// (`text_panel_rect` / `text_detail_panel_rect`) で同じ値を共有する。
    fn text_panel_body_height(image_rect: egui::Rect) -> f32 {
        // body (一覧スクロール + 操作行) は利用可能高から chrome (ヘッダ / 追加行 /
        // セパレータ / popup 余白 ≈ 124px) を引いた分。画面が高いほど一覧も縦に伸ばし、
        // スクロールバーを出す前に余白を活用する (実機 FB)。rect は body + chrome なので
        // 画面内に収まる。上限は超大型ディスプレイ向けの安全弁。
        (image_rect.height() - PANEL_MARGIN_Y * 2.0 - 124.0).clamp(220.0, 1600.0)
    }

    pub(crate) fn text_panel_rect(&self, image_rect: egui::Rect) -> egui::Rect {
        let pos = egui::pos2(
            image_rect.min.x + PANEL_MARGIN_X,
            image_rect.min.y + PANEL_MARGIN_Y,
        );
        // 本体高 + chrome (ヘッダ + 追加行 + セパレータ 2 本 + popup 余白) を確保し、
        // 実際に見えているパネルより矩形が短くならないようにする (Codex P2: 下端の帯で
        // キャンバスクリックが貫通し選択解除/ドラッグが起きるのを防ぐ)。画面下端で打ち切る。
        let h = (Self::text_panel_body_height(image_rect) + 108.0)
            .min(image_rect.height() - PANEL_MARGIN_Y - 4.0)
            .max(120.0);
        egui::Rect::from_min_size(pos, egui::vec2(PANEL_W + 16.0, h))
    }

    /// 詳細設定 (選択オブジェクトの編集 UI) を載せる右パネルの矩形。補正レイヤーの
    /// ツールパネルと同様、画面右端に寄せる。`text_panel_rect` (左=一覧) と対になる。
    pub(crate) fn text_detail_panel_rect(&self, image_rect: egui::Rect) -> egui::Rect {
        let w = PANEL_W + 16.0;
        let x = (image_rect.max.x - w - PANEL_MARGIN_X).max(image_rect.min.x + PANEL_MARGIN_X);
        let pos = egui::pos2(x, image_rect.min.y + PANEL_MARGIN_Y);
        // 右は「詳細設定」見出し + セパレータ + popup 余白のみ (追加行が無い分 chrome 小)。
        let h = (Self::text_panel_body_height(image_rect) + 72.0)
            .min(image_rect.height() - PANEL_MARGIN_Y - 4.0)
            .max(120.0);
        egui::Rect::from_min_size(pos, egui::vec2(w, h))
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
        // 追加ダイアログ (パネルが comic_docs を書き戻した後に描く)。
        self.draw_text_add_bubble_dialog(ctx);
        self.draw_text_add_window_dialog(ctx);
        self.draw_text_add_stamp_dialog(ctx);
        self.draw_text_add_onomatopoeia_dialog(ctx);
    }

    /// 選択中オブジェクトの変形ハンドルを画面に描く。回転を反映した境界四角形 +
    /// 回転ノブ (緑) + 四隅リサイズハンドル (白角) + 吹き出しのしっぽハンドル
    /// (cyan=根元 / 橙=先端)。ラボ `draw_selection_handles` と同じ見た目。
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
        let Some(o) = self
            .comic_docs
            .get(&key)
            .and_then(|objs| objs.iter().find(|o| o.id == id))
            .cloned()
        else {
            return;
        };
        let Some((corners, roth)) = handle_points(&o, fonts.as_deref(), view.scale) else {
            return;
        };
        let painter = ui.painter().with_clip_rect(image_rect);
        let blue = sel_color();
        let cs: Vec<egui::Pos2> = corners
            .iter()
            .map(|c| view.image_to_screen(c.0, c.1))
            .collect();
        // 回転を反映した境界四角形。
        let stroke = egui::Stroke::new(1.5, blue);
        for i in 0..4 {
            painter.line_segment([cs[i], cs[(i + 1) % 4]], stroke);
        }
        // 回転ノブ: 上端中点から茎 + 緑の丸。
        let top_mid = egui::pos2((cs[0].x + cs[1].x) * 0.5, (cs[0].y + cs[1].y) * 0.5);
        let roth_s = view.image_to_screen(roth.0, roth.1);
        painter.line_segment([top_mid, roth_s], stroke);
        painter.circle_filled(roth_s, HANDLE_R, egui::Color32::from_rgb(120, 220, 120));
        painter.circle_stroke(
            roth_s,
            HANDLE_R,
            egui::Stroke::new(1.5, egui::Color32::BLACK),
        );
        // 四隅リサイズハンドル (白い小四角)。
        for c in &cs {
            let r = egui::Rect::from_center_size(*c, egui::vec2(HANDLE_R * 1.8, HANDLE_R * 1.8));
            painter.rect_filled(r, 1.0, egui::Color32::from_rgb(230, 230, 235));
            painter.rect_stroke(
                r,
                1.0,
                egui::Stroke::new(1.5, egui::Color32::BLACK),
                egui::StrokeKind::Outside,
            );
        }
        // 吹き出しのしっぽハンドル (cyan=根元 / 橙=先端)。
        if let Some((base, tip)) = tail_handle_points(&o, fonts.as_deref()) {
            let bp = view.image_to_screen(base.0, base.1);
            painter.circle_filled(bp, HANDLE_R, egui::Color32::from_rgb(80, 200, 220));
            painter.circle_stroke(bp, HANDLE_R, egui::Stroke::new(1.5, egui::Color32::BLACK));
            let tp = view.image_to_screen(tip.0, tip.1);
            painter.circle_filled(tp, HANDLE_R, egui::Color32::from_rgb(255, 160, 60));
            painter.circle_stroke(tp, HANDLE_R, egui::Stroke::new(1.5, egui::Color32::BLACK));
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
        let detail_rect = self.text_detail_panel_rect(image_rect);
        let body_height = Self::text_panel_body_height(image_rect);

        // 借用衝突を避けるため作業セットを一旦取り出し、ローカルだけを編集する。
        let mut objects = self.comic_docs.remove(&key).unwrap_or_default();
        let mut selected = self.text_selected;
        let mut prop_tab = self.text_prop_tab;
        let mut changed = false;
        let mut close = false;
        let mut open_bubble_dialog = false;
        let mut open_window_dialog = false;
        let mut open_stamp_dialog = false;
        let mut open_stamp_replace = false;
        let mut open_onomatopoeia_dialog = false;

        // ── 左パネル: ヘッダ + 追加 + オブジェクト一覧 (補正レイヤー風) ──
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
                        // フルスクリーンビューポートの visuals は明るい場合があり、暗い
                        // パネル背景に暗い文字が乗って読めなくなる。隠蔽 (conceal) パネルと
                        // 同様に暗テーマ + 白文字を明示する。
                        *ui.visuals_mut() = egui::Visuals::dark();
                        ui.visuals_mut().override_text_color = Some(egui::Color32::WHITE);
                        ui.set_min_width(PANEL_W);
                        ui.set_max_width(PANEL_W);
                        ui.horizontal(|ui| {
                            ui.strong("テキスト注釈");
                            ui.with_layout(
                                egui::Layout::right_to_left(egui::Align::Center),
                                |ui| {
                                    // 閉じる × ボタン。消しゴム / 隠蔽 / 補正レイヤーと同じ
                                    // draw_close_icon 様式に統一 (テキストボタンだと見た目が
                                    // 浮くため、実機 FB で指摘あり)。
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
                                        close = true;
                                    }
                                    close_resp.on_hover_text("閉じる (Esc / Ctrl+T)");
                                },
                            );
                        });
                        ui.separator();

                        // ── 追加 ── (ボタンが多いので折り返しレイアウトにする)
                        ui.label("追加:");
                        ui.horizontal_wrapped(|ui| {
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
                                // 形状を選ぶダイアログを開く (ラボの「吹き出しを追加」相当)。
                                open_bubble_dialog = true;
                            }
                            if ui.button("ウィンドウ").clicked() {
                                open_window_dialog = true;
                            }
                            if ui.button("スタンプ").clicked() {
                                open_stamp_dialog = true;
                            }
                            if ui.button("オノマトペ").clicked() {
                                open_onomatopoeia_dialog = true;
                            }
                        });
                        ui.separator();

                        // 一覧 (ScrollArea) と操作行 (固定) を分離。操作行ぶんの高さを
                        // 先に確保し、残りを一覧スクロールに割り当てる。これで一覧を
                        // 多数追加してスクロールしても ↑↓複製削除 が常に見える。
                        let actions_h = 34.0_f32;
                        let list_h = (body_height - actions_h).max(80.0);
                        ui.allocate_ui_with_layout(
                            egui::vec2(PANEL_W, list_h),
                            egui::Layout::top_down(egui::Align::Min),
                            |ui| {
                                egui::ScrollArea::vertical()
                                    .id_salt("text_panel_scroll")
                                    .max_height(list_h)
                                    .auto_shrink([false, false])
                                    .show(ui, |ui| {
                                        object_list_rows_ui(
                                            ui,
                                            &mut objects,
                                            &mut selected,
                                            &mut changed,
                                        );
                                    });
                            },
                        );
                        ui.separator();
                        object_list_actions_ui(ui, &mut objects, &mut selected, &mut changed);
                    });
            });

        // ── 右パネル: 詳細設定 (選択オブジェクトの編集) ──
        egui::Area::new(egui::Id::new("text_detail_panel"))
            .fixed_pos(detail_rect.min)
            .order(egui::Order::Foreground)
            .interactable(true)
            .show(ctx, |ui| {
                ui.interact(
                    detail_rect,
                    egui::Id::new("text_detail_panel_click_sink"),
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
                        *ui.visuals_mut() = egui::Visuals::dark();
                        ui.visuals_mut().override_text_color = Some(egui::Color32::WHITE);
                        ui.set_min_width(PANEL_W);
                        ui.set_max_width(PANEL_W);
                        ui.strong("詳細設定");
                        ui.separator();
                        ui.allocate_ui_with_layout(
                            egui::vec2(PANEL_W, body_height),
                            egui::Layout::top_down(egui::Align::Min),
                            |ui| {
                                egui::ScrollArea::vertical()
                                    .id_salt("text_detail_scroll")
                                    .max_height(body_height)
                                    .auto_shrink([false, false])
                                    .show(ui, |ui| {
                                        if let Some(o) = selected
                                            .and_then(|id| objects.iter_mut().find(|o| o.id == id))
                                        {
                                            edit_object_ui(
                                                ui,
                                                o,
                                                &mut prop_tab,
                                                &mut changed,
                                                &mut open_stamp_replace,
                                            );
                                        } else {
                                            ui.label(
                                                egui::RichText::new(
                                                    "左の一覧で選択 / キャンバスをクリックで選択\nドラッグで移動・ハンドルで変形",
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

        // 追加ボタンはダイアログを開く (実際の追加はダイアログ側)。
        if open_bubble_dialog {
            self.text_add_bubble_dialog = true;
        }
        if open_window_dialog {
            self.text_add_window_dialog = true;
        }
        if open_stamp_dialog {
            // 新規追加 (差し替え対象なし)。
            self.stamp_dialog_replace_target = None;
            self.text_add_stamp_dialog = true;
        }
        if open_stamp_replace {
            // 詳細パネルの「別のスタンプに変更」: 選択中スタンプを差し替え対象にする。
            self.stamp_dialog_replace_target = selected;
            self.text_add_stamp_dialog = true;
        }
        if open_onomatopoeia_dialog {
            self.text_add_onomatopoeia_dialog = true;
        }

        // 書き戻し。編集中は毎フレーム DB へ書かず (Codex P2)、メモリ更新 + 再ベイクに留め、
        // 編集が止まってから (デバウンス) / 退場時に 1 度だけ comic.db + サイドカーへ保存する。
        self.text_selected = selected;
        self.text_prop_tab = prop_tab;
        const DEBOUNCE: std::time::Duration = std::time::Duration::from_millis(700);
        let save_now = if changed {
            self.mark_comic_dirty();
            self.text_dirty_at = Some(std::time::Instant::now());
            false
        } else {
            self.text_dirty_at
                .map(|t| t.elapsed() >= DEBOUNCE)
                .unwrap_or(false)
        };
        if save_now {
            // save_comic_objects が comic_docs へも書き戻す。
            self.save_comic_objects(fs_idx, &key, &objects);
            self.text_dirty_at = None;
        } else {
            self.comic_docs.insert(key, objects);
        }
        if close {
            self.reset_text_mode();
        }
    }

    /// 「吹き出しを追加」形状ピッカー。形状プレビューのグリッドからクリックで追加する
    /// (ラボの「吹き出しを追加」ダイアログ相当)。
    fn draw_text_add_bubble_dialog(&mut self, ctx: &egui::Context) {
        if !self.text_add_bubble_dialog {
            return;
        }
        let Some(fs_idx) = self.fullscreen_idx else {
            self.text_add_bubble_dialog = false;
            return;
        };
        let Some(key) = self.page_path_key(fs_idx) else {
            self.text_add_bubble_dialog = false;
            return;
        };
        let (sw, sh) = self.source_dims_for_idx(fs_idx).unwrap_or((1000.0, 1000.0));
        let font_key = crate::comic_overlay::COMIC_FONT_KEY.to_string();

        let mut open = true;
        let mut chosen: Option<BubblePreset> = None;
        let avail = ctx.content_rect();
        let default_w = (avail.width() - 24.0).clamp(360.0, 600.0);
        let default_h = (avail.height() - 120.0).clamp(240.0, 560.0);
        let frame = egui::Frame::window(ctx.style().as_ref())
            .fill(egui::Color32::from_rgba_unmultiplied(24, 24, 26, 248))
            .stroke(egui::Stroke::new(
                1.0,
                egui::Color32::from_rgba_unmultiplied(255, 255, 255, 70),
            ));
        egui::Window::new("吹き出しを追加")
            .id(egui::Id::new("text_add_bubble_dialog"))
            .order(egui::Order::Foreground)
            .frame(frame)
            .collapsible(false)
            .resizable(true)
            .default_size([default_w, default_h])
            .default_pos(avail.center() - egui::vec2(default_w, default_h) * 0.5)
            .open(&mut open)
            .show(ctx, |ui| {
                *ui.visuals_mut() = egui::Visuals::dark();
                ui.visuals_mut().override_text_color = Some(egui::Color32::WHITE);
                ui.label(
                    egui::RichText::new("形を選んでください。クリックで追加します。")
                        .size(12.0)
                        .color(egui::Color32::from_gray(190)),
                );
                ui.separator();
                egui::ScrollArea::vertical()
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        let cols = ((ui.available_width() / (PRESET_CELL_W + 8.0)).floor()
                            as usize)
                            .max(1);
                        ui.spacing_mut().item_spacing = egui::vec2(8.0, 8.0);
                        for chunk in BubblePreset::ALL.chunks(cols) {
                            ui.horizontal_top(|ui| {
                                for &preset in chunk {
                                    if draw_bubble_preset_thumbnail(ui, preset) {
                                        chosen = Some(preset);
                                    }
                                }
                            });
                        }
                    });
            });

        self.text_add_bubble_dialog = open;
        if let Some(preset) = chosen {
            let mut objs = self.comic_docs.remove(&key).unwrap_or_default();
            let id = next_id(&objs);
            let z = objs.len() as i32;
            // 画像中心に、ソース寸法に合わせたサイズで配置 (build_bubble は auto_size=true
            // なのでセリフ文字数に合わせて自動でフィットする)。
            let pivot = (sw * 0.5, sh * 0.5);
            let mut b = preset.build_bubble(pivot, &font_key);
            // ソース解像度に合わせて文字サイズだけ調整 (既定 40px はラボ基準)。
            b.text.size_px = (sh * 0.035).clamp(24.0, 96.0);
            let mut o = AnnotationObject::new_bubble(id, pivot, b);
            o.z = z;
            objs.push(o);
            self.text_selected = Some(id);
            self.save_comic_objects(fs_idx, &key, &objs);
            self.text_dirty_at = None;
            self.mark_comic_dirty();
            self.text_add_bubble_dialog = false;
        }
    }

    /// 「ウィンドウを追加」スタイルピッカー。定義済みスタイルのボタンからクリックで追加する。
    fn draw_text_add_window_dialog(&mut self, ctx: &egui::Context) {
        if !self.text_add_window_dialog {
            return;
        }
        let Some(fs_idx) = self.fullscreen_idx else {
            self.text_add_window_dialog = false;
            return;
        };
        let Some(key) = self.page_path_key(fs_idx) else {
            self.text_add_window_dialog = false;
            return;
        };
        let (sw, sh) = self.source_dims_for_idx(fs_idx).unwrap_or((1000.0, 1000.0));
        let font_key = crate::comic_overlay::COMIC_FONT_KEY.to_string();

        let mut open = true;
        let mut chosen: Option<usize> = None;
        let avail = ctx.content_rect();
        let default_w = (avail.width() - 24.0).clamp(320.0, 540.0);
        let default_h = (avail.height() - 120.0).clamp(220.0, 460.0);
        let frame = egui::Frame::window(ctx.style().as_ref())
            .fill(egui::Color32::from_rgba_unmultiplied(24, 24, 26, 248))
            .stroke(egui::Stroke::new(
                1.0,
                egui::Color32::from_rgba_unmultiplied(255, 255, 255, 70),
            ));
        egui::Window::new("ウィンドウを追加")
            .id(egui::Id::new("text_add_window_dialog"))
            .order(egui::Order::Foreground)
            .frame(frame)
            .collapsible(false)
            .resizable(true)
            .default_size([default_w, default_h])
            .default_pos(avail.center() - egui::vec2(default_w, default_h) * 0.5)
            .open(&mut open)
            .show(ctx, |ui| {
                *ui.visuals_mut() = egui::Visuals::dark();
                ui.visuals_mut().override_text_color = Some(egui::Color32::WHITE);
                ui.label(
                    egui::RichText::new("デザインを選んでください。クリックで追加します。")
                        .size(12.0)
                        .color(egui::Color32::from_gray(190)),
                );
                ui.separator();
                egui::ScrollArea::vertical()
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        let cols =
                            ((ui.available_width() / (WIN_CELL_W + 8.0)).floor() as usize).max(1);
                        ui.spacing_mut().item_spacing = egui::vec2(8.0, 8.0);
                        for chunk in (0..WIN_PRESETS.len()).collect::<Vec<_>>().chunks(cols) {
                            ui.horizontal_top(|ui| {
                                for &i in chunk {
                                    if draw_winpreset_thumbnail(ui, &WIN_PRESETS[i]) {
                                        chosen = Some(i);
                                    }
                                }
                            });
                        }
                    });
            });

        self.text_add_window_dialog = open;
        if let Some(i) = chosen {
            let p = &WIN_PRESETS[i];
            let mut objs = self.comic_docs.remove(&key).unwrap_or_default();
            let id = next_id(&objs);
            let z = objs.len() as i32;
            let mut w = MessageWindowObject {
                position: WindowPosition::Free,
                half_w: (sw * 0.4).max(120.0),
                half_h: (sh * 0.12).max(60.0),
                frame: p.frame,
                fill_mode: p.fill_mode,
                fill: p.fill,
                fill_opacity: p.fill_opacity,
                gradient_to: p.gradient_to,
                scrim_dense_side: p.scrim_dense_side,
                corner_px: p.corner_px,
                outline: StrokeStyle {
                    color: p.outline,
                    width_px: p.outline_w,
                },
                ..MessageWindowObject::default()
            };
            w.text.text = "本文".to_string();
            w.text.font_key = font_key.clone();
            w.text.size_px = (sh * 0.03).clamp(20.0, 64.0);
            w.text.color = p.text_color;
            let mut o = AnnotationObject::new_message_window(id, (sw * 0.5, sh * 0.8), w);
            o.z = z;
            objs.push(o);
            self.text_selected = Some(id);
            self.save_comic_objects(fs_idx, &key, &objs);
            self.text_dirty_at = None;
            self.mark_comic_dirty();
            self.text_add_window_dialog = false;
        }
    }

    /// オノマトペプリセットの `font_candidate` を実フォントキーへ解決する。列挙済みの
    /// `comic_available_fonts` から区切り無視の部分一致で探し、見つからなければ既定
    /// フォント ("ui") を返す (= 追加パック未導入時はシステム既定で焼かれる)。
    /// 呼び出し前に `ensure_comic_font_registry()` を済ませておくこと。
    fn resolve_onomatopoeia_font(&self, candidate: &str) -> String {
        self.comic_available_fonts
            .iter()
            .find(|a| crate::font_assets::font_name_matches_candidate(&a.key, candidate))
            .map(|a| a.key.clone())
            .unwrap_or_else(|| crate::comic_overlay::COMIC_FONT_KEY.to_string())
    }

    /// オノマトペプリセットを実フォントで comic-core ベイクし、ピッカー用プレビュー画像を
    /// 返す。回転は意図的に省く (字形を比べやすく)。既定フォントすら無ければ `None`。
    fn render_onomatopoeia_preview(
        &mut self,
        preset: OnomatopoeiaPreset,
        font_key: &str,
    ) -> Option<egui::ColorImage> {
        let mut block = preset.build_text(font_key);
        block.size_px = block.size_px.clamp(50.0, 92.0);
        // フォントをロード (参照フォントを含む FontSet を得る)。
        let probe = AnnotationObject::new_text(0, (0.0, 0.0), block.clone());
        let fonts = self.ensure_comic_fonts_for(std::slice::from_ref(&probe))?;
        let font = fonts.get(font_key)?;
        let layout = comic_core::layout_text(&block, font);
        let outline_pad = block.outline.map(|s| s.width_px).unwrap_or(0.0);
        let pad = (outline_pad + 12.0).ceil();
        let w = ((layout.bounds.0 + pad * 2.0).ceil() as usize).clamp(1, 1600);
        let h = ((layout.bounds.1 + pad * 2.0).ceil() as usize).clamp(1, 1200);
        let obj = AnnotationObject::new_text(0, (pad, pad), block);
        let overlay = comic_core::bake_overlay(&[obj], w, h, &fonts);
        Some(egui::ColorImage::from_rgba_unmultiplied(
            [overlay.w, overlay.h],
            &overlay.pixels,
        ))
    }

    /// オノマトペプリセットのサムネイルテクスチャを遅延構築 + キャッシュして返す。
    /// キーは `<label>|<text>|<解決フォント>`。呼び出し前に registry を ensure すること。
    fn onomatopoeia_thumb_texture(
        &mut self,
        ctx: &egui::Context,
        preset: OnomatopoeiaPreset,
    ) -> Option<egui::TextureHandle> {
        let font_key = self.resolve_onomatopoeia_font(preset.font_candidate);
        let cache_key = format!("{}|{}|{}", preset.label, preset.text, font_key);
        if let Some(tex) = self.onomatopoeia_thumb_cache.get(&cache_key) {
            return Some(tex.clone());
        }
        let img = self.render_onomatopoeia_preview(preset, &font_key)?;
        let tex_name = format!(
            "onomato_preview_{}",
            crate::font_assets::font_lookup_key(&cache_key)
        );
        let tex = ctx.load_texture(tex_name, img, egui::TextureOptions::LINEAR);
        self.onomatopoeia_thumb_cache.insert(cache_key, tex.clone());
        Some(tex)
    }

    /// 「オノマトペを追加」プリセットピッカー。実フォントで焼いたサムネイルのグリッドから
    /// クリックで標準テキストオブジェクトを追加する (ラボ準拠)。追加パック未導入時は
    /// 注意書き + 入手ボタンを出すが、既定フォントで追加することもできる。
    fn draw_text_add_onomatopoeia_dialog(&mut self, ctx: &egui::Context) {
        if !self.text_add_onomatopoeia_dialog {
            return;
        }
        let Some(fs_idx) = self.fullscreen_idx else {
            self.text_add_onomatopoeia_dialog = false;
            return;
        };
        let Some(key) = self.page_path_key(fs_idx) else {
            self.text_add_onomatopoeia_dialog = false;
            return;
        };
        let (sw, sh) = self.source_dims_for_idx(fs_idx).unwrap_or((1000.0, 1000.0));
        let pack_installed = crate::editing_addon::is_installed();

        // サムネイルは &mut self が要るので read-only クロージャの前に用意する
        // (スタンプピッカーと同じ流儀)。全プリセットのフォントを 1 回の FontSet 再構築で
        // まとめてロードしてから (= プリセットごとの再構築を避ける)、各サムネを焼く。
        self.ensure_comic_font_registry();
        let probe: Vec<AnnotationObject> = ONOMATOPOEIA_PRESETS
            .iter()
            .map(|p| {
                let fk = self.resolve_onomatopoeia_font(p.font_candidate);
                AnnotationObject::new_text(0, (0.0, 0.0), p.build_text(&fk))
            })
            .collect();
        let _ = self.ensure_comic_fonts_for(&probe);
        let mut cards: Vec<(OnomatopoeiaPreset, Option<egui::TextureHandle>)> =
            Vec::with_capacity(ONOMATOPOEIA_PRESETS.len());
        for &preset in ONOMATOPOEIA_PRESETS {
            let tex = self.onomatopoeia_thumb_texture(ctx, preset);
            cards.push((preset, tex));
        }

        let mut open = true;
        let mut chosen: Option<OnomatopoeiaPreset> = None;
        let mut request_pack = false;
        let avail = ctx.content_rect();
        let default_w = (avail.width() - 24.0).clamp(360.0, 860.0);
        let default_h = (avail.height() - 120.0).clamp(300.0, 660.0);
        let frame = egui::Frame::window(ctx.style().as_ref())
            .fill(egui::Color32::from_rgba_unmultiplied(24, 24, 26, 248))
            .stroke(egui::Stroke::new(
                1.0,
                egui::Color32::from_rgba_unmultiplied(255, 255, 255, 70),
            ));
        egui::Window::new("オノマトペを追加")
            .id(egui::Id::new("text_add_onomatopoeia_dialog"))
            .order(egui::Order::Foreground)
            .frame(frame)
            .collapsible(false)
            .resizable(true)
            .default_size([default_w, default_h])
            .default_pos(avail.center() - egui::vec2(default_w, default_h) * 0.5)
            .open(&mut open)
            .show(ctx, |ui| {
                *ui.visuals_mut() = egui::Visuals::dark();
                ui.visuals_mut().override_text_color = Some(egui::Color32::WHITE);
                if !pack_installed {
                    ui.label(
                        egui::RichText::new(
                            "オノマトペ用の装飾フォントは編集用追加パックに含まれます。",
                        )
                        .size(12.0)
                        .color(egui::Color32::from_rgb(220, 180, 90)),
                    );
                    ui.label(
                        egui::RichText::new(
                            "未導入のため、追加するとシステム既定フォントで作成されます。",
                        )
                        .size(11.0)
                        .color(egui::Color32::from_gray(170)),
                    );
                    if ui.button("編集用追加パックを入手…").clicked() {
                        request_pack = true;
                    }
                    ui.separator();
                }
                ui.label(
                    egui::RichText::new(
                        "フォントごとのサンプルを選んでください。クリックで追加します。",
                    )
                    .size(11.0)
                    .color(egui::Color32::from_gray(180)),
                );
                ui.separator();
                egui::ScrollArea::vertical()
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        let cols = ((ui.available_width() / (ONOMATO_CELL_W + 10.0)).floor()
                            as usize)
                            .max(1);
                        ui.spacing_mut().item_spacing = egui::vec2(10.0, 10.0);
                        for chunk in cards.chunks(cols) {
                            ui.horizontal_top(|ui| {
                                for (preset, tex) in chunk {
                                    if draw_onomatopoeia_card(ui, *preset, tex.as_ref()) {
                                        chosen = Some(*preset);
                                    }
                                }
                            });
                        }
                    });
            });

        self.text_add_onomatopoeia_dialog = open;
        if request_pack {
            // 明示クリックなのでこのセッションで辞退済みでも開く。
            self.editing_addon_declined_session = false;
            self.maybe_prompt_editing_addon();
        }
        if let Some(preset) = chosen {
            self.add_onomatopoeia_object(fs_idx, &key, preset, sw, sh);
            self.text_add_onomatopoeia_dialog = false;
        }
    }

    /// オノマトペプリセットを標準テキストオブジェクトとして画像中央へ追加し、永続化する。
    /// サイズはソース寸法に比例スケールし、回転はプリセット値を適用する。
    fn add_onomatopoeia_object(
        &mut self,
        fs_idx: usize,
        key: &str,
        preset: OnomatopoeiaPreset,
        sw: f32,
        sh: f32,
    ) {
        self.ensure_comic_font_registry();
        let font_key = self.resolve_onomatopoeia_font(preset.font_candidate);
        let mut tb = preset.build_text(&font_key);
        // プリセットは ~760px 基準のラボ寸法。ソース解像度に比例させる (相対比は保つ)。
        let scale = (sh / 760.0).clamp(0.4, 12.0);
        tb.size_px = (preset.size_px * scale).clamp(20.0, sh * 0.45);
        // レイアウト寸法を測って画像中央へ寄せる (単独テキストの pivot は左上)。
        let probe = AnnotationObject::new_text(0, (0.0, 0.0), tb.clone());
        let fonts = self.ensure_comic_fonts_for(std::slice::from_ref(&probe));
        let (lw, lh) = text_layout_size(&tb, fonts.as_deref());
        let pivot = (sw * 0.5 - lw * 0.5, sh * 0.5 - lh * 0.5);

        let mut objs = self.comic_docs.remove(key).unwrap_or_default();
        let id = next_id(&objs);
        let z = objs.len() as i32;
        let mut o = AnnotationObject::new_text(id, pivot, tb);
        o.rotation_rad = preset.rotation_deg.to_radians();
        o.z = z;
        objs.push(o);
        self.text_selected = Some(id);
        self.save_comic_objects(fs_idx, key, &objs);
        self.text_dirty_at = None;
        self.mark_comic_dirty();
    }

    /// スタンプピッカーのサムネイルテクスチャを (なければ) 用意する。デコードは
    /// `comic_stamp_cache` (ベイクと共有) 経由で 1 度だけ行い、44px に縮小して
    /// `stamp_thumb_cache` にアップロードする。
    fn ensure_stamp_thumb(&mut self, ctx: &egui::Context, source: &comic_core::StampSource) {
        let key = crate::comic_stamp::stamp_source_key(source);
        if self.stamp_thumb_cache.contains_key(&key) {
            return;
        }
        let full = self
            .comic_stamp_cache
            .entry(key.clone())
            .or_insert_with(|| {
                crate::comic_stamp::load_stamp_image(source).map(std::sync::Arc::new)
            })
            .clone();
        let Some(full) = full else {
            return;
        };
        let thumb = crate::comic_stamp::downscale_overlay(&full, 44);
        let color = egui::ColorImage::from_rgba_unmultiplied([thumb.w, thumb.h], &thumb.pixels);
        let tex = ctx.load_texture(
            format!("stamp_thumb_{key}"),
            color,
            egui::TextureOptions::LINEAR,
        );
        self.stamp_thumb_cache.insert(key, tex);
    }

    /// 絵文字スタンプピッカー (カテゴリタブ + 検索 + 最近使った行 + 絵文字グリッド +
    /// 「画像ファイルから追加」)。クリックで新規追加 or (差し替えモードなら) ソース差し替え。
    /// ラボ `draw_stamp_dialog` の本体移植。
    fn draw_text_add_stamp_dialog(&mut self, ctx: &egui::Context) {
        if !self.text_add_stamp_dialog {
            return;
        }
        let Some(fs_idx) = self.fullscreen_idx else {
            self.text_add_stamp_dialog = false;
            return;
        };
        let Some(key) = self.page_path_key(fs_idx) else {
            self.text_add_stamp_dialog = false;
            return;
        };
        let (sw, sh) = self.source_dims_for_idx(fs_idx).unwrap_or((1000.0, 1000.0));

        // 最近使ったスタンプを初回だけロード。
        if !self.recent_stamps_loaded {
            self.recent_stamps = crate::comic_stamp::load_recent_stamps();
            self.recent_stamps_loaded = true;
        }

        let assets = crate::comic_stamp::emoji_assets_available();
        let filter = self.stamp_dialog_filter.to_lowercase();
        let cat = self.stamp_dialog_category;
        // 表示対象: 検索中はカテゴリを無視。
        let visible: Vec<(&'static str, &'static str)> = crate::comic_stamp::EMOJI_CATALOG
            .iter()
            .filter(|e| !e.name.is_empty())
            .filter(|e| {
                if filter.is_empty() {
                    e.category == cat
                } else {
                    e.name.to_lowercase().contains(&filter) || e.key.contains(&filter)
                }
            })
            .map(|e| (e.key, e.name))
            .collect();

        // サムネイルは &mut self が要るので read-only クロージャの前に用意しておく。
        // 初回デコードは UI スレッド同期 (Codex P2)。ただし (1) これはユーザーが明示的に
        // 開くモーダルで、スクロール等のホットパスではない、(2) 絵文字はカテゴリ単位で
        // 件数が bounded + resvg 512px と軽量 + デコード結果はキャッシュで 1 度きり、
        // (3) ユーザー画像は FILE_STAMP_MAX_PX で抑制済み、なので許容する。将来、開封時の
        // 引っかかりが問題になれば worker 化する (docs/ui-responsiveness.md §2 のテンプレ)。
        if assets {
            for (k, _) in &visible {
                self.ensure_stamp_thumb(ctx, &comic_core::StampSource::Emoji((*k).to_string()));
            }
        }
        let recents = self.recent_stamps.clone();
        for s in &recents {
            self.ensure_stamp_thumb(ctx, s);
        }
        let thumbs = self.stamp_thumb_cache.clone();
        let replacing = self.stamp_dialog_replace_target.is_some();

        let mut open = true;
        let mut chosen: Option<comic_core::StampSource> = None;
        let mut pick_file = false;
        let mut filter_local = self.stamp_dialog_filter.clone();
        let mut cat_local = cat;

        let title = if replacing {
            "スタンプを変更"
        } else {
            "スタンプを追加"
        };
        let avail = ctx.content_rect();
        let default_w = (avail.width() - 24.0).clamp(360.0, 600.0);
        let default_h = (avail.height() - 120.0).clamp(280.0, 560.0);
        let frame = egui::Frame::window(ctx.style().as_ref())
            .fill(egui::Color32::from_rgba_unmultiplied(24, 24, 26, 248))
            .stroke(egui::Stroke::new(
                1.0,
                egui::Color32::from_rgba_unmultiplied(255, 255, 255, 70),
            ));
        egui::Window::new(title)
            .id(egui::Id::new("text_add_stamp_dialog"))
            .order(egui::Order::Foreground)
            .frame(frame)
            .collapsible(false)
            .resizable(true)
            .default_size([default_w, default_h])
            .default_pos(avail.center() - egui::vec2(default_w, default_h) * 0.5)
            .open(&mut open)
            .show(ctx, |ui| {
                *ui.visuals_mut() = egui::Visuals::dark();
                ui.visuals_mut().override_text_color = Some(egui::Color32::WHITE);
                ui.horizontal(|ui| {
                    if ui.button("画像ファイルから追加…").clicked() {
                        pick_file = true;
                    }
                    ui.separator();
                    ui.label("検索");
                    ui.add(
                        egui::TextEdit::singleline(&mut filter_local)
                            .hint_text("名前 / コード")
                            .desired_width(160.0),
                    );
                });

                // 最近使った行。
                if !recents.is_empty() {
                    ui.add_space(4.0);
                    ui.label(egui::RichText::new("最近使った").weak());
                    ui.horizontal_wrapped(|ui| {
                        for s in &recents {
                            let k = crate::comic_stamp::stamp_source_key(s);
                            let label = crate::comic_stamp::stamp_label(s);
                            let resp = if let Some(tex) = thumbs.get(&k) {
                                ui.add(
                                    egui::Button::image(egui::load::SizedTexture::new(
                                        tex.id(),
                                        egui::vec2(34.0, 34.0),
                                    ))
                                    .corner_radius(4.0),
                                )
                            } else {
                                ui.button(&label)
                            };
                            if resp.on_hover_text(&label).clicked() {
                                chosen = Some(s.clone());
                            }
                        }
                    });
                    ui.separator();
                }

                // カテゴリタブ (検索中は隠す)。
                if filter_local.is_empty() {
                    ui.horizontal_wrapped(|ui| {
                        for &c in crate::comic_stamp::EmojiCategory::all() {
                            ui.selectable_value(&mut cat_local, c, c.label());
                        }
                    });
                    ui.add_space(2.0);
                }

                if !assets {
                    ui.colored_label(
                        egui::Color32::from_rgb(220, 180, 90),
                        "絵文字アセット未配置: scripts/setup-twemoji.sh で取得 (画像ファイルからは追加できます)",
                    );
                }

                // 絵文字グリッド。
                egui::ScrollArea::vertical()
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        ui.horizontal_wrapped(|ui| {
                            for (k, name) in &visible {
                                let src_key = format!("e:{k}");
                                let resp = if let Some(tex) = thumbs.get(&src_key) {
                                    ui.add(
                                        egui::Button::image(egui::load::SizedTexture::new(
                                            tex.id(),
                                            egui::vec2(40.0, 40.0),
                                        ))
                                        .corner_radius(4.0),
                                    )
                                } else {
                                    // アセット無し (or デコード失敗): コンパクトなテキストチップ。
                                    ui.add_sized(
                                        [44.0, 44.0],
                                        egui::Button::new(egui::RichText::new(*k).size(9.0).weak()),
                                    )
                                };
                                if resp.on_hover_text(*name).clicked() {
                                    chosen =
                                        Some(comic_core::StampSource::Emoji((*k).to_string()));
                                }
                            }
                        });
                    });
            });

        // 編集した検索文字列 / カテゴリを書き戻す。
        self.stamp_dialog_filter = filter_local;
        self.stamp_dialog_category = cat_local;
        self.text_add_stamp_dialog = open;

        if pick_file {
            if let Some(path) = rfd::FileDialog::new()
                .add_filter("画像", &["png", "jpg", "jpeg", "webp", "gif", "bmp"])
                .pick_file()
            {
                chosen = Some(comic_core::StampSource::File(path));
            }
        }

        if let Some(src) = chosen {
            self.apply_stamp_choice(fs_idx, &key, (sw, sh), src);
            self.text_add_stamp_dialog = false;
        } else if !open {
            // × で閉じた: 差し替え対象もクリア。
            self.stamp_dialog_replace_target = None;
        }
    }

    /// ピッカーで選んだスタンプソースを適用する。差し替え対象があれば既存スタンプの
    /// ソースを差し替え (ジオメトリ保持・アスペクト再フィット)、無ければ画像中心へ新規追加。
    /// MRU 更新 + comic.db 保存まで行う。
    fn apply_stamp_choice(
        &mut self,
        fs_idx: usize,
        key: &str,
        src_dims: (f32, f32),
        source: comic_core::StampSource,
    ) {
        // ソースをデコードしてアスペクト (w/h) を得る (キャッシュ経由)。
        let skey = crate::comic_stamp::stamp_source_key(&source);
        let aspect = {
            let entry = self.comic_stamp_cache.entry(skey).or_insert_with(|| {
                crate::comic_stamp::load_stamp_image(&source).map(std::sync::Arc::new)
            });
            match entry {
                Some(o) if o.w > 0 && o.h > 0 => o.w as f32 / o.h as f32,
                _ => 1.0,
            }
        }
        .max(1e-3);

        let mut objs = self.comic_docs.remove(key).unwrap_or_default();
        match self.stamp_dialog_replace_target.take() {
            Some(id) => {
                // 既存スタンプのソース差し替え (長辺サイズ保持、短辺をアスペクト再フィット)。
                if let Some(obj) = objs.iter_mut().find(|o| o.id == id) {
                    if let AnnotationKind::Stamp(s) = &mut obj.kind {
                        let long = s.half_w.max(s.half_h);
                        if aspect >= 1.0 {
                            s.half_w = long;
                            s.half_h = (long / aspect).max(8.0);
                        } else {
                            s.half_h = long;
                            s.half_w = (long * aspect).max(8.0);
                        }
                        s.source = source.clone();
                    }
                }
                self.text_selected = Some(id);
            }
            None => {
                // 画像中心へ新規追加 (長辺 ~ ソース短辺の比率)。
                let (sw, sh) = src_dims;
                let long = (sw.min(sh) * 0.12).clamp(48.0, 800.0); // half-extent
                let (half_w, half_h) = if aspect >= 1.0 {
                    (long, (long / aspect).max(8.0))
                } else {
                    ((long * aspect).max(8.0), long)
                };
                let id = next_id(&objs);
                let z = objs.len() as i32;
                let stamp = StampObject {
                    source: source.clone(),
                    half_w,
                    half_h,
                    ..StampObject::default()
                };
                let mut o = AnnotationObject::new_stamp(id, (sw * 0.5, sh * 0.5), stamp);
                o.z = z;
                objs.push(o);
                self.text_selected = Some(id);
            }
        }
        // MRU 更新 + 永続化。
        crate::comic_stamp::push_recent_stamp(&mut self.recent_stamps, &source);
        crate::comic_stamp::save_recent_stamps(&self.recent_stamps);
        self.save_comic_objects(fs_idx, key, &objs);
        self.text_dirty_at = None;
        self.mark_comic_dirty();
    }
}

// ── 追加ダイアログ用ヘルパー ────────────────────────────────────────────

/// ウィンドウ追加ダイアログのスタイルプリセット。ラボ `system_window_presets` の
/// 見た目 (塗り・枠・角丸・本文色) を移植したもの。名前プレート / 立ち絵 / 指標などの
/// 詳細 (Inc 4d) はここでは持たず、追加後に詳細設定で付ける。
struct WinPreset {
    label: &'static str,
    frame: FrameStyle,
    fill_mode: FillMode,
    fill: Option<Rgba>,
    fill_opacity: f32,
    gradient_to: Option<Rgba>,
    scrim_dense_side: VAnchor,
    outline: Rgba,
    outline_w: f32,
    corner_px: f32,
    text_color: Rgba,
}

const WIN_PRESETS: &[WinPreset] = &[
    WinPreset {
        label: "DQ風 紺枠",
        frame: FrameStyle::DoubleLine,
        fill_mode: FillMode::Solid,
        fill: Some(Rgba::new(12, 18, 52, 255)),
        fill_opacity: 1.0,
        gradient_to: None,
        scrim_dense_side: VAnchor::Center,
        outline: Rgba::WHITE,
        outline_w: 3.0,
        corner_px: 6.0,
        text_color: Rgba::WHITE,
    },
    WinPreset {
        label: "FF風 青グラデ",
        frame: FrameStyle::SolidRounded,
        fill_mode: FillMode::LinearGradient,
        fill: Some(Rgba::new(30, 60, 160, 255)),
        fill_opacity: 1.0,
        gradient_to: Some(Rgba::new(8, 16, 60, 255)),
        scrim_dense_side: VAnchor::Center,
        outline: Rgba::WHITE,
        outline_w: 3.0,
        corner_px: 10.0,
        text_color: Rgba::WHITE,
    },
    WinPreset {
        label: "ツクール窓",
        frame: FrameStyle::SolidRounded,
        fill_mode: FillMode::Solid,
        fill: Some(Rgba::new(20, 24, 40, 235)),
        fill_opacity: 1.0,
        gradient_to: None,
        scrim_dense_side: VAnchor::Center,
        outline: Rgba::new(120, 150, 220, 255),
        outline_w: 3.0,
        corner_px: 10.0,
        text_color: Rgba::WHITE,
    },
    WinPreset {
        label: "ツクール暗幕",
        frame: FrameStyle::None,
        fill_mode: FillMode::GradientScrim,
        fill: Some(Rgba::new(0, 0, 0, 255)),
        fill_opacity: 1.0,
        gradient_to: None,
        scrim_dense_side: VAnchor::Bottom,
        outline: Rgba::WHITE,
        outline_w: 0.0,
        corner_px: 0.0,
        text_color: Rgba::WHITE,
    },
    WinPreset {
        label: "枠なし下部",
        frame: FrameStyle::None,
        fill_mode: FillMode::Translucent,
        fill: Some(Rgba::new(0, 0, 0, 140)),
        fill_opacity: 1.0,
        gradient_to: None,
        scrim_dense_side: VAnchor::Center,
        outline: Rgba::WHITE,
        outline_w: 0.0,
        corner_px: 0.0,
        text_color: Rgba::WHITE,
    },
    WinPreset {
        label: "枠あり下部",
        frame: FrameStyle::SolidRounded,
        fill_mode: FillMode::Translucent,
        fill: Some(Rgba::new(20, 20, 28, 200)),
        fill_opacity: 1.0,
        gradient_to: None,
        scrim_dense_side: VAnchor::Center,
        outline: Rgba::new(220, 220, 230, 255),
        outline_w: 2.0,
        corner_px: 18.0,
        text_color: Rgba::WHITE,
    },
    WinPreset {
        label: "ノベルADV",
        frame: FrameStyle::SolidRounded,
        fill_mode: FillMode::Translucent,
        fill: Some(Rgba::new(10, 12, 24, 190)),
        fill_opacity: 1.0,
        gradient_to: None,
        scrim_dense_side: VAnchor::Center,
        outline: Rgba::new(180, 190, 210, 255),
        outline_w: 2.0,
        corner_px: 14.0,
        text_color: Rgba::WHITE,
    },
    WinPreset {
        label: "ノベルNVL",
        frame: FrameStyle::None,
        fill_mode: FillMode::Translucent,
        fill: Some(Rgba::new(0, 0, 0, 150)),
        fill_opacity: 1.0,
        gradient_to: None,
        scrim_dense_side: VAnchor::Center,
        outline: Rgba::WHITE,
        outline_w: 0.0,
        corner_px: 0.0,
        text_color: Rgba::WHITE,
    },
    WinPreset {
        label: "ノベル白枠",
        frame: FrameStyle::SolidRounded,
        fill_mode: FillMode::Translucent,
        fill: Some(Rgba::new(250, 250, 250, 220)),
        fill_opacity: 1.0,
        gradient_to: None,
        scrim_dense_side: VAnchor::Center,
        outline: Rgba::new(90, 90, 100, 255),
        outline_w: 2.0,
        corner_px: 16.0,
        text_color: Rgba::BLACK,
    },
    WinPreset {
        label: "コミックキャプション",
        frame: FrameStyle::SolidRounded,
        fill_mode: FillMode::Solid,
        fill: Some(Rgba::new(250, 245, 225, 255)),
        fill_opacity: 1.0,
        gradient_to: None,
        scrim_dense_side: VAnchor::Center,
        outline: Rgba::new(40, 40, 40, 255),
        outline_w: 2.0,
        corner_px: 0.0,
        text_color: Rgba::BLACK,
    },
];

/// ウィンドウプリセット 1 セルの幅。
const WIN_CELL_W: f32 = 116.0;

/// ウィンドウプリセットを `area` 内にプレビュー描画 (塗り + 枠 + 本文見立ての線)。
/// ラボ `paint_window_preview` の移植。グラデ / スクリム (濃淡側) / 角丸 / 本文色を反映。
fn paint_winpreset_preview(painter: &egui::Painter, area: egui::Rect, p: &WinPreset) {
    let to_c = |c: Rgba, a: u8| egui::Color32::from_rgba_unmultiplied(c.r, c.g, c.b, a);
    let corner = (p.corner_px * 0.2).clamp(0.0, 10.0);
    if let Some(fill) = p.fill {
        let a = (fill.a as f32 * p.fill_opacity).round().clamp(0.0, 255.0) as u8;
        match p.fill_mode {
            FillMode::None => {}
            FillMode::GradientScrim => match p.scrim_dense_side {
                VAnchor::Center => {
                    let third = area.height() / 3.0;
                    let m1 = area.top() + third;
                    let m2 = area.bottom() - third;
                    painter.rect_filled(
                        egui::Rect::from_min_max(area.min, egui::pos2(area.right(), m1)),
                        0.0,
                        to_c(fill, a / 5),
                    );
                    painter.rect_filled(
                        egui::Rect::from_min_max(
                            egui::pos2(area.left(), m1),
                            egui::pos2(area.right(), m2),
                        ),
                        0.0,
                        to_c(fill, a),
                    );
                    painter.rect_filled(
                        egui::Rect::from_min_max(egui::pos2(area.left(), m2), area.max),
                        0.0,
                        to_c(fill, a / 5),
                    );
                }
                other => {
                    let (top_a, bot_a) = match other {
                        VAnchor::Top => (a, a / 5),
                        _ => (a / 5, a),
                    };
                    let mid = area.center().y;
                    painter.rect_filled(
                        egui::Rect::from_min_max(area.min, egui::pos2(area.right(), mid)),
                        0.0,
                        to_c(fill, top_a),
                    );
                    painter.rect_filled(
                        egui::Rect::from_min_max(egui::pos2(area.left(), mid), area.max),
                        0.0,
                        to_c(fill, bot_a),
                    );
                }
            },
            FillMode::LinearGradient => {
                let to = p.gradient_to.unwrap_or(fill);
                let mid = area.center().y;
                painter.rect_filled(
                    egui::Rect::from_min_max(area.min, egui::pos2(area.right(), mid)),
                    corner,
                    to_c(fill, a),
                );
                painter.rect_filled(
                    egui::Rect::from_min_max(egui::pos2(area.left(), mid), area.max),
                    corner,
                    to_c(to, a),
                );
            }
            _ => {
                // Solid / Translucent。
                painter.rect_filled(area, corner, to_c(fill, a));
            }
        }
    }
    if !matches!(p.frame, FrameStyle::None) {
        let stroke = egui::Stroke::new(
            (p.outline_w * 0.4).clamp(1.0, 3.0),
            to_c(p.outline, p.outline.a),
        );
        painter.rect_stroke(area, corner, stroke, egui::StrokeKind::Inside);
        if matches!(p.frame, FrameStyle::DoubleLine) {
            painter.rect_stroke(
                area.shrink(3.0),
                (corner - 1.0).max(0.0),
                stroke,
                egui::StrokeKind::Inside,
            );
        }
    }
    // 本文見立ての線 2 本 (プリセットの本文色)。
    let line_col = to_c(p.text_color, 210);
    for i in 0..2 {
        let y = area.top() + area.height() * (0.45 + i as f32 * 0.22);
        painter.line_segment(
            [
                egui::pos2(area.left() + 8.0, y),
                egui::pos2(area.right() - 8.0 - i as f32 * 16.0, y),
            ],
            egui::Stroke::new(3.0, line_col),
        );
    }
}

/// ウィンドウプリセットのサムネイル 1 枚 (プレビュー + ラベル)。クリックで true。
fn draw_winpreset_thumbnail(ui: &mut egui::Ui, p: &WinPreset) -> bool {
    const PREVIEW_H: f32 = 60.0;
    ui.vertical(|ui| {
        ui.set_width(WIN_CELL_W);
        let (rect, r) =
            ui.allocate_exact_size(egui::vec2(WIN_CELL_W, PREVIEW_H), egui::Sense::click());
        let hovered = r.hovered();
        let painter = ui.painter_at(rect);
        painter.rect_filled(
            rect,
            4.0,
            if hovered {
                egui::Color32::from_rgb(70, 70, 74)
            } else {
                egui::Color32::from_rgb(46, 46, 50)
            },
        );
        painter.rect_stroke(
            rect,
            4.0,
            egui::Stroke::new(
                1.0,
                if hovered {
                    egui::Color32::from_rgb(150, 195, 255)
                } else {
                    egui::Color32::from_gray(70)
                },
            ),
            egui::StrokeKind::Inside,
        );
        paint_winpreset_preview(&painter, rect.shrink(7.0), p);
        ui.add(egui::Label::new(
            egui::RichText::new(p.label)
                .size(11.0)
                .color(egui::Color32::WHITE),
        ));
        r.clicked()
    })
    .inner
}

/// 吹き出しプリセット 1 セルの幅 (追加ダイアログのグリッド列)。ラボ `PRESET_CELL_W`。
const PRESET_CELL_W: f32 = 96.0;

/// 既定のしっぽ (下左 45°)。ラボ `default_bubble_tail` と同式。
fn default_bubble_tail(pivot: (f32, f32)) -> Tail {
    const TAIL_DIAG: f32 = 106.0;
    Tail {
        tip: (pivot.0 - TAIL_DIAG, pivot.1 + TAIL_DIAG),
        base_t: 0.25,
        base_auto: true,
        width_px: 32.0,
        kind: TailKind::Spike,
    }
}

/// 吹き出しプリセット = 形状 + しっぽ + 塗り + 縁取り + 文字スタイルの束。ラボ
/// `BubblePreset` を移植 (preset 永続化用の sys_slug は省略)。これにより追加ダイアログの
/// プレビューが「実際に焼かれる吹き出し」と一致する。
#[derive(Clone, Copy, PartialEq, Eq)]
enum BubblePreset {
    Normal,
    RoundRect,
    Narration,
    Thought,
    Shout,
    Whisper,
    Soft,
    Polygon,
    Diamond,
    Heart,
    Arrow,
    MotionLines,
    SpeedLines,
    Concentration,
    MindEllipse,
    Strokes,
    DoubleStroke,
    TextOnly,
}

impl BubblePreset {
    const ALL: &'static [BubblePreset] = &[
        BubblePreset::Normal,
        BubblePreset::RoundRect,
        BubblePreset::Soft,
        BubblePreset::Narration,
        BubblePreset::Thought,
        BubblePreset::MindEllipse,
        BubblePreset::Shout,
        BubblePreset::Whisper,
        BubblePreset::Concentration,
        BubblePreset::Polygon,
        BubblePreset::Diamond,
        BubblePreset::Heart,
        BubblePreset::Arrow,
        BubblePreset::MotionLines,
        BubblePreset::SpeedLines,
        BubblePreset::Strokes,
        BubblePreset::DoubleStroke,
        BubblePreset::TextOnly,
    ];

    fn label(self) -> &'static str {
        match self {
            BubblePreset::Normal => "通常",
            BubblePreset::RoundRect => "角丸",
            BubblePreset::Narration => "ナレーション",
            BubblePreset::Thought => "思考",
            BubblePreset::Shout => "叫び",
            BubblePreset::Whisper => "ささやき",
            BubblePreset::Soft => "やわらか",
            BubblePreset::Polygon => "多角形",
            BubblePreset::Diamond => "ダイヤ",
            BubblePreset::Heart => "ハート",
            BubblePreset::Arrow => "矢印",
            BubblePreset::MotionLines => "集中線",
            BubblePreset::SpeedLines => "流線",
            BubblePreset::Concentration => "意識",
            BubblePreset::MindEllipse => "思考(楕円)",
            BubblePreset::Strokes => "線",
            BubblePreset::DoubleStroke => "二重線",
            BubblePreset::TextOnly => "なし",
        }
    }

    fn shape(self) -> BubbleShape {
        match self {
            BubblePreset::Normal | BubblePreset::Whisper => BubbleShape::Ellipse {
                rx: 160.0,
                ry: 100.0,
            },
            BubblePreset::RoundRect => BubbleShape::RoundRect {
                half_w: 160.0,
                half_h: 100.0,
                corner_px: 28.0,
            },
            BubblePreset::Narration => BubbleShape::RoundRect {
                half_w: 170.0,
                half_h: 90.0,
                corner_px: 0.0,
            },
            BubblePreset::Thought => BubbleShape::Cloud {
                rx: 170.0,
                ry: 115.0,
                lobes: 11,
                amp: 0.14,
                shape_seed: 0,
            },
            BubblePreset::Shout => BubbleShape::Burst {
                rx: 170.0,
                ry: 120.0,
                spikes: 20,
                jag: 0.55,
                shape_seed: 1,
            },
            BubblePreset::Soft => BubbleShape::Soft {
                half_w: 165.0,
                half_h: 105.0,
                corner_px: 38.0,
                shape_seed: 0,
            },
            BubblePreset::Polygon => BubbleShape::Polygon {
                rx: 155.0,
                ry: 125.0,
                sides: 6,
            },
            BubblePreset::Diamond => BubbleShape::Diamond {
                half_w: 160.0,
                half_h: 130.0,
            },
            BubblePreset::Heart => BubbleShape::Heart {
                rx: 150.0,
                ry: 140.0,
            },
            BubblePreset::Arrow => BubbleShape::Arrow {
                half_w: 150.0,
                half_h: 110.0,
                dir_rad: -std::f32::consts::FRAC_PI_2,
            },
            BubblePreset::MotionLines => BubbleShape::MotionLines {
                rx: 240.0,
                ry: 180.0,
                count: 72,
                shape_seed: 0,
            },
            BubblePreset::SpeedLines => BubbleShape::SpeedLines {
                half_w: 260.0,
                half_h: 170.0,
                dir_rad: 0.0,
                count: 48,
                shape_seed: 0,
            },
            BubblePreset::Concentration => BubbleShape::Concentration {
                rx: 180.0,
                ry: 120.0,
                shape_seed: 0,
            },
            BubblePreset::MindEllipse => BubbleShape::Ellipse {
                rx: 165.0,
                ry: 110.0,
            },
            BubblePreset::Strokes => BubbleShape::Strokes {
                half_w: 165.0,
                half_h: 105.0,
                corner_px: 36.0,
                shape_seed: 0,
            },
            BubblePreset::DoubleStroke => BubbleShape::DoubleStroke {
                half_w: 165.0,
                half_h: 105.0,
                corner_px: 26.0,
                gap_px: 8.0,
            },
            BubblePreset::TextOnly => BubbleShape::TextOnly {
                half_w: 150.0,
                half_h: 95.0,
            },
        }
    }

    fn tail_kind(self) -> Option<TailKind> {
        match self {
            BubblePreset::Narration
            | BubblePreset::MotionLines
            | BubblePreset::SpeedLines
            | BubblePreset::Concentration
            | BubblePreset::TextOnly
            | BubblePreset::Arrow => None,
            BubblePreset::Thought | BubblePreset::MindEllipse => Some(TailKind::Thought),
            _ => Some(TailKind::Spike),
        }
    }

    fn text_outline(self) -> Option<StrokeStyle> {
        match self {
            BubblePreset::MotionLines | BubblePreset::SpeedLines => Some(StrokeStyle {
                color: Rgba::WHITE,
                width_px: 6.0,
            }),
            _ => None,
        }
    }

    fn outline_width(self) -> f32 {
        match self {
            BubblePreset::Shout => 5.0,
            BubblePreset::Whisper => 1.5,
            _ => 3.0,
        }
    }

    fn text_align(self) -> TextAlign {
        match self {
            BubblePreset::Narration => TextAlign::Start,
            _ => TextAlign::Center,
        }
    }

    fn text_color(self) -> Rgba {
        match self {
            BubblePreset::Whisper => Rgba::new(120, 120, 120, 255),
            _ => Rgba::BLACK,
        }
    }

    /// 新規吹き出しを構築 (縦書き + markup ON、ラボの追加既定に一致)。
    fn build_bubble(self, pivot: (f32, f32), font_key: &str) -> BubbleObject {
        let mut b = BubbleObject {
            shape: self.shape(),
            fill: Some(Rgba::WHITE),
            fill_opacity: 1.0,
            outline: StrokeStyle {
                color: Rgba::BLACK,
                width_px: self.outline_width(),
            },
            tail: None,
            padding_px: 16.0,
            decorations: Vec::new(),
            text: TextBlock {
                text: "セリフ".to_string(),
                font_key: font_key.to_string(),
                size_px: 40.0,
                color: self.text_color(),
                align: self.text_align(),
                orientation: Orientation::Vertical,
                markup_enabled: true,
                outline: self.text_outline(),
                ..TextBlock::default()
            },
            auto_size: true,
            merge_with_below: false,
            shape_preset_link: None,
        };
        if let Some(kind) = self.tail_kind() {
            let mut tail = default_bubble_tail(pivot);
            tail.kind = kind;
            b.tail = Some(tail);
        }
        b
    }
}

/// 吹き出しプリセットを `area` 内に縮小フィットで描く (塗り + 統合アウトライン +
/// しっぽ)。comic-core ジオメトリを使うので焼き上がりと一致。ラボ `paint_bubble_preview`。
fn paint_bubble_preview(painter: &egui::Painter, area: egui::Rect, preset: BubblePreset) {
    use egui::{Color32, Pos2};
    let pivot = (0.0f32, 0.0f32);
    let shape = preset.shape();

    // なし: 箱なし — テキスト見立ての線 2 本。
    if matches!(shape, BubbleShape::TextOnly { .. }) {
        let line_col = Color32::from_gray(210);
        for i in 0..2 {
            let y = area.top() + area.height() * (0.40 + i as f32 * 0.24);
            painter.line_segment(
                [
                    egui::pos2(area.left() + 10.0, y),
                    egui::pos2(area.right() - 10.0 - i as f32 * 14.0, y),
                ],
                egui::Stroke::new(3.0, line_col),
            );
        }
        return;
    }

    const CLEAR: f32 = 0.55; // comic_core::LINE_FIELD_CLEAR_RATIO 相当
    let line_col = Color32::from_gray(210);
    if matches!(shape, BubbleShape::MotionLines { .. }) {
        let (cx, cy) = (area.center().x, area.center().y);
        let (rx, ry) = (area.width() * 0.46, area.height() * 0.46);
        let n = 22;
        for i in 0..n {
            let a = i as f32 / n as f32 * std::f32::consts::TAU;
            let (c, s) = (a.cos(), a.sin());
            painter.line_segment(
                [
                    egui::pos2(cx + rx * CLEAR * c, cy + ry * CLEAR * s),
                    egui::pos2(cx + rx * c, cy + ry * s),
                ],
                egui::Stroke::new(1.4, line_col),
            );
        }
        return;
    }
    if matches!(shape, BubbleShape::SpeedLines { .. }) {
        let (cx, cy) = (area.center().x, area.center().y);
        let (rx, ry) = (area.width() * 0.47, area.height() * 0.44);
        let n = 9;
        for i in 0..n {
            let f = i as f32 / (n as f32 - 1.0) * 2.0 - 1.0;
            let yoff = f * ry;
            let outer_k = 1.0 - (yoff / ry).powi(2);
            if outer_k <= 0.0 {
                continue;
            }
            let half = rx * outer_k.sqrt();
            let y = cy + yoff;
            let clear_k = (CLEAR * CLEAR) - (yoff / ry).powi(2);
            let gap = if clear_k > 0.0 {
                rx * clear_k.sqrt()
            } else {
                0.0
            };
            if gap > 0.0 {
                painter.line_segment(
                    [egui::pos2(cx - half, y), egui::pos2(cx - gap, y)],
                    egui::Stroke::new(1.4, line_col),
                );
                painter.line_segment(
                    [egui::pos2(cx + gap, y), egui::pos2(cx + half, y)],
                    egui::Stroke::new(1.4, line_col),
                );
            } else {
                painter.line_segment(
                    [egui::pos2(cx - half, y), egui::pos2(cx + half, y)],
                    egui::Stroke::new(1.4, line_col),
                );
            }
        }
        return;
    }
    if let BubbleShape::Concentration { .. } = shape {
        let c = area.center();
        let r = egui::vec2(area.width() * 0.44, area.height() * 0.44);
        painter.add(egui::Shape::ellipse_filled(
            c,
            r,
            Color32::from_rgba_unmultiplied(255, 255, 255, 150),
        ));
        painter.add(egui::Shape::ellipse_stroke(
            c,
            r,
            egui::Stroke::new(1.2, Color32::from_gray(180)),
        ));
        return;
    }

    let tail = preset.tail_kind().map(|kind| Tail {
        tip: (-70.0, 200.0),
        base_t: 0.30,
        base_auto: true,
        width_px: 60.0,
        kind,
    });
    let geo = comic_core::bubble_geometry(&shape, pivot, tail.as_ref());

    let mut min = (f32::INFINITY, f32::INFINITY);
    let mut max = (f32::NEG_INFINITY, f32::NEG_INFINITY);
    let mut grow = |x: f32, y: f32| {
        min.0 = min.0.min(x);
        min.1 = min.1.min(y);
        max.0 = max.0.max(x);
        max.1 = max.1.max(y);
    };
    for &(x, y) in &geo.outline {
        grow(x, y);
    }
    for &(cx, cy, r) in &geo.thought {
        grow(cx - r, cy - r);
        grow(cx + r, cy + r);
    }
    if !min.0.is_finite() {
        return;
    }
    let w = (max.0 - min.0).max(1.0);
    let h = (max.1 - min.1).max(1.0);
    let scale = (area.width() / w).min(area.height() / h);
    let cx = (min.0 + max.0) * 0.5;
    let cy = (min.1 + max.1) * 0.5;
    let map = |p: (f32, f32)| -> Pos2 {
        Pos2::new(
            area.center().x + (p.0 - cx) * scale,
            area.center().y + (p.1 - cy) * scale,
        )
    };

    let fill = Color32::WHITE;
    let stroke = egui::Stroke::new((preset.outline_width() * scale).max(1.0), Color32::BLACK);

    // 統合アウトラインを pivot からの三角形ファンで塗る (concave でも star-shaped)。
    let center = map(pivot);
    let outline: Vec<Pos2> = geo.outline.iter().map(|&p| map(p)).collect();
    if outline.len() >= 3 {
        let mut mesh = egui::Mesh::default();
        mesh.colored_vertex(center, fill);
        for &p in &outline {
            mesh.colored_vertex(p, fill);
        }
        let n = outline.len() as u32;
        for i in 0..n {
            let a = 1 + i;
            let b = 1 + (i + 1) % n;
            mesh.add_triangle(0, a, b);
        }
        painter.add(egui::Shape::mesh(mesh));
        painter.add(egui::Shape::closed_line(outline, stroke));
    }
    // 二重線: 内側の同心リング。
    if let BubbleShape::DoubleStroke {
        half_w,
        half_h,
        corner_px,
        gap_px,
    } = shape
    {
        let g = gap_px.max(1.0);
        let inner_shape = BubbleShape::RoundRect {
            half_w: (half_w - g).max(1.0),
            half_h: (half_h - g).max(1.0),
            corner_px: (corner_px - g).max(0.0),
        };
        let inner: Vec<Pos2> = comic_core::tessellate_bubble(&inner_shape, pivot)
            .iter()
            .map(|&p| map(p))
            .collect();
        if inner.len() >= 3 {
            painter.add(egui::Shape::closed_line(inner, stroke));
        }
    }
    // 思考のしっぽ円 (塗り + 縁取り)。
    for &(tcx, tcy, r) in &geo.thought {
        let c = map((tcx, tcy));
        painter.circle(c, r * scale, fill, stroke);
    }
}

/// 吹き出しプリセットのサムネイル 1 枚 (描画プレビュー + ラベル)。クリックで true。
fn draw_bubble_preset_thumbnail(ui: &mut egui::Ui, preset: BubblePreset) -> bool {
    const PREVIEW_H: f32 = 64.0;
    ui.vertical(|ui| {
        ui.set_width(PRESET_CELL_W);
        let (rect, r) =
            ui.allocate_exact_size(egui::vec2(PRESET_CELL_W, PREVIEW_H), egui::Sense::click());
        let hovered = r.hovered();
        let painter = ui.painter_at(rect);
        painter.rect_filled(
            rect,
            4.0,
            if hovered {
                egui::Color32::from_rgb(60, 60, 64)
            } else {
                egui::Color32::from_rgb(40, 40, 44)
            },
        );
        painter.rect_stroke(
            rect,
            4.0,
            egui::Stroke::new(
                1.0,
                if hovered {
                    egui::Color32::from_rgb(150, 195, 255)
                } else {
                    egui::Color32::from_gray(70)
                },
            ),
            egui::StrokeKind::Inside,
        );
        paint_bubble_preview(&painter, rect.shrink(8.0), preset);
        ui.add(egui::Label::new(
            egui::RichText::new(preset.label())
                .size(11.0)
                .color(egui::Color32::WHITE),
        ));
        r.clicked()
    })
    .inner
}

// ── オノマトペプリセット (Inc 4c、ラボ ONOMATOPOEIA_PRESETS 準拠) ───────────
//
// 各プリセットは装飾フォント (追加パック同梱の OFL フォント) を `font_candidate` で
// 指定し、`resolve_onomatopoeia_font` が実フォント名へ解決する。サイズはラボの ~760px
// 基準値で、追加時にソース解像度へ比例スケールする。

/// オノマトペ追加ピッカーの 1 セル幅 (実フォントサンプルを大きめに見せる)。
const ONOMATO_CELL_W: f32 = 184.0;

#[derive(Clone, Copy)]
struct OnomatopoeiaPreset {
    label: &'static str,
    text: &'static str,
    font_candidate: &'static str,
    size_px: f32,
    color: Rgba,
    orientation: Orientation,
    letter_gap: f32,
    outline: Option<StrokeStyle>,
    rotation_deg: f32,
}

const ONOMATOPOEIA_PRESETS: &[OnomatopoeiaPreset] = &[
    OnomatopoeiaPreset {
        label: "Otomanopee One",
        text: "ドンッ",
        font_candidate: "Otomanopee One",
        size_px: 92.0,
        color: Rgba::BLACK,
        orientation: Orientation::Horizontal,
        letter_gap: 0.0,
        outline: Some(StrokeStyle {
            color: Rgba::WHITE,
            width_px: 8.0,
        }),
        rotation_deg: -8.0,
    },
    OnomatopoeiaPreset {
        label: "Dela Gothic One",
        text: "ガーン",
        font_candidate: "Dela Gothic One",
        size_px: 92.0,
        color: Rgba::new(44, 78, 190, 255),
        orientation: Orientation::Horizontal,
        letter_gap: 0.0,
        outline: Some(StrokeStyle {
            color: Rgba::WHITE,
            width_px: 7.0,
        }),
        rotation_deg: 5.0,
    },
    OnomatopoeiaPreset {
        label: "Reggae One",
        text: "ゴゴゴ",
        font_candidate: "Reggae One",
        size_px: 78.0,
        color: Rgba::new(42, 28, 72, 255),
        orientation: Orientation::Vertical,
        letter_gap: 2.0,
        outline: Some(StrokeStyle {
            color: Rgba::new(218, 196, 255, 255),
            width_px: 5.0,
        }),
        rotation_deg: 0.0,
    },
    OnomatopoeiaPreset {
        label: "RocknRoll One",
        text: "ザザッ",
        font_candidate: "RocknRoll One",
        size_px: 82.0,
        color: Rgba::WHITE,
        orientation: Orientation::Horizontal,
        letter_gap: 0.0,
        outline: Some(StrokeStyle {
            color: Rgba::BLACK,
            width_px: 7.0,
        }),
        rotation_deg: -10.0,
    },
    OnomatopoeiaPreset {
        label: "Rampart One",
        text: "バァン",
        font_candidate: "Rampart One",
        size_px: 88.0,
        color: Rgba::new(255, 216, 64, 255),
        orientation: Orientation::Horizontal,
        letter_gap: 0.0,
        outline: Some(StrokeStyle {
            color: Rgba::BLACK,
            width_px: 4.0,
        }),
        rotation_deg: 6.0,
    },
    OnomatopoeiaPreset {
        label: "Stick",
        text: "シュッ",
        font_candidate: "Stick",
        size_px: 84.0,
        color: Rgba::WHITE,
        orientation: Orientation::Horizontal,
        letter_gap: 0.0,
        outline: Some(StrokeStyle {
            color: Rgba::BLACK,
            width_px: 6.0,
        }),
        rotation_deg: -14.0,
    },
    OnomatopoeiaPreset {
        label: "Train One",
        text: "ビューン",
        font_candidate: "Train One",
        size_px: 78.0,
        color: Rgba::new(90, 230, 255, 255),
        orientation: Orientation::Horizontal,
        letter_gap: 0.0,
        outline: Some(StrokeStyle {
            color: Rgba::new(12, 40, 70, 255),
            width_px: 5.0,
        }),
        rotation_deg: -12.0,
    },
    OnomatopoeiaPreset {
        label: "DotGothic16",
        text: "ピコ",
        font_candidate: "DotGothic16",
        size_px: 64.0,
        color: Rgba::new(125, 255, 110, 255),
        orientation: Orientation::Horizontal,
        letter_gap: 0.0,
        outline: Some(StrokeStyle {
            color: Rgba::new(20, 55, 25, 255),
            width_px: 4.0,
        }),
        rotation_deg: 0.0,
    },
    OnomatopoeiaPreset {
        label: "Hachi Maru Pop",
        text: "わーい",
        font_candidate: "Hachi Maru Pop",
        size_px: 74.0,
        color: Rgba::new(255, 140, 45, 255),
        orientation: Orientation::Horizontal,
        letter_gap: 0.0,
        outline: Some(StrokeStyle {
            color: Rgba::WHITE,
            width_px: 6.0,
        }),
        rotation_deg: -4.0,
    },
    OnomatopoeiaPreset {
        label: "Darumadrop One",
        text: "ぽよん",
        font_candidate: "Darumadrop One",
        size_px: 78.0,
        color: Rgba::new(245, 70, 145, 255),
        orientation: Orientation::Horizontal,
        letter_gap: 0.0,
        outline: Some(StrokeStyle {
            color: Rgba::WHITE,
            width_px: 6.0,
        }),
        rotation_deg: 6.0,
    },
    OnomatopoeiaPreset {
        label: "Yusei Magic",
        text: "キラキラ",
        font_candidate: "Yusei Magic",
        size_px: 68.0,
        color: Rgba::new(255, 230, 72, 255),
        orientation: Orientation::Horizontal,
        letter_gap: 0.0,
        outline: Some(StrokeStyle {
            color: Rgba::new(92, 62, 0, 255),
            width_px: 5.0,
        }),
        rotation_deg: 4.0,
    },
    OnomatopoeiaPreset {
        label: "Klee One SemiBold",
        text: "しーん",
        font_candidate: "Klee One SemiBold",
        size_px: 58.0,
        color: Rgba::new(122, 126, 132, 255),
        orientation: Orientation::Horizontal,
        letter_gap: 7.0,
        outline: None,
        rotation_deg: 0.0,
    },
    OnomatopoeiaPreset {
        label: "Kaisei Decol Bold",
        text: "ふわっ",
        font_candidate: "Kaisei Decol Bold",
        size_px: 64.0,
        color: Rgba::new(245, 220, 255, 255),
        orientation: Orientation::Horizontal,
        letter_gap: 0.0,
        outline: Some(StrokeStyle {
            color: Rgba::new(74, 40, 120, 255),
            width_px: 4.0,
        }),
        rotation_deg: -3.0,
    },
    OnomatopoeiaPreset {
        label: "Zen Kurenaido",
        text: "ひそ",
        font_candidate: "Zen Kurenaido",
        size_px: 50.0,
        color: Rgba::new(150, 150, 155, 255),
        orientation: Orientation::Horizontal,
        letter_gap: 3.0,
        outline: Some(StrokeStyle {
            color: Rgba::WHITE,
            width_px: 3.0,
        }),
        rotation_deg: 0.0,
    },
    OnomatopoeiaPreset {
        label: "Kaisei Tokumin ExtraBold",
        text: "ズシン",
        font_candidate: "Kaisei Tokumin ExtraBold",
        size_px: 86.0,
        color: Rgba::BLACK,
        orientation: Orientation::Horizontal,
        letter_gap: 0.0,
        outline: Some(StrokeStyle {
            color: Rgba::WHITE,
            width_px: 6.0,
        }),
        rotation_deg: 3.0,
    },
    OnomatopoeiaPreset {
        label: "Zen Maru Gothic Black",
        text: "ぷにっ",
        font_candidate: "Zen Maru Gothic Black",
        size_px: 76.0,
        color: Rgba::new(130, 235, 190, 255),
        orientation: Orientation::Horizontal,
        letter_gap: 0.0,
        outline: Some(StrokeStyle {
            color: Rgba::new(18, 76, 62, 255),
            width_px: 5.0,
        }),
        rotation_deg: -4.0,
    },
    OnomatopoeiaPreset {
        label: "M PLUS 1",
        text: "バシッ",
        font_candidate: "M PLUS 1",
        size_px: 82.0,
        color: Rgba::BLACK,
        orientation: Orientation::Horizontal,
        letter_gap: 0.0,
        outline: Some(StrokeStyle {
            color: Rgba::WHITE,
            width_px: 7.0,
        }),
        rotation_deg: -8.0,
    },
    OnomatopoeiaPreset {
        label: "Shippori Mincho Bold",
        text: "ぞくっ",
        font_candidate: "Shippori Mincho Bold",
        size_px: 70.0,
        color: Rgba::new(38, 48, 86, 255),
        orientation: Orientation::Horizontal,
        letter_gap: 0.0,
        outline: Some(StrokeStyle {
            color: Rgba::WHITE,
            width_px: 4.0,
        }),
        rotation_deg: -2.0,
    },
];

impl OnomatopoeiaPreset {
    /// プリセットから標準テキストブロックを作る (font_key は解決済みを渡す)。
    fn build_text(self, font_key: &str) -> TextBlock {
        TextBlock {
            text: self.text.to_string(),
            font_key: font_key.to_string(),
            size_px: self.size_px,
            color: self.color,
            orientation: self.orientation,
            align: TextAlign::Center,
            line_gap: 0.0,
            letter_gap: self.letter_gap,
            outline: self.outline,
            auto_tcy: false,
            ..TextBlock::default()
        }
    }
}

/// オノマトペサムネイルが焼けないとき (フォント未ロード等) の簡易プレビュー。egui の
/// proportional UI フォントで袋文字風に描く (実フォントの形は出ないがレイアウトは伝わる)。
fn paint_onomatopoeia_preview(
    painter: &egui::Painter,
    area: egui::Rect,
    preset: OnomatopoeiaPreset,
) {
    let fill = to_c32(preset.color);
    let outline = preset
        .outline
        .map(|s| to_c32(s.color))
        .unwrap_or(egui::Color32::from_black_alpha(0));
    let center = area.center();
    let font_size = if preset.text.chars().count() >= 4 {
        24.0
    } else {
        28.0
    };
    let font_id = egui::FontId::proportional(font_size);
    let text = preset.text;
    if preset.outline.is_some() {
        for (dx, dy) in [
            (-2.0, 0.0),
            (2.0, 0.0),
            (0.0, -2.0),
            (0.0, 2.0),
            (-1.5, -1.5),
            (1.5, -1.5),
            (-1.5, 1.5),
            (1.5, 1.5),
        ] {
            painter.text(
                center + egui::vec2(dx, dy),
                egui::Align2::CENTER_CENTER,
                text,
                font_id.clone(),
                outline,
            );
        }
    }
    painter.text(center, egui::Align2::CENTER_CENTER, text, font_id, fill);
}

/// オノマトペプリセットの 1 カード。上に実フォントで焼いたプレビュー (無ければ簡易
/// プレビュー)、下にフォント名ラベル。クリックで true。`tex` は呼び出し側で事前構築。
fn draw_onomatopoeia_card(
    ui: &mut egui::Ui,
    preset: OnomatopoeiaPreset,
    tex: Option<&egui::TextureHandle>,
) -> bool {
    const PREVIEW_H: f32 = 104.0;
    const LABEL_H: f32 = 30.0;
    const PAD: f32 = 7.0;
    let (rect, resp) = ui.allocate_exact_size(
        egui::vec2(ONOMATO_CELL_W, PREVIEW_H + LABEL_H),
        egui::Sense::click(),
    );
    let hovered = resp.hovered();
    let painter = ui.painter_at(rect);
    painter.rect_filled(
        rect,
        4.0,
        if hovered {
            egui::Color32::from_rgb(66, 66, 70)
        } else {
            egui::Color32::from_rgb(42, 42, 46)
        },
    );
    painter.rect_stroke(
        rect,
        4.0,
        egui::Stroke::new(
            1.0,
            if hovered {
                egui::Color32::from_rgb(150, 195, 255)
            } else {
                egui::Color32::from_gray(70)
            },
        ),
        egui::StrokeKind::Inside,
    );
    let preview_area = egui::Rect::from_min_max(
        rect.min + egui::vec2(PAD, PAD),
        egui::pos2(rect.max.x - PAD, rect.min.y + PREVIEW_H - 5.0),
    );
    painter.rect_filled(preview_area, 3.0, egui::Color32::from_rgb(34, 34, 38));
    if let Some(tex) = tex {
        let sz = tex.size_vec2();
        if sz.x > 0.0 && sz.y > 0.0 {
            let scale = (preview_area.width() / sz.x)
                .min(preview_area.height() / sz.y)
                .min(1.45);
            let draw = egui::vec2(sz.x * scale, sz.y * scale);
            let origin = preview_area.center() - draw * 0.5;
            painter.image(
                tex.id(),
                egui::Rect::from_min_size(origin, draw),
                egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
                egui::Color32::WHITE,
            );
        }
    } else {
        paint_onomatopoeia_preview(&painter, preview_area.shrink(6.0), preset);
    }
    let label_area = egui::Rect::from_min_max(
        egui::pos2(rect.min.x + 6.0, rect.max.y - LABEL_H),
        egui::pos2(rect.max.x - 6.0, rect.max.y - 3.0),
    );
    painter.text(
        label_area.center(),
        egui::Align2::CENTER_CENTER,
        preset.label,
        egui::FontId::proportional(11.0),
        egui::Color32::WHITE,
    );
    resp.clicked()
}

// ── パネル UI ヘルパー (self を借りない純 UI 関数) ──────────────────────

/// オブジェクト一覧の行部分 (選択ハイライト + 表示トグル)。補正レイヤーパネルと同じ見た目。
/// 共通操作行 (↑ ↓ 複製 削除) は `object_list_actions_ui` に分離し、ScrollArea の外へ
/// 固定表示する (一覧だけがスクロールし、操作ボタンは常に見える)。
fn object_list_rows_ui(
    ui: &mut egui::Ui,
    objects: &mut Vec<AnnotationObject>,
    selected: &mut Option<u64>,
    changed: &mut bool,
) {
    ui.label(
        egui::RichText::new(format!("オブジェクト ({})", objects.len()))
            .strong()
            .color(egui::Color32::WHITE),
    );
    if objects.is_empty() {
        ui.label(
            egui::RichText::new("「追加」からテキスト・吹き出し・ウィンドウを作成してください。")
                .small()
                .color(egui::Color32::from_gray(175)),
        );
        return;
    }

    let row_w = PANEL_W - 12.0;
    let mut clicked: Option<u64> = None;
    let mut toggle_enabled: Option<(usize, bool)> = None;
    // 配列の末尾ほど前面 (z 大)。前面のものを上に見せると直感的なので逆順で描く。
    for i in (0..objects.len()).rev() {
        let o = &objects[i];
        let id = o.id;
        let is_sel = *selected == Some(id);
        let label = format!("{}: {}", i + 1, kind_label(o));
        let text_color = if o.enabled {
            egui::Color32::WHITE
        } else {
            egui::Color32::from_gray(140)
        };
        let frame = egui::Frame::new()
            .fill(if is_sel {
                egui::Color32::from_rgba_unmultiplied(58, 96, 150, 170)
            } else {
                egui::Color32::from_rgba_unmultiplied(52, 52, 54, 120)
            })
            .stroke(egui::Stroke::new(
                1.0,
                if is_sel {
                    egui::Color32::from_rgba_unmultiplied(150, 195, 255, 130)
                } else {
                    egui::Color32::from_rgba_unmultiplied(255, 255, 255, 24)
                },
            ))
            .corner_radius(4.0)
            .inner_margin(6.0)
            .show(ui, |ui| {
                ui.set_min_width(row_w);
                let mut row_clicked = false;
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 6.0;
                    let mut enabled = o.enabled;
                    if ui
                        .checkbox(&mut enabled, "")
                        .on_hover_text("表示")
                        .changed()
                    {
                        toggle_enabled = Some((i, enabled));
                    }
                    if ui
                        .add(
                            egui::Label::new(egui::RichText::new(label).color(text_color))
                                .sense(egui::Sense::click()),
                        )
                        .on_hover_cursor(egui::CursorIcon::PointingHand)
                        .clicked()
                    {
                        row_clicked = true;
                    }
                    let spacer_w = ui.available_width().max(0.0);
                    if spacer_w > 4.0 {
                        let (_, r) = ui
                            .allocate_exact_size(egui::vec2(spacer_w, 18.0), egui::Sense::click());
                        if r.on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
                            row_clicked = true;
                        }
                    }
                });
                row_clicked
            });
        if frame.inner {
            clicked = Some(id);
        }
    }
    if let Some((i, en)) = toggle_enabled {
        objects[i].enabled = en;
        *changed = true;
    }
    if let Some(id) = clicked {
        *selected = Some(id);
    }
}

/// 一覧の共通操作行 (↑ ↓ 複製 削除)。`object_list_rows_ui` の ScrollArea とは別に
/// パネル下部へ固定表示する (スクロールしても操作ボタンが常に見えるように)。
/// 操作は「選択中オブジェクト」に対して行う。
fn object_list_actions_ui(
    ui: &mut egui::Ui,
    objects: &mut Vec<AnnotationObject>,
    selected: &mut Option<u64>,
    changed: &mut bool,
) {
    let row_w = PANEL_W - 12.0;
    // ── 共通操作行 (選択中オブジェクトに作用) ──
    let sel_idx = selected.and_then(|id| objects.iter().position(|o| o.id == id));
    ui.add_space(4.0);
    let mut move_up: Option<usize> = None;
    let mut move_down: Option<usize> = None;
    let mut duplicate: Option<usize> = None;
    let mut delete: Option<usize> = None;
    ui.horizontal(|ui| {
        let gap = 4.0;
        ui.spacing_mut().item_spacing.x = gap;
        let unit_w = ((row_w - gap * 3.0) / 6.0).max(24.0);
        let small_btn = egui::vec2(unit_w, 22.0);
        let wide_btn = egui::vec2(unit_w * 2.0, 22.0);
        let can_front = sel_idx.is_some_and(|i| i + 1 < objects.len());
        let can_back = sel_idx.is_some_and(|i| i > 0);
        let has_sel = sel_idx.is_some();
        if ui
            .add_enabled(can_front, egui::Button::new("↑").min_size(small_btn))
            .on_hover_text("前面へ")
            .clicked()
        {
            move_up = sel_idx;
        }
        if ui
            .add_enabled(can_back, egui::Button::new("↓").min_size(small_btn))
            .on_hover_text("背面へ")
            .clicked()
        {
            move_down = sel_idx;
        }
        if ui
            .add_enabled(has_sel, egui::Button::new("複製").min_size(wide_btn))
            .clicked()
        {
            duplicate = sel_idx;
        }
        if ui
            .add_enabled(
                has_sel,
                egui::Button::new("削除")
                    .min_size(wide_btn)
                    .fill(egui::Color32::from_rgb(120, 50, 50)),
            )
            .clicked()
        {
            delete = sel_idx;
        }
    });

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
/// 右詳細パネルの本体。選択オブジェクトを「カラーの縦線での分類」(カテゴリタブ +
/// 色帯) で編集する。`tab` はアプリ単位の選択状態で、オブジェクト種別に応じて
/// 有効タブへ正規化する。
fn edit_object_ui(
    ui: &mut egui::Ui,
    o: &mut AnnotationObject,
    tab: &mut TextPropTab,
    changed: &mut bool,
    open_stamp_replace: &mut bool,
) {
    ui.strong(kind_label(o));
    // 回転 (全種共通、タブの外)。
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
            // テキストは セリフ カテゴリのみ。タブ行は出さず色帯だけ付ける。
            *tab = TextPropTab::Serifu;
            ui.add_space(4.0);
            draw_section_bar(ui, TextPropTab::Serifu.color(), |ui| {
                text_block_ui(ui, t, changed, true);
            });
        }
        AnnotationKind::Bubble(b) => {
            // セリフ / 本体 / しっぽ。しっぽタブには表示 on/off も入れるので常時有効
            // (しっぽが無い吹き出しでも中の checkbox から付与できる)。
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing = egui::vec2(4.0, 4.0);
                for t in [TextPropTab::Serifu, TextPropTab::Body, TextPropTab::Tail] {
                    if prop_tab_button(ui, t, *tab == t, true, t.label()) {
                        *tab = t;
                    }
                }
            });
            ui.add_space(4.0);
            let cur = *tab;
            draw_section_bar(ui, cur.color(), |ui| match cur {
                TextPropTab::Serifu => text_block_ui(ui, &mut b.text, changed, true),
                TextPropTab::Tail => bubble_tail_ui(ui, b, changed),
                TextPropTab::Body => bubble_body_ui(ui, b, changed),
            });
        }
        AnnotationKind::MessageWindow(w) => {
            // セリフ / 枠 (= Body)。しっぽは無いので Body へ正規化する。
            if *tab == TextPropTab::Tail {
                *tab = TextPropTab::Body;
            }
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing = egui::vec2(4.0, 4.0);
                for (t, lbl) in [(TextPropTab::Serifu, "セリフ"), (TextPropTab::Body, "枠")] {
                    if prop_tab_button(ui, t, *tab == t, true, lbl) {
                        *tab = t;
                    }
                }
            });
            ui.add_space(4.0);
            let cur = *tab;
            draw_section_bar(ui, cur.color(), |ui| match cur {
                TextPropTab::Serifu => text_block_ui(ui, &mut w.text, changed, true),
                _ => window_body_ui(ui, w, changed),
            });
        }
        AnnotationKind::Stamp(s) => {
            // スタンプは画像プロパティのみ (タブなし)。
            stamp_ui(ui, s, changed, open_stamp_replace);
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

/// 吹き出し「本体」タブ (形状・塗り・輪郭・自動サイズ・余白)。本文は セリフ タブ、
/// しっぽは しっぽ タブへ分離している。
fn bubble_body_ui(ui: &mut egui::Ui, b: &mut BubbleObject, changed: &mut bool) {
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
}

/// 吹き出し「しっぽ」タブ (表示 on/off + 種別 + 幅)。
fn bubble_tail_ui(ui: &mut egui::Ui, b: &mut BubbleObject, changed: &mut bool) {
    let mut has_tail = b.tail.is_some();
    if ui.checkbox(&mut has_tail, "しっぽを表示").changed() {
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
        });
        if ui
            .add(egui::Slider::new(&mut t.width_px, 4.0..=120.0).text("幅"))
            .changed()
        {
            *changed = true;
        }
    } else {
        ui.label(
            egui::RichText::new("「しっぽを表示」で会話/思考のしっぽを付けられます")
                .small()
                .color(egui::Color32::from_gray(160)),
        );
    }
}

/// メッセージウィンドウ「枠」タブ (枠・塗り・輪郭)。本文は セリフ タブへ分離。
fn window_body_ui(ui: &mut egui::Ui, w: &mut MessageWindowObject, changed: &mut bool) {
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
}

/// スタンプの編集 (画像 = 大きさ / 不透明度 / 反転 / ステッカー風縁取り + ソース差し替え)。
/// `open_stamp_replace` は「別のスタンプに変更」が押されたときに立て、呼び出し側が
/// 絵文字ピッカーを差し替えモードで開く (ラボ `draw_stamp_properties` 相当)。
fn stamp_ui(
    ui: &mut egui::Ui,
    s: &mut StampObject,
    changed: &mut bool,
    open_stamp_replace: &mut bool,
) {
    ui.label(format!(
        "画像: {}",
        crate::comic_stamp::stamp_label(&s.source)
    ));
    if ui.button("別のスタンプに変更…").clicked() {
        *open_stamp_replace = true;
    }
    ui.separator();

    // 大きさ (長辺 px、アスペクト保持)。
    let aspect = if s.half_h > 1e-3 {
        s.half_w / s.half_h
    } else {
        1.0
    };
    let mut long = s.half_w.max(s.half_h) * 2.0;
    ui.horizontal(|ui| {
        ui.label("大きさ");
        if ui
            .add(egui::Slider::new(&mut long, 16.0..=1600.0).suffix("px"))
            .changed()
        {
            let half_long = (long * 0.5).max(8.0);
            if aspect >= 1.0 {
                s.half_w = half_long;
                s.half_h = (half_long / aspect).max(8.0);
            } else {
                s.half_h = half_long;
                s.half_w = (half_long * aspect).max(8.0);
            }
            *changed = true;
        }
    });
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
    // ステッカー風縁取り (シルエットを膨張させて背面に敷く)。
    let mut has_outline = s.outline.is_some();
    if ui
        .checkbox(&mut has_outline, "縁取り (ステッカー風)")
        .changed()
    {
        s.outline = if has_outline {
            Some(StrokeStyle {
                color: Rgba::WHITE,
                width_px: 6.0,
            })
        } else {
            None
        };
        *changed = true;
    }
    if let Some(o) = &mut s.outline {
        ui.horizontal(|ui| {
            ui.label("縁色");
            let mut col = to_c32(o.color);
            if ui.color_edit_button_srgba(&mut col).changed() {
                o.color = from_c32(col);
                *changed = true;
            }
            if ui
                .add(egui::Slider::new(&mut o.width_px, 0.0..=40.0).text("太さ"))
                .changed()
            {
                *changed = true;
            }
        });
    }
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
    fn onomatopoeia_preset_builds_text_block_style() {
        let preset = ONOMATOPOEIA_PRESETS
            .iter()
            .find(|p| p.font_candidate == "Otomanopee One")
            .copied()
            .expect("Otomanopee preset exists");
        let tb = preset.build_text("OtomanopeeOne Regular");
        assert_eq!(tb.text, "ドンッ");
        assert_eq!(tb.font_key, "OtomanopeeOne Regular");
        assert_eq!(tb.align, TextAlign::Center);
        assert!(tb.outline.is_some());
        assert!(!tb.auto_tcy, "オノマトペは自動縦中横を無効にする");
        assert!(tb.size_px > 0.0);
    }

    #[test]
    fn onomatopoeia_presets_have_candidates() {
        assert!(!ONOMATOPOEIA_PRESETS.is_empty());
        for p in ONOMATOPOEIA_PRESETS {
            assert!(
                !p.font_candidate.is_empty(),
                "{} に候補フォント無し",
                p.label
            );
            assert!(!p.text.is_empty(), "{} にテキスト無し", p.label);
            // 候補は実フォント名 (ファイル名由来ラベル) に区切り無視で一致するはず。
            // 代表ケースをスポット検証する。
        }
        // 既知の対応関係 (pack ファイル名 → 候補)。
        assert!(crate::font_assets::font_name_matches_candidate(
            "MPLUS1 wght",
            "M PLUS 1"
        ));
        assert!(crate::font_assets::font_name_matches_candidate(
            "KleeOne SemiBold",
            "Klee One SemiBold"
        ));
    }

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
            scale: fit,
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

    fn stamp_obj(id: u64, pivot: (f32, f32), half_w: f32, half_h: f32) -> AnnotationObject {
        AnnotationObject::new_stamp(
            id,
            pivot,
            StampObject {
                half_w,
                half_h,
                ..StampObject::default()
            },
        )
    }

    #[test]
    fn rotate_drag_sets_rotation_about_center() {
        // フォント非依存のスタンプで回転ノブの数式を検証。
        let mut objs = vec![stamp_obj(1, (100.0, 100.0), 50.0, 30.0)];
        let drag = TextDrag {
            id: 1,
            kind: TextDragKind::Rotate,
            start: egui::pos2(0.0, 0.0),
            last_img: (0.0, 0.0),
            armed: true,
            moved: false,
        };
        // 中心の真右 → atan2(0,+) + π/2 = π/2。
        assert!(apply_text_drag(&mut objs, &drag, (200.0, 100.0), None));
        assert!((objs[0].rotation_rad - std::f32::consts::FRAC_PI_2).abs() < 1e-4);
        // 中心の真上 (画面 y は下向き) → atan2(-1,0) + π/2 = 0。
        apply_text_drag(&mut objs, &drag, (100.0, 50.0), None);
        assert!(objs[0].rotation_rad.abs() < 1e-4);
    }

    #[test]
    fn corner_drag_resizes_stamp_aspect_preserving() {
        // アスペクト 2:1 のスタンプ。corner を (180,100) へ → lx=80, ly=0。
        let mut objs = vec![stamp_obj(1, (100.0, 100.0), 40.0, 20.0)];
        let drag = TextDrag {
            id: 1,
            kind: TextDragKind::Corner(2),
            start: egui::pos2(0.0, 0.0),
            last_img: (0.0, 0.0),
            armed: true,
            moved: false,
        };
        assert!(apply_text_drag(&mut objs, &drag, (180.0, 100.0), None));
        if let AnnotationKind::Stamp(s) = &objs[0].kind {
            assert!((s.half_w - 80.0).abs() < 1e-3, "half_w={}", s.half_w);
            // アスペクト 2:1 を保持。
            assert!((s.half_h - 40.0).abs() < 1e-3, "half_h={}", s.half_h);
        } else {
            panic!("not a stamp");
        }
    }

    #[test]
    fn pick_handle_finds_rotation_knob_then_misses() {
        let view = make_view(Rotation::None, 0.0); // scale = 2.0
        let obj = stamp_obj(1, (200.0, 150.0), 50.0, 30.0);
        let (_, roth) = handle_points(&obj, None, view.scale).expect("stamp has handles");
        let knob_screen = view.image_to_screen(roth.0, roth.1);
        assert_eq!(
            pick_handle(&obj, knob_screen, &view, None),
            Some(TextDragKind::Rotate)
        );
        // 遠い点はどのハンドルにも当たらない。
        assert_eq!(pick_handle(&obj, egui::pos2(0.0, 0.0), &view, None), None);
    }

    #[test]
    fn pick_handle_finds_corner() {
        let view = make_view(Rotation::None, 0.0);
        let obj = stamp_obj(1, (200.0, 150.0), 50.0, 30.0);
        let (corners, _) = handle_points(&obj, None, view.scale).expect("stamp has handles");
        // BR コーナー (index 2)。
        let br_screen = view.image_to_screen(corners[2].0, corners[2].1);
        assert_eq!(
            pick_handle(&obj, br_screen, &view, None),
            Some(TextDragKind::Corner(2))
        );
    }
}
