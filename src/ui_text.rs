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
//! - Undo/Redo (`do_comic_undo` / `do_comic_redo` / `commit_comic_undo_on_settle`、Inc 6):
//!   テキストモード中だけ効く Ctrl+Z / Ctrl+Y (Ctrl+Shift+Z) のエディタ専用
//!   スナップショットスタック (D7)。`meta_undo` (レーティング/タグ) とは別スタックで、
//!   `handle_text_keys` が先に key を consume するので干渉しない。編集が settle したら
//!   ベースラインとの差を 1 エントリに coalesce する (ラボ移植)。
//!
//! 変形ハンドル (四隅スケール / 回転ノブ / しっぽ) と Undo/Redo は Inc 6、スタンプ
//! ピッカー (絵文字アセット) は Inc 4c、プリセット / 追加ダイアログは Inc 5。

use crate::app::{App, TextDrag, TextDragKind};
use crate::comic_presets::{ShapeStylePreset, TextStylePreset, WindowStylePreset};
use crate::ui_fullscreen::{FsKeyAction, SpreadPair};
use comic_core::{
    AnnotationKind, AnnotationObject, BubbleObject, BubbleShape, DecoKind, DecoPlacement,
    DecorationLayer, FillMode, FontSet, FrameStyle, IndicatorKind, InlineDir, MarkupRule,
    MessageWindowObject, NamePlateMode, Orientation, PortraitSide, Rgba, ShadowStyle, SizeMode,
    StampObject, StrokeStyle, Tail, TailKind, TextAlign, TextBackgroundStyle, TextBlock,
    TextEchoStyle, TextGlowStyle, TextShadowStyle, VAnchor, WindowPosition,
};
use std::collections::HashMap;

/// パネル幅 (編集コントロールが入るので conceal より少し広い)。
const PANEL_W: f32 = 268.0;
const PANEL_MARGIN_X: f32 = 16.0;
const PANEL_MARGIN_Y: f32 = 60.0;
/// ハンドル (回転ノブ / 四隅 / しっぽ) の当たり判定半径 (画面 px)。ラボと同値。
const HANDLE_R: f32 = 7.0;
/// エディタ専用 Undo/Redo (Inc 6) の最大スタック深さ。ラボ (`UNDO_CAP`) と同値。
/// 超過時は最古エントリから捨てる。
const COMIC_UNDO_CAP: usize = 100;

/// 右詳細パネルのカテゴリタブ。補正レイヤーの section-accent と同じく、各カテゴリに
/// アクセント色を割り当て、タブボタン + コンテンツ左端の色帯で「カラーの縦線での分類」を
/// 与える (ラボの `PropTab` 相当)。種別ごとに有効タブが異なる: テキスト=セリフのみ、
/// 吹き出し=セリフ/本体/しっぽ/飾り、ウィンドウ=セリフ/枠(本体)/部品。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TextPropTab {
    /// セリフ (本文テキスト): 青。
    Serifu,
    /// 本体 (吹き出し形状・塗り / ウィンドウ枠): 緑。
    Body,
    /// しっぽ (吹き出しのしっぽ): 橙。
    Tail,
    /// 部品 (ウィンドウの名前プレート / 立ち絵枠 / 続き指標): 紫。
    Parts,
    /// 飾り (吹き出しの装飾レイヤー = きらきら / 花 / 泡): 桃。
    Deco,
}

impl TextPropTab {
    /// カテゴリのアクセント色 (タブボタンと左色帯で共有)。
    fn color(self) -> egui::Color32 {
        match self {
            TextPropTab::Serifu => egui::Color32::from_rgb(90, 170, 255), // 青
            TextPropTab::Body => egui::Color32::from_rgb(95, 208, 140),   // 緑
            TextPropTab::Tail => egui::Color32::from_rgb(255, 160, 60),   // 橙
            TextPropTab::Parts => egui::Color32::from_rgb(170, 140, 240), // 紫
            TextPropTab::Deco => egui::Color32::from_rgb(240, 130, 195),  // 桃
        }
    }

    fn label(self) -> &'static str {
        match self {
            TextPropTab::Serifu => "セリフ",
            TextPropTab::Body => "本体",
            TextPropTab::Tail => "しっぽ",
            TextPropTab::Parts => "部品",
            TextPropTab::Deco => "飾り",
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
                    // ウィンドウを手で動かしたら位置プリセットを解除する (= Free)。
                    // でないと `resolve_window_placement` が次の編集で元位置へ戻してしまう。
                    if let AnnotationKind::MessageWindow(w) = &mut o.kind {
                        w.position = WindowPosition::Free;
                    }
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

/// `text` の char 範囲 `[start, end)` を `open` / `close` で囲み、新しいキャレット位置
/// (char index、close の直後) を返す (ラボ `insert_markers` 相当)。記号挿入ボタン用。
fn insert_markers(text: &mut String, start: usize, end: usize, open: char, close: char) -> usize {
    let chars: Vec<char> = text.chars().collect();
    let n = chars.len();
    let start = start.min(n);
    let end = end.min(n).max(start);
    let mut out = String::with_capacity(text.len() + open.len_utf8() + close.len_utf8());
    out.extend(chars[..start].iter());
    out.push(open);
    out.extend(chars[start..end].iter());
    out.push(close);
    out.extend(chars[end..].iter());
    *text = out;
    start + 1 + (end - start)
}

/// 2 つの記法ルール列が open/close ペアとして等価か (dir は位置で固定なので無視)。
/// 記号セットコンボの現在選択判定に使う (ラボ `marker_pairs_eq` 相当)。
fn marker_pairs_eq(a: &[MarkupRule], b: &[MarkupRule]) -> bool {
    a.len() == b.len()
        && a.iter()
            .zip(b.iter())
            .all(|(x, y)| x.open == y.open && x.close == y.close)
}

fn kind_label(o: &AnnotationObject) -> &'static str {
    match &o.kind {
        AnnotationKind::Bubble(_) => "吹き出し",
        AnnotationKind::Text(_) => "テキスト",
        AnnotationKind::MessageWindow(_) => "ウィンドウ",
        AnnotationKind::Stamp(_) => "スタンプ",
    }
}

// ── Undo / Redo の純ロジック (Inc 6、テスト容易にするため App 非依存) ──────
//
// ラボ `commit_pending` / `do_undo` / `do_redo` の coalesce スタック操作を、
// `comic_docs[key]` の作業 Vec (`objects`) と 3 本のスタック / ベースラインだけを
// 受け取る自由関数に切り出したもの。App 側メソッドはこれを呼ぶだけの薄いファサード。

/// coalesce commit: `objects` がベースラインと異なれば旧ベースラインを undo へ積み、
/// redo をクリアし、ベースラインを `objects` へ更新する。`UNDO_CAP` 超過分は最古を捨てる。
fn comic_commit_pending(
    undo: &mut Vec<Vec<AnnotationObject>>,
    redo: &mut Vec<Vec<AnnotationObject>>,
    baseline: &mut Vec<AnnotationObject>,
    objects: &[AnnotationObject],
) {
    if objects != baseline.as_slice() {
        undo.push(std::mem::replace(baseline, objects.to_vec()));
        if undo.len() > COMIC_UNDO_CAP {
            undo.remove(0);
        }
        redo.clear();
    }
}

/// undo 1 段: まず未コミット編集を commit してから 1 つ戻す。戻せたら復元後の状態を
/// `Some` で返す (呼び出し側が `comic_docs` へ書き戻す)。スタック空なら `None`。
fn comic_undo_step(
    undo: &mut Vec<Vec<AnnotationObject>>,
    redo: &mut Vec<Vec<AnnotationObject>>,
    baseline: &mut Vec<AnnotationObject>,
    objects: &[AnnotationObject],
) -> Option<Vec<AnnotationObject>> {
    comic_commit_pending(undo, redo, baseline, objects);
    let prev = undo.pop()?;
    redo.push(objects.to_vec());
    *baseline = prev.clone();
    Some(prev)
}

/// redo 1 段: 1 つ進める (commit はしない、ラボ `do_redo` と同じ)。進めたら復元後の
/// 状態を `Some` で返す。スタック空なら `None`。
fn comic_redo_step(
    undo: &mut Vec<Vec<AnnotationObject>>,
    redo: &mut Vec<Vec<AnnotationObject>>,
    baseline: &mut Vec<AnnotationObject>,
    objects: &[AnnotationObject],
) -> Option<Vec<AnnotationObject>> {
    let next = redo.pop()?;
    undo.push(objects.to_vec());
    *baseline = next.clone();
    Some(next)
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
        self.text_font_dialog = false;
        self.text_font_dialog_target = None;
        // スタンプピッカーの差し替え対象は page-local id なので、モードをまたいで残すと
        // 別ページの同 id スタンプを誤って差し替える (Codex P2)。入場時に必ずクリアする。
        self.text_add_stamp_dialog = false;
        self.stamp_dialog_replace_target = None;
        self.clear_meta_undo();
        self.ensure_comic_doc_loaded(&key);
        // エディタ専用 undo を入場ページの現状態へリセット (D7、`meta_undo` とは別スタック)。
        self.reset_comic_history(&key);

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
        self.text_font_dialog = false;
        self.text_font_dialog_target = None;
        // スタンプピッカーの差し替え対象を退場時にもクリア (Codex P2、enter 側と対)。
        self.text_add_stamp_dialog = false;
        self.stamp_dialog_replace_target = None;
        // 進行中のスタンプ埋め込み worker も退場時に cancel して破棄 (Codex P3: フルスクリーンを
        // 閉じると overlay の poll が走らず stale guard が遅延/喪失するため、ここで確実に止める)。
        if let Some(p) = self.stamp_embed_pending.take() {
            p.cancel.store(true, std::sync::atomic::Ordering::Relaxed);
        }
        // プレビュー解像度の縮小 base を解放 (R2 perf)。退場後はフル解像度で焼き直すので不要。
        self.comic_preview_base = None;
        if was_text_mode {
            self.clear_meta_undo();
        }
        // エディタ専用 undo スタックも退場時にクリア (次回入場で再ベースライン化される)。
        self.comic_undo_stack.clear();
        self.comic_redo_stack.clear();
        self.comic_undo_baseline.clear();
        self.comic_undo_key = None;
        // しっぽ stash もクリア (ページ / モードをまたいで持ち越さない)。
        self.comic_tail_stash.clear();

        if let Some(ctx) = self.text_spread_ctx.take() {
            self.spread_mode = ctx.saved_mode;
            self.fullscreen_idx = Some(ctx.pair.0);
            self.fs_zoom = 1.0;
            self.fs_pan = egui::Vec2::ZERO;
        }
        crate::logger::log("text: reset mode".to_string());
    }

    // ── Undo / Redo (Inc 6、エディタ専用スナップショットスタック、D7) ──────
    //
    // ラボ (`tools/comic_lab`) の coalesce 方式を移植: 編集が settle するたびに
    // ベースラインとの差分を 1 エントリにまとめて push する。`meta_undo`
    // (レーティング/タグ) とは完全に別スタックで、テキストモード中のみ動く。
    // 対象は現在ページの注釈 `comic_docs[key]` 全体。

    /// 注釈 undo/redo を現在ページ `key` の状態へリセットする (モード入場・ページ確定時)。
    /// ラボ `reset_history` 相当。
    fn reset_comic_history(&mut self, key: &str) {
        self.comic_undo_stack.clear();
        self.comic_redo_stack.clear();
        self.comic_undo_baseline = self.comic_docs.get(key).cloned().unwrap_or_default();
        self.comic_undo_key = Some(key.to_string());
    }

    /// coalesce した 1 エントリを commit する (busy 判定は呼び出し側)。ラボ
    /// `commit_pending` 相当。`comic_docs[key]` の作業 Vec を渡して純ロジックへ委譲。
    fn commit_comic_pending(&mut self, key: &str) {
        if let Some(objects) = self.comic_docs.get(key) {
            comic_commit_pending(
                &mut self.comic_undo_stack,
                &mut self.comic_redo_stack,
                &mut self.comic_undo_baseline,
                objects,
            );
        }
    }

    /// フレーム末で呼ぶ coalesce commit。drag 中 (ポインタ押下) や IME / テキストフィールド
    /// フォーカス中 (キーボード入力要求) は commit せず、操作が落ち着いてから 1 エントリに
    /// まとめる。ラボ `update` 末尾の `if !busy { commit_pending() }` 相当。
    pub(crate) fn commit_comic_undo_on_settle(&mut self, ctx: &egui::Context) {
        if !self.text_mode {
            return;
        }
        let Some(key) = self.fullscreen_idx.and_then(|i| self.page_path_key(i)) else {
            return;
        };
        // テキストモード中はページ固定だが、防御的にキー不一致を検出したら再ベースライン化して
        // cross-page の undo エントリができないようにする。
        if self.comic_undo_key.as_deref() != Some(key.as_str()) {
            self.reset_comic_history(&key);
            return;
        }
        let busy = ctx.input(|i| i.pointer.any_down()) || ctx.wants_keyboard_input();
        if busy {
            return;
        }
        self.commit_comic_pending(&key);
    }

    /// Ctrl+Z。未コミットの編集をまず commit してから 1 つ戻す。ラボ `do_undo` 相当。
    fn do_comic_undo(&mut self) {
        let Some(key) = self.fullscreen_idx.and_then(|i| self.page_path_key(i)) else {
            return;
        };
        if self.comic_undo_key.as_deref() != Some(key.as_str()) {
            // 別ページ — 戻す履歴がない。現ページで再ベースライン化だけして抜ける。
            self.reset_comic_history(&key);
            return;
        }
        let cur = self.comic_docs.get(&key).cloned().unwrap_or_default();
        if let Some(prev) = comic_undo_step(
            &mut self.comic_undo_stack,
            &mut self.comic_redo_stack,
            &mut self.comic_undo_baseline,
            &cur,
        ) {
            self.comic_docs.insert(key.clone(), prev);
            self.after_comic_history_change(&key);
        }
    }

    /// Ctrl+Y / Ctrl+Shift+Z。ラボ `do_redo` 相当 (commit_pending は呼ばない)。
    fn do_comic_redo(&mut self) {
        let Some(key) = self.fullscreen_idx.and_then(|i| self.page_path_key(i)) else {
            return;
        };
        if self.comic_undo_key.as_deref() != Some(key.as_str()) {
            self.reset_comic_history(&key);
            return;
        }
        let cur = self.comic_docs.get(&key).cloned().unwrap_or_default();
        if let Some(next) = comic_redo_step(
            &mut self.comic_undo_stack,
            &mut self.comic_redo_stack,
            &mut self.comic_undo_baseline,
            &cur,
        ) {
            self.comic_docs.insert(key.clone(), next);
            self.after_comic_history_change(&key);
        }
    }

    /// undo/redo 後の後始末: 消えた選択を解除し、進行中ドラッグを破棄、再ベイクと
    /// 永続化 (デバウンス保存) を予約する。ラボ `after_history_change` 相当
    /// (mIV は tail_stash/deco_stash を持たないので保持処理は不要)。
    fn after_comic_history_change(&mut self, key: &str) {
        if let Some(sel) = self.text_selected {
            let exists = self
                .comic_docs
                .get(key)
                .is_some_and(|objs| objs.iter().any(|o| o.id == sel));
            if !exists {
                self.text_selected = None;
            }
        }
        // 復元の途中で握っていたドラッグ状態は無効化する (古い id / 座標基準が残る事故防止)。
        self.text_drag = None;
        // undo/redo で消えたオブジェクトのしっぽ stash を prune (stash の desync 防止、
        // ラボ `after_history_change` 相当)。
        if let Some(objs) = self.comic_docs.get(key) {
            self.comic_tail_stash
                .retain(|id, _| objs.iter().any(|o| o.id == *id));
        }
        // 再ベイク (comic_generation を進める) + 復元結果を comic.db + サイドカーへ
        // デバウンス保存に乗せる (退場時 reset_text_mode でも最終保存される)。
        self.mark_comic_dirty();
        self.text_dirty_at = Some(std::time::Instant::now());
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

        let editing_text = ctx.memory(|m| m.focused().is_some());

        // Ctrl+Z / Ctrl+Y / Ctrl+Shift+Z: エディタ専用 Undo/Redo (D7)。
        // テキスト入力中のウィジェット (TextEdit) が **キーボード入力を要求している間**は
        // egui に Ctrl+Z を委ねて (= 消費しない) フィールド内テキストの undo を壊さない。
        // 判定は `wants_keyboard_input()` を使う (ラボと同じ。`focused().is_some()` だと
        // ボタン / スライダーへのフォーカスでも抑制してしまい、TextEdit でもないのに
        // comic undo が効かなくなる — Codex P3)。フィールド外の編集 (追加 / 削除 / 移動 /
        // 変形 / パネル操作) のみここで undo する。
        // **メタ undo との非干渉**: テキストモード中は `handle_fs_key_input` がこの関数で
        // return するため、ここで消費しなかった key も含めて `handle_meta_undo_keys`
        // (レーティング/タグ undo) には決して到達しない。
        // consume 順は `handle_meta_undo_keys` と同じ: Ctrl+Shift+Z (redo) → Ctrl+Y (redo)
        // → Ctrl+Z (undo)。`Modifiers::CTRL` 指定の consume は Shift 併用 Z も拾うため
        // redo を先に握り、Ctrl+Shift+Z が undo 側へ流れないようにする。
        if !ctx.wants_keyboard_input() {
            let (undo, redo) = ctx.input_mut(|i| {
                let redo = i
                    .consume_key(egui::Modifiers::CTRL | egui::Modifiers::SHIFT, egui::Key::Z)
                    || i.consume_key(egui::Modifiers::CTRL, egui::Key::Y);
                let undo = i.consume_key(egui::Modifiers::CTRL, egui::Key::Z);
                (undo, redo)
            });
            if undo {
                self.do_comic_undo();
                return action;
            }
            if redo {
                self.do_comic_redo();
                return action;
            }
        }

        // Delete / Backspace: 選択オブジェクトを削除 (テキストフィールド非フォーカス時のみ)。
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
        // 削除したオブジェクトのしっぽ stash も破棄 (orphan 防止)。
        self.comic_tail_stash.remove(&id);
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
        // これらは暗フレーム (Frame::window().fill(dark)) を強制するが、egui::Window の
        // タイトルバー文字と × ボタンはビューポート ctx の visuals で描かれる。フルスクリーン
        // ビューポートは OS テーマ連動でライトになる場合があり、その時タイトル/× が暗色になって
        // 暗フレーム上で読めない (実機 FB 2026-06-06)。ダイアログ描画の間だけ ctx visuals を
        // ダークへ上書きし、描画後に元へ戻す (他の fullscreen UI に影響させない)。
        let prev_visuals = ctx.style().visuals.clone();
        ctx.set_visuals(egui::Visuals::dark());
        self.draw_text_add_bubble_dialog(ctx);
        self.draw_text_add_window_dialog(ctx);
        self.draw_text_add_stamp_dialog(ctx);
        self.draw_text_add_onomatopoeia_dialog(ctx);
        self.draw_text_font_dialog(ctx);
        ctx.set_visuals(prev_visuals);
        // フレーム末: 編集が settle (drag 終了 + フィールド非フォーカス) したら
        // coalesce した undo エントリを 1 つ commit する (Inc 6、ラボの frame-end
        // `commit_pending` 相当)。パネル / キャンバス / ダイアログの全編集が反映済みの
        // この時点で呼ぶ。
        self.commit_comic_undo_on_settle(ctx);
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

        // プリセット (組み込み + ユーザー) を 1 度だけロードし、編集 UI 用にスナップショット
        // する (詳細パネル closure は self を借りないので、ローカルへ複製してから渡す)。
        self.ensure_comic_presets_loaded();
        let preset_text = self.comic_text_presets.clone();
        let preset_shape = self.comic_shape_presets.clone();
        let preset_window = self.comic_window_presets.clone();
        let mut preset_name_input = self.comic_preset_name_input.clone();
        let mut preset_reqs = PresetRequests::default();

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
        let mut open_font_picker = false;
        // プレビュー解像度 (R2 perf): closure 内では self を借りない規約に従い、現在値を Copy で
        // 取り出し、変更要求はローカルに溜めて closure 後に適用する。
        let current_preview_scale = self.settings.text_preview_scale.clamp(1, 8);
        let mut preview_scale_req: Option<u32> = None;

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

                        // ── プレビュー解像度 (R2 perf) ──。編集中だけ表示を 1/N に下げて合成 +
                        // GPU upload を軽くし、ドラッグをスムーズにする。保存/コピー/比較/書き出しは
                        // フル解像度のまま。ツールを閉じると原寸に戻る (鮮明化)。
                        ui.horizontal(|ui| {
                            ui.label("プレビュー解像度:");
                            for (label, scale) in
                                [("原寸", 1u32), ("1/2", 2), ("1/4", 4), ("1/8", 8)]
                            {
                                if ui
                                    .selectable_label(current_preview_scale == scale, label)
                                    .clicked()
                                {
                                    preview_scale_req = Some(scale);
                                }
                            }
                        });
                        ui.label(
                            egui::RichText::new("※下げると操作がスムーズになります")
                                .small()
                                .weak(),
                        );
                        ui.separator();

                        // ── 追加 ── (ラボと同じく 1 行 1 ボタンの全幅レイアウト・同じ並び)。
                        let add_w = PANEL_W - 16.0;
                        if ui
                            .add_sized([add_w, 26.0], egui::Button::new("吹き出し追加"))
                            .clicked()
                        {
                            // 形状を選ぶダイアログを開く (ラボの「吹き出しを追加」相当)。
                            open_bubble_dialog = true;
                        }
                        ui.add_space(2.0);
                        if ui
                            .add_sized([add_w, 26.0], egui::Button::new("ウィンドウ追加"))
                            .clicked()
                        {
                            open_window_dialog = true;
                        }
                        ui.add_space(2.0);
                        if ui
                            .add_sized([add_w, 26.0], egui::Button::new("テキスト追加"))
                            .clicked()
                        {
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
                            let mut o = AnnotationObject::new_text(id, (sw * 0.3, sh * 0.3), tb);
                            o.z = z;
                            objects.push(o);
                            selected = Some(id);
                            changed = true;
                        }
                        ui.add_space(2.0);
                        if ui
                            .add_sized([add_w, 26.0], egui::Button::new("オノマトペ追加"))
                            .clicked()
                        {
                            open_onomatopoeia_dialog = true;
                        }
                        ui.add_space(2.0);
                        if ui
                            .add_sized([add_w, 26.0], egui::Button::new("スタンプ追加"))
                            .clicked()
                        {
                            open_stamp_dialog = true;
                        }
                        ui.separator();

                        // 一覧 (ScrollArea) の直下に操作行 (↑↓複製削除) を置く。一覧は内容に
                        // 合わせて縦に縮み (auto_shrink 縦)、左パネルからあふれるときだけ
                        // スクロールする (ラボ準拠 / 実機 FB 2026-06-06)。
                        // スクロール上限は「現在のカーソル位置 → sink_rect 下端 − 操作行ぶん」
                        // で算出する。これにより操作行は必ず sink_rect (= パネル矩形) 内に
                        // 収まる。sink_rect の外に出ると handle_text_canvas_input が操作行の
                        // クリックを「パネル外＝キャンバス操作」と誤判定し、選択が解除されて
                        // 削除/複製/前後移動が無効化される (実機 FB)。追加ボタンを全幅縦に
                        // した分パネルが縦に伸び、旧 body_height ベースの固定高では操作行が
                        // 矩形外へはみ出していた。
                        const ACTIONS_RESERVE: f32 = 40.0;
                        let list_top = ui.cursor().top();
                        let list_max =
                            (sink_rect.bottom() - list_top - ACTIONS_RESERVE).clamp(80.0, 2000.0);
                        egui::ScrollArea::vertical()
                            .id_salt("text_panel_scroll")
                            .max_height(list_max)
                            .auto_shrink([false, true])
                            .show(ui, |ui| {
                                object_list_rows_ui(ui, &mut objects, &mut selected, &mut changed);
                            });
                        ui.separator();
                        object_list_actions_ui(ui, &mut objects, &mut selected, &mut changed);
                    });
            });

        // ── 右パネル: 詳細設定 (選択オブジェクトの編集) ──
        // 選択が窓なら、本文が枠から溢れているかを fonts 込みで判定 (常時表示の本文欄を
        // 赤枠警告にする)。窓以外は計算しない (ensure_comic_fonts はロード済みなら Arc
        // clone を返すだけだが、無駄な呼び出しを避ける)。
        let window_overflow = {
            let sel_win = selected
                .and_then(|id| objects.iter().find(|o| o.id == id))
                .map(|o| matches!(o.kind, AnnotationKind::MessageWindow(_)))
                .unwrap_or(false);
            if sel_win {
                // 参照フォント (カスタム/pack) も含めてロードしてから判定する。base のみの
                // ensure_comic_fonts() だと未ロードフォントの窓で fallback metrics になり
                // overflow 警告がずれる (Codex P3)。warm なら cache 済み Arc を返すだけ。
                let fonts = self.ensure_comic_fonts_for(&objects);
                match (
                    fonts.as_deref(),
                    selected.and_then(|id| objects.iter().find(|o| o.id == id)),
                ) {
                    (Some(f), Some(o)) => matches!(
                        &o.kind,
                        AnnotationKind::MessageWindow(w)
                            if comic_core::message_window_overflows(w, f)
                    ),
                    _ => false,
                }
            } else {
                false
            }
        };
        // しっぽ stash は App 状態だが、closure は self を借りないので `objects` と同様に
        // 一旦ローカルへ取り出して &mut を渡し、closure 後に書き戻す。
        let mut tail_stash = std::mem::take(&mut self.comic_tail_stash);
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
                                            let mut pctx = PresetCtx {
                                                text: &preset_text,
                                                shape: &preset_shape,
                                                window: &preset_window,
                                                name_input: &mut preset_name_input,
                                                reqs: &mut preset_reqs,
                                            };
                                            edit_object_ui(
                                                ui,
                                                o,
                                                &mut prop_tab,
                                                &mut changed,
                                                &mut open_stamp_replace,
                                                &mut open_font_picker,
                                                &mut pctx,
                                                &mut tail_stash,
                                                window_overflow,
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
        // しっぽ stash を書き戻す (closure で更新された分を App 状態へ反映)。
        self.comic_tail_stash = tail_stash;
        // 一覧パネルの削除など (object_list_actions_ui は stash を知らない) で消えた
        // オブジェクトの stash を毎フレーム prune する。next_id = max+1 なので、消した
        // 最大 id を放置すると新規オブジェクトが同 id を再利用して別物のしっぽを復元して
        // しまう (Codex P2)。`objects` は書き戻し前の最新状態。
        self.comic_tail_stash
            .retain(|id, _| objects.iter().any(|o| o.id == *id));

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
        if open_font_picker {
            // 選択中テキストのフォントを見本から選ぶダイアログを開く。
            self.text_font_dialog_target = selected;
            self.text_font_dialog_filter.clear();
            if self.text_font_dialog_sample.is_empty() {
                self.text_font_dialog_sample = "あア永 Ag".to_string();
            }
            self.font_sample_cache.clear();
            self.text_font_dialog = true;
        }

        // プリセット要求の処理 (保存 / 更新 / 削除)。名前入力を書き戻し、操作が効いたら
        // changed を立てて下の保存 + 再ベイクに乗せる。
        self.comic_preset_name_input = preset_name_input;
        if let Some(target) = preset_reqs.save {
            if self.save_current_as_preset(&mut objects, selected, target) {
                changed = true;
            }
        }
        if let Some((target, id)) = preset_reqs.update {
            if self.update_user_preset(&mut objects, selected, target, &id) {
                changed = true;
            }
        }
        if let Some(id) = preset_reqs.delete {
            if self.delete_user_preset(&mut objects, &id) {
                changed = true;
            }
        }

        // 位置プリセット (上/中/下/中央) のウィンドウは pivot をソース寸法に対して解決する。
        // 何か編集された (changed) フレームだけ実行し、対象が非 Free のウィンドウのときに限る。
        // 解決は冪等なので、配置と無関係な編集で呼んでも同じ pivot になる (Free はドラッグで
        // 置いた位置を保持)。プリセット適用で位置が変わった直後にも効く。
        if changed {
            if let Some(id) = selected {
                let is_positioned_window = objects.iter().any(|o| {
                    o.id == id
                        && matches!(
                            &o.kind,
                            AnnotationKind::MessageWindow(w) if w.position != WindowPosition::Free
                        )
                });
                if is_positioned_window {
                    let fonts = self.ensure_comic_fonts();
                    resolve_window_placement(&mut objects, id, sw, sh, fonts.as_deref());
                }
            }
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
        // プレビュー解像度の変更を適用 (R2 perf)。次フレームの ensure_comic_composite_texture が
        // preview_scale 不一致で cache miss → 新倍率で焼き直す。設定は永続化する。
        if let Some(scale) = preview_scale_req {
            if self.settings.text_preview_scale != scale {
                self.settings.text_preview_scale = scale;
                self.settings.save();
                ctx.request_repaint();
            }
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
        egui::Window::new("メッセージウィンドウを追加")
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
            // 名前プレートの本文も同じ既定フォントにしておく (ラボと同様。名前フォントを
            // 個別に変える UI は無いので、作成時に有効なフォントを焼き付ける)。
            w.name_plate.name.font_key = font_key.clone();
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
    /// クリックで標準テキストオブジェクトを追加する (ラボ準拠)。オノマトペは各プリセットが
    /// OFL フォントを指定する前提なので、追加パック未導入時は **追加自体をブロック** し
    /// (システム既定フォントへのフォールバックはしない)、注意書き + 入手ボタンを出す。
    /// 後からパックを導入して保存済みオノマトペの見た目が変わる事故を防ぐため (実機 FB)。
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
                            "未導入のため追加できません。下のボタンから追加パックを導入してください。",
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
                        // パック未導入時はカードを無効化して追加をブロックする (見本は見える
                        // が灰色 + 非クリック。OFL フォント前提なのでフォールバック追加しない)。
                        ui.add_enabled_ui(pack_installed, |ui| {
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
            });

        self.text_add_onomatopoeia_dialog = open;
        if request_pack {
            // 明示クリックなのでこのセッションで辞退済みでも開く。
            self.editing_addon_declined_session = false;
            self.maybe_prompt_editing_addon();
        }
        // パック未導入時は追加をブロック (カード無効化に加えた防御。OFL フォント前提)。
        if let Some(preset) = chosen.filter(|_| pack_installed) {
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

    /// フォント `font_key` で見本テキストを 1 行ベイクして RGBA 画像にする。
    ///
    /// ピッカー専用に、共有 FontSet (`comic_fonts`) を**触らず**フォントを単発で
    /// read+parse して焼く。共有 FontSet を rebuild すると、一覧スクロールで新フォントを
    /// 見るたびに既ロード分まで再 read/再 parse され O(n^2) になる (Codex P2)。単発 parse
    /// なら 1 フォント = 1 read + 1 parse で、呼び出し側の 1 フレーム予算で律速できる。
    /// パス不明 / read / parse 失敗時は `None` (= カードに見本を出さない)。
    fn render_font_sample(
        &self,
        font_key: &str,
        sample: &str,
        px: f32,
    ) -> Option<egui::ColorImage> {
        let path = self.comic_font_paths.get(font_key)?;
        let bytes = std::fs::read(path).ok()?;
        let font = comic_core::LoadedFont::from_bytes(font_key.to_string(), bytes).ok()?;
        // 1 カードが巨大テクスチャを確保しないよう見本長を制限する (フィールドは編集可)。
        let sample: String = sample.chars().take(40).collect();
        let block = TextBlock {
            text: sample,
            font_key: font_key.to_string(),
            size_px: px,
            color: Rgba::BLACK,
            orientation: Orientation::Horizontal,
            ..TextBlock::default()
        };
        let layout = comic_core::layout_text(&block, &font);
        let pad = 6.0f32;
        let w = ((layout.bounds.0 + pad * 2.0).ceil() as usize).clamp(1, 2000);
        let h = ((layout.bounds.1 + pad * 2.0).ceil() as usize).clamp(1, 400);
        let mut set = comic_core::FontSet::new();
        set.insert(font);
        let obj = AnnotationObject::new_text(0, (pad, pad), block);
        let overlay = comic_core::bake_overlay(&[obj], w, h, &set);
        Some(egui::ColorImage::from_rgba_unmultiplied(
            [overlay.w, overlay.h],
            &overlay.pixels,
        ))
    }

    /// フォント見本テクスチャを遅延構築 + キャッシュして返す。キーは `(font_key, 見本)`。
    /// 描画失敗は `font_sample_failed` に記録して毎フレームの再試行を防ぐ (Codex P3)。
    fn font_sample_texture(
        &mut self,
        ctx: &egui::Context,
        font_key: &str,
    ) -> Option<egui::TextureHandle> {
        let sample = self.text_font_dialog_sample.clone();
        let cache_key = (font_key.to_string(), sample.clone());
        if let Some(tex) = self.font_sample_cache.get(&cache_key) {
            return Some(tex.clone());
        }
        if self.font_sample_failed.contains(font_key) {
            return None;
        }
        let Some(img) = self.render_font_sample(font_key, &sample, 30.0) else {
            self.font_sample_failed.insert(font_key.to_string());
            return None;
        };
        let tex_name = format!(
            "font_sample_{}_{}",
            crate::font_assets::font_lookup_key(font_key),
            crate::font_assets::font_lookup_key(&sample)
        );
        let tex = ctx.load_texture(tex_name, img, egui::TextureOptions::LINEAR);
        self.font_sample_cache.insert(cache_key, tex.clone());
        Some(tex)
    }

    /// フォント選択ダイアログの 1 カード (上に見本ストリップ、下にフォント名)。クリックで
    /// true。`allow_build` のときだけ見本テクスチャを焼く (1 フレーム予算)。
    fn draw_font_card(
        &mut self,
        ui: &mut egui::Ui,
        ctx: &egui::Context,
        key: &str,
        selected: bool,
        allow_build: bool,
    ) -> bool {
        let (rect, resp) =
            ui.allocate_exact_size(egui::vec2(FONT_CARD_W, FONT_CARD_H), egui::Sense::click());
        let hovered = resp.hovered();
        let painter = ui.painter_at(rect);
        let bg = if selected {
            egui::Color32::from_rgb(50, 70, 100)
        } else if hovered {
            egui::Color32::from_rgb(58, 58, 64)
        } else {
            egui::Color32::from_rgb(40, 40, 44)
        };
        painter.rect_filled(rect, 4.0, bg);
        painter.rect_stroke(
            rect,
            4.0,
            egui::Stroke::new(
                if selected { 2.0 } else { 1.0 },
                if selected || hovered {
                    egui::Color32::from_rgb(150, 195, 255)
                } else {
                    egui::Color32::from_gray(70)
                },
            ),
            egui::StrokeKind::Inside,
        );
        let pad = 6.0;
        let name_band = 22.0;
        let sample_h = (FONT_CARD_H - pad * 2.0 - name_band).max(8.0);
        let sample_area = egui::Rect::from_min_size(
            rect.min + egui::vec2(pad, pad),
            egui::vec2(FONT_CARD_W - pad * 2.0, sample_h),
        );
        // 黒文字の見本が読めるよう明るいストリップを敷く。
        painter.rect_filled(sample_area, 2.0, egui::Color32::from_gray(235));
        let tex = if allow_build {
            self.font_sample_texture(ctx, key)
        } else {
            self.font_sample_cache
                .get(&(key.to_string(), self.text_font_dialog_sample.clone()))
                .cloned()
        };
        if let Some(tex) = tex {
            let sz = tex.size_vec2();
            if sz.x > 0.0 && sz.y > 0.0 {
                let scale = (sample_area.width() / sz.x)
                    .min(sample_area.height() / sz.y)
                    .min(1.0);
                let draw = egui::vec2(sz.x * scale, sz.y * scale);
                let origin = sample_area.center() - draw * 0.5;
                painter.image(
                    tex.id(),
                    egui::Rect::from_min_size(origin, draw),
                    egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
                    egui::Color32::WHITE,
                );
            }
        }
        painter.text(
            egui::pos2(rect.min.x + 8.0, rect.max.y - pad - name_band * 0.5),
            egui::Align2::LEFT_CENTER,
            key,
            egui::FontId::proportional(12.0),
            egui::Color32::WHITE,
        );
        resp.clicked()
    }

    /// フォント選択ダイアログ。見本テキスト + 絞り込み + カテゴリ (すべて/追加パック/
    /// ユーザー/システム) でフィルタした一覧を、可視行だけ実フォントで焼いて見せる
    /// (`show_rows` 仮想化 + 1 フレーム予算でカクつきを防ぐ)。選んだフォントを対象
    /// テキストの font_key に設定して再ベイクする。
    fn draw_text_font_dialog(&mut self, ctx: &egui::Context) {
        if !self.text_font_dialog {
            return;
        }
        let Some(fs_idx) = self.fullscreen_idx else {
            self.text_font_dialog = false;
            return;
        };
        let Some(target) = self.text_font_dialog_target else {
            self.text_font_dialog = false;
            return;
        };
        let Some(page_key) = self.page_path_key(fs_idx) else {
            self.text_font_dialog = false;
            return;
        };
        self.ensure_comic_font_registry();

        // 一覧スナップショット (フィルタ適用) と現在のフォントを closure 前に用意する。
        let filter_lc = self.text_font_dialog_filter.to_lowercase();
        let cat = self.text_font_dialog_category;
        let visible: Vec<String> = self
            .comic_available_fonts
            .iter()
            .filter(|a| {
                (filter_lc.is_empty() || a.key.to_lowercase().contains(&filter_lc))
                    && (cat.is_none() || cat == Some(a.category))
            })
            .map(|a| a.key.clone())
            .collect();
        let current_key = self
            .comic_docs
            .get(&page_key)
            .and_then(|objs| objs.iter().find(|o| o.id == target))
            .and_then(|o| o.text_block().map(|tb| tb.font_key.clone()))
            .unwrap_or_default();
        let pack_count = self
            .comic_available_fonts
            .iter()
            .filter(|a| a.category == crate::font_assets::FontCategory::Pack)
            .count();

        let mut open = true;
        let mut chosen: Option<String> = None;
        let mut pick_file = false;
        let avail = ctx.content_rect();
        let default_w = (avail.width() - 24.0).clamp(360.0, 760.0);
        let default_h = (avail.height() - 120.0).clamp(320.0, 600.0);
        let frame = egui::Frame::window(ctx.style().as_ref())
            .fill(egui::Color32::from_rgba_unmultiplied(24, 24, 26, 248))
            .stroke(egui::Stroke::new(
                1.0,
                egui::Color32::from_rgba_unmultiplied(255, 255, 255, 70),
            ));
        egui::Window::new("フォントを見本から選択")
            .id(egui::Id::new("text_font_dialog"))
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
                    ui.label("見本");
                    if ui
                        .add(
                            egui::TextEdit::singleline(&mut self.text_font_dialog_sample)
                                .desired_width(180.0),
                        )
                        .changed()
                    {
                        self.font_sample_cache.clear();
                    }
                    ui.label("絞り込み");
                    ui.add(
                        egui::TextEdit::singleline(&mut self.text_font_dialog_filter)
                            .desired_width(140.0)
                            .hint_text("フォント名"),
                    );
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui.button("ファイルから追加…").clicked() {
                            pick_file = true;
                        }
                    });
                });
                ui.horizontal(|ui| {
                    ui.label("種別");
                    ui.selectable_value(&mut self.text_font_dialog_category, None, "すべて");
                    ui.selectable_value(
                        &mut self.text_font_dialog_category,
                        Some(crate::font_assets::FontCategory::Pack),
                        format!("追加パック ({pack_count})"),
                    );
                    ui.selectable_value(
                        &mut self.text_font_dialog_category,
                        Some(crate::font_assets::FontCategory::User),
                        "ユーザー追加",
                    );
                    ui.selectable_value(
                        &mut self.text_font_dialog_category,
                        Some(crate::font_assets::FontCategory::System),
                        "システム",
                    );
                });
                ui.separator();

                let avail_w = ui.available_width();
                let cols = ((avail_w / (FONT_CARD_W + 8.0)).floor() as usize).max(1);
                let rows = visible.len().div_ceil(cols);
                // 1 フレームに新規ラスタライズするカード数を制限 (開封 / スクロールで
                // フォント parse + texture upload が UI を止めないよう、残りは次フレーム以降)。
                let mut build_budget = 4i32;
                let mut need_repaint = false;

                egui::ScrollArea::vertical()
                    .auto_shrink([false, false])
                    .show_rows(ui, FONT_CARD_H + 8.0, rows, |ui, row_range| {
                        for row in row_range {
                            ui.horizontal(|ui| {
                                for col in 0..cols {
                                    let idx = row * cols + col;
                                    let Some(key) = visible.get(idx) else {
                                        break;
                                    };
                                    let selected = key == &current_key;
                                    // 構築済み (テクスチャ有) か失敗済みなら予算を消費しない。
                                    let settled = self.font_sample_cache.contains_key(&(
                                        key.clone(),
                                        self.text_font_dialog_sample.clone(),
                                    )) || self.font_sample_failed.contains(key);
                                    let allow = settled || build_budget > 0;
                                    if !settled && allow {
                                        build_budget -= 1;
                                    }
                                    if !settled && !allow {
                                        need_repaint = true;
                                    }
                                    if self.draw_font_card(ui, ctx, key, selected, allow) {
                                        chosen = Some(key.clone());
                                    }
                                }
                            });
                        }
                        if need_repaint {
                            ui.ctx().request_repaint();
                        }
                    });
            });

        self.text_font_dialog = open;
        if pick_file {
            if let Some(path) = rfd::FileDialog::new()
                .add_filter("フォント", &["ttf", "otf", "ttc"])
                .pick_file()
            {
                if let Some(label) = self.add_user_font_file(&path) {
                    chosen = Some(label);
                }
            }
        }
        if let Some(font_key) = chosen {
            let mut objs = self.comic_docs.remove(&page_key).unwrap_or_default();
            if let Some(tb) = objs
                .iter_mut()
                .find(|o| o.id == target)
                .and_then(|o| o.text_block_mut())
            {
                tb.font_key = font_key.clone();
                tb.preset_link = None; // 個別編集なのでプリセットリンクを解除
            }
            let fonts = self.ensure_comic_fonts_for(&objs);
            // AutoFitText の位置プリセット窓は本文フォントの寸法で高さが決まるので、
            // フォント変更後は pivot を再アンカーする (Codex P2: フォントダイアログ経路は
            // draw_text_panel の changed 解決を通らないため、ここで明示的に解決する)。
            if let Some((sw, sh)) = self.source_dims_for_idx(fs_idx) {
                resolve_window_placement(&mut objs, target, sw, sh, fonts.as_deref());
            }
            self.save_comic_objects(fs_idx, &page_key, &objs);
            self.text_dirty_at = None;
            self.mark_comic_dirty();
            self.text_font_dialog = false;
        }
    }

    /// 選んだフォントファイルをユーザーフォントディレクトリへコピーし、レジストリを
    /// 再列挙して、その表示ラベル (= font_key) を返す。失敗時は `None`。
    fn add_user_font_file(&mut self, path: &std::path::Path) -> Option<String> {
        let dir = crate::font_assets::user_fonts_dir();
        if std::fs::create_dir_all(&dir).is_err() {
            return None;
        }
        let file_name = path.file_name()?;
        let dest = dir.join(file_name);
        // 既に同名があればコピーを省く (上書きでロックを避ける)。
        if !dest.exists() && std::fs::copy(path, &dest).is_err() {
            return None;
        }
        // 再列挙してこのフォントを一覧に載せる。
        self.comic_font_registry_loaded = false;
        self.ensure_comic_font_registry();
        self.font_sample_cache.clear();
        self.font_sample_failed.clear();
        // コピー先パスから表示ラベルを導出 (font_assets と同じ規則: stem の _/- を空白に)。
        let stem = dest.file_stem().and_then(|s| s.to_str())?;
        let label = stem.replace(['_', '-'], " ").trim().to_string();
        if label.is_empty() { None } else { Some(label) }
    }

    /// 注釈スタイルプリセット (組み込み + ユーザー) を 1 度だけ組み立てる (Inc 5)。
    /// 組み込みは起動ごとに作り直し、ユーザー分は presets.json から読む。
    pub(crate) fn ensure_comic_presets_loaded(&mut self) {
        if self.comic_presets_loaded {
            return;
        }
        self.comic_presets_loaded = true;
        let font = crate::comic_overlay::COMIC_FONT_KEY;
        let user = crate::comic_presets::load_user_presets();
        let mut text = system_text_presets(font);
        text.extend(user.text);
        let mut shape = system_shape_presets();
        shape.extend(user.shape);
        let mut window = system_window_presets(font);
        window.extend(user.window);
        self.comic_text_presets = text;
        self.comic_shape_presets = shape;
        self.comic_window_presets = window;
    }

    /// 再利用されないユニークな `user:<prefix>-<uuid>` プリセット id を作る。
    ///
    /// UUIDv4 にすることで、プリセットを削除しても同じ id が再発行されない (壁時計に
    /// 依存しない)。インデックス再利用方式だと、削除した id が別ページ (comic.db に
    /// 保存済み) の旧リンクと衝突し、無関係なプリセットに点灯/再適用される (Codex P2/P3)。
    fn next_user_preset_id(&self, prefix: &str) -> String {
        format!("user:{prefix}-{}", uuid::Uuid::new_v4())
    }

    /// 選択オブジェクトの現在のスタイルを新規ユーザープリセットとして保存し、その
    /// プリセットへリンクする。名前は `comic_preset_name_input` (空なら "ユーザー")。
    fn save_current_as_preset(
        &mut self,
        objects: &mut [AnnotationObject],
        selected: Option<u64>,
        target: PresetTarget,
    ) -> bool {
        let Some(sel) = selected else {
            return false;
        };
        let Some(idx) = objects.iter().position(|o| o.id == sel) else {
            return false;
        };
        let name = {
            let n = self.comic_preset_name_input.trim();
            if n.is_empty() {
                "ユーザー".to_string()
            } else {
                n.to_string()
            }
        };
        // どのプリセットバーから要求されたか (target) で保存先の種別を決める。オブジェクト
        // 種別では判定しない (吹き出し/窓でセリフ + 本体/ウィンドウのバーが同時に出るため。
        // Codex P2)。テキストプリセットは TextBlock を持つ全種 (Text/Bubble/Window) で保存可。
        match target {
            PresetTarget::Text => {
                let id = self.next_user_preset_id("t");
                let Some(tb) = objects[idx].text_block() else {
                    return false;
                };
                let preset = TextStylePreset::from_text(id.clone(), name, tb);
                self.comic_text_presets.push(preset);
                if let Some(tbm) = objects[idx].text_block_mut() {
                    tbm.preset_link = Some(id);
                }
            }
            PresetTarget::Shape => {
                let AnnotationKind::Bubble(b) = &objects[idx].kind else {
                    return false;
                };
                let id = self.next_user_preset_id("s");
                let preset = ShapeStylePreset::from_bubble(id.clone(), name, b);
                self.comic_shape_presets.push(preset);
                if let AnnotationKind::Bubble(bb) = &mut objects[idx].kind {
                    bb.shape_preset_link = Some(id);
                }
            }
            PresetTarget::Window => {
                let AnnotationKind::MessageWindow(w) = &objects[idx].kind else {
                    return false;
                };
                let id = self.next_user_preset_id("w");
                let preset = WindowStylePreset::from_window(id.clone(), name, w);
                self.comic_window_presets.push(preset);
                if let AnnotationKind::MessageWindow(ww) = &mut objects[idx].kind {
                    ww.style_preset_link = Some(id);
                }
            }
        }
        self.comic_preset_name_input.clear();
        self.save_comic_presets_to_disk();
        true
    }

    /// 既存ユーザープリセット `id` を選択オブジェクトの現在のスタイルで更新し、同じ
    /// プリセットにリンクした全オブジェクトへ再適用して同期する。
    fn update_user_preset(
        &mut self,
        objects: &mut [AnnotationObject],
        selected: Option<u64>,
        target: PresetTarget,
        id: &str,
    ) -> bool {
        if crate::comic_presets::is_system_preset(id) {
            return false;
        }
        let Some(sel) = selected else {
            return false;
        };
        let Some(idx) = objects.iter().position(|o| o.id == sel) else {
            return false;
        };
        // 更新先の種別は target で決める (オブジェクト種別ではない。Codex P2)。
        let mut updated = false;
        match target {
            PresetTarget::Text => {
                if let Some(tb) = objects[idx].text_block() {
                    if let Some(p) = self.comic_text_presets.iter_mut().find(|p| p.id == id) {
                        let name = p.name.clone();
                        *p = TextStylePreset::from_text(id.to_string(), name, tb);
                        updated = true;
                    }
                }
            }
            PresetTarget::Shape => {
                if let AnnotationKind::Bubble(b) = &objects[idx].kind {
                    if let Some(p) = self.comic_shape_presets.iter_mut().find(|p| p.id == id) {
                        let name = p.name.clone();
                        *p = ShapeStylePreset::from_bubble(id.to_string(), name, b);
                        updated = true;
                    }
                }
            }
            PresetTarget::Window => {
                if let AnnotationKind::MessageWindow(w) = &objects[idx].kind {
                    if let Some(p) = self.comic_window_presets.iter_mut().find(|p| p.id == id) {
                        let name = p.name.clone();
                        *p = WindowStylePreset::from_window(id.to_string(), name, w);
                        updated = true;
                    }
                }
            }
        }
        if updated {
            self.reapply_preset_to_linked(objects, id);
            self.save_comic_presets_to_disk();
        }
        updated
    }

    /// 更新済みプリセット `id` を、それにリンクした全オブジェクト (更新元含む) へ再適用する。
    fn reapply_preset_to_linked(&self, objects: &mut [AnnotationObject], id: &str) {
        for o in objects.iter_mut() {
            let pivot = o.pivot;
            // テキストプリセットは全種の本文 TextBlock (Text / Bubble.text / Window.text) に
            // 反映する。常時表示セリフバーで吹き出し/窓にもテキストプリセットを当てられる
            // ようになったため、standalone Text だけ見ると取りこぼす (Codex P2)。
            // 窓の場合、再適用で本文スタイルが実際に変わったら、ウィンドウプリセットが捉える
            // 本文スタイルから乖離するのでウィンドウリンクも解除する (直接適用パスの
            // text_style_diverged 判定と同じ規約。Codex P2 3rd)。
            let mut window_text_changed = false;
            if let Some(tb) = o.text_block_mut() {
                if tb.preset_link.as_deref() == Some(id) {
                    if let Some(p) = self.comic_text_presets.iter().find(|p| p.id == id) {
                        let before = tb.clone();
                        p.apply_to(tb);
                        window_text_changed = text_style_diverged(&before, tb);
                    }
                }
            }
            // 本体 / ウィンドウプリセットは形状 / 枠リンクへ (テキストとは独立。id 接頭辞が
            // 違うので同一 id が両方に当たることはない)。
            match &mut o.kind {
                AnnotationKind::Bubble(b) if b.shape_preset_link.as_deref() == Some(id) => {
                    if let Some(p) = self.comic_shape_presets.iter().find(|p| p.id == id) {
                        p.apply_to(b, default_bubble_tail(pivot));
                    }
                }
                AnnotationKind::MessageWindow(w) if w.style_preset_link.as_deref() == Some(id) => {
                    if let Some(p) = self.comic_window_presets.iter().find(|p| p.id == id) {
                        p.apply_to(w);
                    }
                }
                AnnotationKind::MessageWindow(w) if window_text_changed => {
                    w.style_preset_link = None;
                }
                _ => {}
            }
        }
    }

    /// ユーザープリセット `id` を削除し、リンクしていたオブジェクトのリンクを外す。
    fn delete_user_preset(&mut self, objects: &mut [AnnotationObject], id: &str) -> bool {
        if crate::comic_presets::is_system_preset(id) {
            return false;
        }
        let before = self.comic_text_presets.len()
            + self.comic_shape_presets.len()
            + self.comic_window_presets.len();
        self.comic_text_presets.retain(|p| p.id != id);
        self.comic_shape_presets.retain(|p| p.id != id);
        self.comic_window_presets.retain(|p| p.id != id);
        let after = self.comic_text_presets.len()
            + self.comic_shape_presets.len()
            + self.comic_window_presets.len();
        if before == after {
            return false;
        }
        for o in objects.iter_mut() {
            // テキストプリセットリンクは全種の本文 TextBlock で解除 (Bubble.text / Window.text
            // も含む。常時表示セリフバーで当てられるため。Codex P2)。
            if let Some(tb) = o.text_block_mut() {
                if tb.preset_link.as_deref() == Some(id) {
                    tb.preset_link = None;
                }
            }
            match &mut o.kind {
                AnnotationKind::Bubble(b) if b.shape_preset_link.as_deref() == Some(id) => {
                    b.shape_preset_link = None;
                }
                AnnotationKind::MessageWindow(w) if w.style_preset_link.as_deref() == Some(id) => {
                    w.style_preset_link = None;
                }
                _ => {}
            }
        }
        self.save_comic_presets_to_disk();
        true
    }

    /// ユーザープリセットを presets.json に保存する (sys:* は除外される)。
    fn save_comic_presets_to_disk(&self) {
        crate::comic_presets::save_user_presets(
            &self.comic_text_presets,
            &self.comic_shape_presets,
            &self.comic_window_presets,
        );
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
                // パス参照ではなく画像を縮小して注釈に埋め込む (フォルダ移動 / 別 PC /
                // 元削除でも欠落しない。Codex 監査 P1)。read→decode→縮小→PNG→base64 は
                // 大判画像で重い (R2-6) ので worker に逃がし、完了時に適用する。ダイアログは
                // 即閉じ、処理中は中央に「スタンプ読み込み中…」を出す (draw_stamp_embed_overlay)。
                self.start_stamp_embed(fs_idx, &key, (sw, sh), path);
                self.text_add_stamp_dialog = false;
            }
        }

        if let Some(src) = chosen {
            // 絵文字 / 最近使用 (= 即時・デコード不要) はそのまま同期適用。
            let replace_target = self.stamp_dialog_replace_target.take();
            self.apply_stamp_choice(fs_idx, &key, (sw, sh), src, replace_target);
            self.text_add_stamp_dialog = false;
        } else if !open {
            // × で閉じた: 差し替え対象もクリア。
            self.stamp_dialog_replace_target = None;
        }
    }

    /// ユーザー画像スタンプの埋め込みを worker で開始する (R2-6)。適用先 (fs_idx / key /
    /// 元寸法 / 差し替え対象) を捕捉し、完了時に `poll_stamp_embed` が `apply_stamp_choice`
    /// する。差し替え対象はここで take しておく (ダイアログを閉じても保持するため)。
    fn start_stamp_embed(
        &mut self,
        fs_idx: usize,
        key: &str,
        src_dims: (f32, f32),
        path: std::path::PathBuf,
    ) {
        use std::sync::atomic::AtomicBool;
        // 既存の進行中 worker があれば破棄 (連打対策)。
        if let Some(prev) = self.stamp_embed_pending.take() {
            prev.cancel
                .store(true, std::sync::atomic::Ordering::Relaxed);
        }
        let (tx, rx) = std::sync::mpsc::channel();
        let cancel = std::sync::Arc::new(AtomicBool::new(false));
        let replace_target = self.stamp_dialog_replace_target.take();
        let c = std::sync::Arc::clone(&cancel);
        std::thread::Builder::new()
            .name("stamp-embed".into())
            .spawn(move || {
                if c.load(std::sync::atomic::Ordering::Relaxed) {
                    return;
                }
                let result = crate::comic_stamp::embed_file_stamp(&path);
                let _ = tx.send(result);
            })
            .ok();
        self.stamp_embed_pending = Some(crate::app::StampEmbedPending {
            fs_idx,
            key: key.to_string(),
            src_dims,
            replace_target,
            rx,
            cancel,
        });
    }

    /// 埋め込み worker の完了をポーリングし、できていれば適用する (R2-6)。フルスクリーンの
    /// 描画 (`draw_stamp_embed_overlay`) から毎フレーム呼ばれる。ページ移動 / テキストモード
    /// 終了で stale 化したら cancel して破棄する。`true` を返すと「まだ処理中」(呼び出し側が
    /// 中央トースト + repaint を出す)。
    pub(crate) fn poll_stamp_embed(&mut self) -> bool {
        if self.stamp_embed_pending.is_none() {
            return false;
        }
        // stale guard: テキストモードを抜けた / 別ページに移った → cancel して破棄。
        let stale = {
            let p = self.stamp_embed_pending.as_ref().unwrap();
            !self.text_mode || self.fullscreen_idx != Some(p.fs_idx)
        };
        if stale {
            if let Some(p) = self.stamp_embed_pending.take() {
                p.cancel.store(true, std::sync::atomic::Ordering::Relaxed);
            }
            return false;
        }
        // 借用を閉じてから mutable 操作へ。
        let recv = self.stamp_embed_pending.as_ref().unwrap().rx.try_recv();
        match recv {
            Ok(result) => {
                let p = self.stamp_embed_pending.take().unwrap();
                match result {
                    Some(src) => {
                        self.apply_stamp_choice(
                            p.fs_idx,
                            &p.key,
                            p.src_dims,
                            src,
                            p.replace_target,
                        );
                    }
                    None => {
                        self.show_feedback_toast("画像を読み込めませんでした".to_string());
                    }
                }
                false
            }
            Err(std::sync::mpsc::TryRecvError::Empty) => true, // まだ処理中
            Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                self.stamp_embed_pending = None;
                false
            }
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
        replace_target: Option<u64>,
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
        match replace_target {
            Some(id) => {
                // 既存スタンプのソース差し替え (長辺サイズ保持、短辺をアスペクト再フィット)。
                // 読み込み中に対象が削除された等で見つからない場合は、存在しない ID を選択して
                // 未変更のまま dirty/save するのを避け、注釈を戻して中断する (Codex P2)。
                let Some(obj) = objs.iter_mut().find(|o| o.id == id) else {
                    self.comic_docs.insert(key.to_string(), objs);
                    self.show_feedback_toast(
                        "差し替え対象のスタンプが見つかりませんでした".to_string(),
                    );
                    return;
                };
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

// ── スタイルプリセットの組み込み生成 (Inc 5、ラボ system_*_presets 準拠) ──────
//
// データ構造と永続化は `crate::comic_presets`。組み込み (sys:*) の生成だけは
// BubblePreset / WIN_PRESETS に依存するのでここに置く。

/// 組み込みセリフ (テキスト) プリセット。縦書き + markup ON、既定フォント。
fn system_text_presets(font: &str) -> Vec<TextStylePreset> {
    let base = |id: &str, name: &str, color: Rgba, outline: Option<StrokeStyle>| TextStylePreset {
        id: id.to_string(),
        name: name.to_string(),
        font_key: font.to_string(),
        size_px: 40.0,
        color,
        orientation: Orientation::Vertical,
        align: TextAlign::Center,
        line_gap: 0.0,
        letter_gap: 0.0,
        outline,
        extra_outlines: Vec::new(),
        shadow: None,
        glow: None,
        background: None,
        echo: None,
        auto_tcy: true,
        markup_enabled: true,
        markup_rules: comic_core::default_markup_rules(),
        bold: false,
        italic: false,
    };
    let with_shadow = |mut p: TextStylePreset, shadow: TextShadowStyle| {
        p.shadow = Some(shadow);
        p
    };
    let with_glow = |mut p: TextStylePreset, glow: TextGlowStyle| {
        p.glow = Some(glow);
        p
    };
    let with_bg = |mut p: TextStylePreset, bg: TextBackgroundStyle| {
        p.background = Some(bg);
        p
    };
    let with_echo = |mut p: TextStylePreset, echo: TextEchoStyle| {
        p.echo = Some(echo);
        p
    };
    let with_extra_outline = |mut p: TextStylePreset, stroke: StrokeStyle| {
        p.extra_outlines.push(stroke);
        p
    };
    vec![
        base(
            "sys:text_white",
            "白フチ",
            Rgba::WHITE,
            Some(StrokeStyle {
                color: Rgba::BLACK,
                width_px: 4.0,
            }),
        ),
        base(
            "sys:text_black",
            "黒フチ",
            Rgba::BLACK,
            Some(StrokeStyle {
                color: Rgba::WHITE,
                width_px: 4.0,
            }),
        ),
        base("sys:text_plain", "フチなし黒", Rgba::BLACK, None),
        base(
            "sys:text_quiet",
            "小声グレー",
            Rgba::new(120, 120, 120, 255),
            None,
        ),
        with_shadow(
            base(
                "sys:text_caption",
                "字幕",
                Rgba::WHITE,
                Some(StrokeStyle {
                    color: Rgba::BLACK,
                    width_px: 3.0,
                }),
            ),
            TextShadowStyle {
                color: Rgba::new(0, 0, 0, 170),
                offset: (3.0, 3.0),
                blur_px: 5.0,
                spread_px: 1.0,
            },
        ),
        with_bg(
            base("sys:text_plate", "半透明帯", Rgba::WHITE, None),
            TextBackgroundStyle {
                fill: Rgba::new(0, 0, 0, 150),
                padding_px: 12.0,
                corner_px: 8.0,
            },
        ),
        {
            let mut p = base(
                "sys:text_hollow",
                "中抜き",
                Rgba::TRANSPARENT,
                Some(StrokeStyle {
                    color: Rgba::WHITE,
                    width_px: 4.0,
                }),
            );
            p.extra_outlines.push(StrokeStyle {
                color: Rgba::BLACK,
                width_px: 7.0,
            });
            p
        },
        with_glow(
            base(
                "sys:text_neon",
                "ネオン",
                Rgba::new(120, 245, 255, 255),
                Some(StrokeStyle {
                    color: Rgba::new(10, 40, 80, 255),
                    width_px: 2.0,
                }),
            ),
            TextGlowStyle {
                color: Rgba::new(60, 220, 255, 170),
                radius_px: 12.0,
                spread_px: 2.0,
            },
        ),
        with_echo(
            with_extra_outline(
                base("sys:text_echo", "Echo", Rgba::WHITE, None),
                StrokeStyle {
                    color: Rgba::BLACK,
                    width_px: 3.0,
                },
            ),
            TextEchoStyle {
                color: Rgba::new(40, 90, 210, 150),
                offset: (6.0, 5.0),
                count: 3,
            },
        ),
    ]
}

/// 組み込み本体 (吹き出し) プリセット。`BubblePreset` の各形から生成する。
fn system_shape_presets() -> Vec<ShapeStylePreset> {
    BubblePreset::ALL
        .iter()
        .enumerate()
        .map(|(i, p)| ShapeStylePreset {
            id: format!("sys:shape_{i}"),
            name: p.label().to_string(),
            shape: p.shape(),
            tail_kind: p.tail_kind(),
            fill: Some(Rgba::WHITE),
            fill_opacity: 1.0,
            outline: StrokeStyle {
                color: Rgba::BLACK,
                width_px: p.outline_width(),
            },
            padding_px: 16.0,
        })
        .collect()
}

/// 組み込みウィンドウプリセット。追加ダイアログと同じ `WIN_PRESETS` の見た目から生成する
/// (サイズ/位置は既定。適用後に利用者がリサイズできる)。
fn system_window_presets(font: &str) -> Vec<WindowStylePreset> {
    WIN_PRESETS
        .iter()
        .enumerate()
        .map(|(i, p)| {
            let mut w = MessageWindowObject {
                position: WindowPosition::Free,
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
            w.text.color = p.text_color;
            w.text.font_key = font.to_string();
            w.name_plate.name.font_key = font.to_string();
            WindowStylePreset::from_window(format!("sys:win_{i}"), p.label.to_string(), &w)
        })
        .collect()
}

// ── オノマトペプリセット (Inc 4c、ラボ ONOMATOPOEIA_PRESETS 準拠) ───────────
//
// 各プリセットは装飾フォント (追加パック同梱の OFL フォント) を `font_candidate` で
// 指定し、`resolve_onomatopoeia_font` が実フォント名へ解決する。サイズはラボの ~760px
// 基準値で、追加時にソース解像度へ比例スケールする。

/// オノマトペ追加ピッカーの 1 セル幅 (実フォントサンプルを大きめに見せる)。
const ONOMATO_CELL_W: f32 = 184.0;

/// フォント選択ダイアログの 1 カード寸法 (見本ストリップ + フォント名)。
const FONT_CARD_W: f32 = 220.0;
const FONT_CARD_H: f32 = 70.0;

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
/// 一覧カード用の 1 行ラベル先頭の本文抜粋 (最初の行を 12 文字で省略、ラボ `short` 相当)。
fn short_excerpt(s: &str) -> String {
    let line = s.lines().next().unwrap_or("");
    if line.chars().count() > 12 {
        let t: String = line.chars().take(12).collect();
        format!("{t}…")
    } else {
        line.to_string()
    }
}

/// オブジェクト一覧カードのラベル「種類: 本文抜粋」(ラボ `draw_object_list` 相当)。
fn object_list_label(o: &AnnotationObject) -> String {
    let detail = match &o.kind {
        AnnotationKind::Bubble(b) => short_excerpt(&b.text.text),
        AnnotationKind::Text(t) => short_excerpt(&t.text),
        AnnotationKind::MessageWindow(w) => short_excerpt(&w.text.text),
        AnnotationKind::Stamp(s) => crate::comic_stamp::stamp_label(&s.source).to_string(),
    };
    format!("{}: {}", kind_label(o), detail)
}

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
        let label = object_list_label(o);
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
                                .truncate()
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
/// 詳細パネルの中身。ラボ `draw_properties` と同じ **2 層構造**:
/// ①「常時表示エリア」(タブの上) = よく使う 本文テキスト + 記号挿入 / (吹) 自動サイズ /
/// プリセットバー (セリフ + 本体/ウィンドウ) / (吹) 構造トグル (結合・しっぽ) / (窓) 名前ヘッダ。
/// ②「詳細タブ」= 種別ごとの細かい設定。`window_overflow` は窓本文が枠から溢れているか
/// (呼び出し側が fonts 込みで判定して渡す。赤枠警告に使う)。
fn edit_object_ui(
    ui: &mut egui::Ui,
    o: &mut AnnotationObject,
    tab: &mut TextPropTab,
    changed: &mut bool,
    open_stamp_replace: &mut bool,
    open_font_picker: &mut bool,
    presets: &mut PresetCtx,
    tail_stash: &mut HashMap<u64, Tail>,
    window_overflow: bool,
) {
    ui.strong(kind_label(o));
    ui.separator();

    // 回転 (全種共通、タブの外)。mIV 独自 (ラボにはスライダーは無いがそのまま残す)。
    let mut deg = o.rotation_rad.to_degrees();
    if ui
        .add(egui::Slider::new(&mut deg, -180.0..=180.0).text("回転°"))
        .changed()
    {
        o.rotation_rad = deg.to_radians();
        *changed = true;
    }
    let pivot = o.pivot;
    let obj_id = o.id;

    // スタンプは画像プロパティのみ (常時表示エリア / タブなし)。早期 return。
    if let AnnotationKind::Stamp(s) = &mut o.kind {
        stamp_ui(ui, s, changed, open_stamp_replace);
        return;
    }

    let is_bubble = matches!(o.kind, AnnotationKind::Bubble(_));
    let is_window = matches!(o.kind, AnnotationKind::MessageWindow(_));

    // 選択種別に合わせて現在タブを正規化 (持ち越しを補正)。
    if !is_bubble && !is_window {
        *tab = TextPropTab::Serifu;
    } else if is_window && matches!(*tab, TextPropTab::Tail | TextPropTab::Deco) {
        *tab = TextPropTab::Body;
    } else if is_bubble && *tab == TextPropTab::Parts {
        *tab = TextPropTab::Body;
    }

    // ===== 常時表示エリア (タブの上) =====
    // (窓) 名前ヘッダ — 話者名は頻繁に変えるのでタブではなく上部に置く。名前の「スタイル」
    // (mode/色) 変更はウィンドウリンクを解除 (名前テキスト内容では解除しない = mIV 規約)。
    if is_window {
        if let AnnotationKind::MessageWindow(w) = &mut o.kind {
            let snap = w.clone();
            window_name_header_ui(ui, w, changed);
            if w.style_preset_link.is_some() && window_style_diverged(&snap, w) {
                w.style_preset_link = None;
            }
        }
    }

    // 本文テキスト欄 + (記法 ON 時) 記号挿入。窓が枠から溢れていれば赤枠 + 警告。
    // 本文内容の編集はプリセットリンクを解除しない (mIV 規約)。
    if is_window && window_overflow {
        ui.colored_label(
            egui::Color32::from_rgb(235, 100, 100),
            "(!) テキストが枠に収まっていません",
        );
        egui::Frame::new()
            .stroke(egui::Stroke::new(2.0, egui::Color32::from_rgb(220, 70, 70)))
            .inner_margin(3.0)
            .show(ui, |ui| {
                if let Some(tb) = o.text_block_mut() {
                    text_body_ui(ui, tb, changed, obj_id);
                }
            });
    } else if let Some(tb) = o.text_block_mut() {
        text_body_ui(ui, tb, changed, obj_id);
    }

    // (吹) 自動サイズ — 記号挿入のすぐ下 (頻繁に切り替えるので本体タブに埋めない)。
    if let AnnotationKind::Bubble(b) = &mut o.kind {
        bubble_autosize_toggle_ui(ui, b, changed);
    }

    // プリセットバー (色帯): セリフ (常時) + 本体 (吹) / ウィンドウ (窓)。
    ui.add_space(4.0);
    // 窓に対しテキストプリセットを当てると本文スタイルが変わり、ウィンドウプリセットが捉える
    // 本文スタイルから乖離する → ウィンドウリンクを解除する (本文内容では解除しない規約と一貫。
    // Codex P2)。適用検出のため当てる前の本文スタイルを控える。
    let win_text_snap = match &o.kind {
        AnnotationKind::MessageWindow(w) => Some(w.text.clone()),
        _ => None,
    };
    draw_section_bar(ui, TextPropTab::Serifu.color(), |ui| {
        if let Some(tb) = o.text_block_mut() {
            text_preset_bar(ui, tb, presets, changed);
        }
    });
    if let (Some(snap), AnnotationKind::MessageWindow(w)) = (&win_text_snap, &mut o.kind) {
        if w.style_preset_link.is_some() && text_style_diverged(snap, &w.text) {
            w.style_preset_link = None;
        }
    }
    if let AnnotationKind::Bubble(b) = &mut o.kind {
        draw_section_bar(ui, TextPropTab::Body.color(), |ui| {
            shape_preset_bar(ui, b, pivot, presets, changed);
        });
    } else if let AnnotationKind::MessageWindow(w) = &mut o.kind {
        draw_section_bar(ui, TextPropTab::Body.color(), |ui| {
            window_preset_bar(ui, w, presets, changed);
        });
    }

    // (吹) 構造トグル: 結合 / しっぽ表示。戻り値 = しっぽタブを有効化するか。
    let tail_enabled = match &mut o.kind {
        AnnotationKind::Bubble(b) => {
            bubble_struct_toggles_ui(ui, b, changed, obj_id, pivot, tail_stash)
        }
        _ => false,
    };
    // しっぽタブが無効なのに選択されていたら本体へ退避。
    if is_bubble && *tab == TextPropTab::Tail && !tail_enabled {
        *tab = TextPropTab::Body;
    }

    // ===== 詳細タブ =====
    ui.add_space(6.0);
    ui.separator();
    match &mut o.kind {
        AnnotationKind::Text(t) => {
            // テキストは セリフ のみ (タブ行なし)。スタイルを色帯で。
            draw_section_bar(ui, TextPropTab::Serifu.color(), |ui| {
                let snap = t.clone();
                text_block_ui(ui, t, changed, open_font_picker);
                // 個別スタイル編集でプリセットリンクを解除 (本文内容の編集では外さない)。
                if t.preset_link.is_some() && text_style_diverged(&snap, t) {
                    t.preset_link = None;
                }
            });
        }
        AnnotationKind::Bubble(b) => {
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing = egui::vec2(4.0, 4.0);
                for t in [
                    TextPropTab::Serifu,
                    TextPropTab::Body,
                    TextPropTab::Tail,
                    TextPropTab::Deco,
                ] {
                    let enabled = match t {
                        TextPropTab::Tail => tail_enabled,
                        _ => true,
                    };
                    if prop_tab_button(ui, t, *tab == t, enabled, t.label()) {
                        *tab = t;
                    }
                }
            });
            ui.add_space(4.0);
            let cur = *tab;
            draw_section_bar(ui, cur.color(), |ui| match cur {
                TextPropTab::Serifu => {
                    let snap = b.text.clone();
                    text_block_ui(ui, &mut b.text, changed, open_font_picker);
                    if b.text.preset_link.is_some() && text_style_diverged(&snap, &b.text) {
                        b.text.preset_link = None;
                    }
                }
                TextPropTab::Tail => {
                    let snap = b.clone();
                    bubble_tail_ui(ui, b, changed);
                    // しっぽ種別はプリセット (tail_kind) に含まれる → 個別編集で link 解除。
                    if b.shape_preset_link.is_some() && shape_style_diverged(&snap, b) {
                        b.shape_preset_link = None;
                    }
                }
                TextPropTab::Deco => {
                    // 飾りは形状プリセットに含まれない (ラボと同じ)。link は切らない。
                    bubble_deco_ui(ui, b, changed);
                }
                // Body (+ Parts/正規化済みは Body に倒れる)。
                _ => {
                    let snap = b.clone();
                    bubble_body_ui(ui, b, changed);
                    if b.shape_preset_link.is_some() && shape_style_diverged(&snap, b) {
                        b.shape_preset_link = None;
                    }
                }
            });
        }
        AnnotationKind::MessageWindow(w) => {
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing = egui::vec2(4.0, 4.0);
                for (t, lbl) in [
                    (TextPropTab::Serifu, "セリフ"),
                    (TextPropTab::Body, "枠"),
                    (TextPropTab::Parts, "部品"),
                ] {
                    if prop_tab_button(ui, t, *tab == t, true, lbl) {
                        *tab = t;
                    }
                }
            });
            ui.add_space(4.0);
            let cur = *tab;
            draw_section_bar(ui, cur.color(), |ui| match cur {
                TextPropTab::Serifu => {
                    // 本文スタイル編集でウィンドウリンクを解除 (本文内容では外さない)。
                    let snap = w.text.clone();
                    text_block_ui(ui, &mut w.text, changed, open_font_picker);
                    if w.style_preset_link.is_some() && text_style_diverged(&snap, &w.text) {
                        w.style_preset_link = None;
                    }
                }
                TextPropTab::Parts => {
                    // 名前プレート詳細 / 立ち絵枠 / 続き指標。個別編集でウィンドウリンク解除。
                    let snap = w.clone();
                    window_parts_ui(ui, w, changed);
                    if w.style_preset_link.is_some() && window_style_diverged(&snap, w) {
                        w.style_preset_link = None;
                    }
                }
                // 枠 (Body)。
                _ => {
                    let snap = w.clone();
                    window_body_ui(ui, w, changed);
                    if w.style_preset_link.is_some() && window_style_diverged(&snap, w) {
                        w.style_preset_link = None;
                    }
                }
            });
        }
        // スタンプは上で早期 return 済み。
        AnnotationKind::Stamp(_) => {}
    }
}

/// 常時表示: 本文テキスト欄 + (記法 ON 時) カーソル位置への記号挿入ボタン。記法トグルと
/// 記号セット選択は セリフ タブ側。本文内容の編集はプリセットリンクを切らない (mIV 規約)
/// ので、ここでは `changed` を立てるだけ (リンク解除は呼び出し側のスタイル差分判定に任せる)。
fn text_body_ui(ui: &mut egui::Ui, t: &mut TextBlock, changed: &mut bool, sel: u64) {
    // 安定 id を与えて記号挿入後にキャレットを復元できるようにする。
    let text_edit_id = egui::Id::new(("comic_text_edit", sel));
    let te_out = egui::TextEdit::multiline(&mut t.text)
        .id(text_edit_id)
        .desired_rows(3)
        .desired_width(f32::INFINITY)
        .hint_text("テキスト")
        .show(ui);
    if te_out.response.changed() {
        *changed = true;
    }
    let text_sel: Option<(usize, usize)> = te_out
        .cursor_range
        .map(|r| (r.primary.index, r.secondary.index));
    // 記法 ON のとき、本文欄の直下に記号挿入ボタンを出す。選択範囲があればそれを囲み、
    // 無ければキャレット位置 (末尾) に空ペアを挿入する (ラボ `draw_text_body` 相当)。
    if t.markup_enabled {
        let rules = t.markup_rules.clone();
        ui.horizontal_wrapped(|ui| {
            ui.label("記号挿入:");
            for rule in &rules {
                let dir_label = match rule.dir {
                    InlineDir::TateChuYoko => "縦中横",
                    InlineDir::Sideways => "横倒し",
                    InlineDir::Upright => "正立",
                };
                if ui
                    .button(format!("{}{} {}", rule.open, rule.close, dir_label))
                    .clicked()
                {
                    let (a, b) = text_sel.unwrap_or_else(|| {
                        let n = t.text.chars().count();
                        (n, n)
                    });
                    let (s, e) = (a.min(b), a.max(b));
                    let caret = insert_markers(&mut t.text, s, e, rule.open, rule.close);
                    let ctx = ui.ctx();
                    if let Some(mut st) = egui::text_edit::TextEditState::load(ctx, text_edit_id) {
                        let cc = egui::text::CCursor::new(caret);
                        st.cursor
                            .set_char_range(Some(egui::text::CCursorRange::one(cc)));
                        st.store(ctx, text_edit_id);
                    }
                    ctx.memory_mut(|m| m.request_focus(text_edit_id));
                    *changed = true;
                }
            }
        });
    }
}

/// 常時表示 (吹き出し): 自動サイズトグル。頻繁に切り替えるので本体タブではなく上部に置く
/// (ラボ `draw_bubble_autosize_toggle` 相当)。mIV はオフ時の形状凍結は行わない (現挙動維持)。
fn bubble_autosize_toggle_ui(ui: &mut egui::Ui, b: &mut BubbleObject, changed: &mut bool) {
    let mut auto = b.auto_size;
    if ui
        .checkbox(&mut auto, "吹き出し自動サイズ")
        .on_hover_text("文字に合わせて形状サイズを決める")
        .changed()
    {
        b.auto_size = auto;
        *changed = true;
    }
}

/// 常時表示 (吹き出し): 構造トグル = 結合 / しっぽ表示 (ラボ `draw_bubble_toggles` 相当)。
/// 戻り値はしっぽタブを有効化するか (= しっぽが存在し、かつ形状がしっぽを描く)。off→on は
/// stash から復元して位置を覚える。結合・しっぽともに非対応形状ではトグルを無効化する。
fn bubble_struct_toggles_ui(
    ui: &mut egui::Ui,
    b: &mut BubbleObject,
    changed: &mut bool,
    obj_id: u64,
    pivot: (f32, f32),
    tail_stash: &mut HashMap<u64, Tail>,
) -> bool {
    ui.add_space(2.0);
    // 結合 (「すぐ下」= z 順で直下の吹き出しと union)。**直下のみ**なのは意図的 — 間に別
    // オブジェクトが挟まる吹き出しまで飛ばして融合すると z 重なり順が壊れるため (comic-core
    // raster の merge グルーピングが連続 z だけを 1 ユニット化する仕様)。ラベルを「すぐ下の
    // 吹き出しと結合」にして利用者へ直下条件を伝える。塗りつぶせる本体を持つ形状のみ対応 —
    // ぼかし系 / 線描画系 / テキストのみ (意識 / 集中線 / 流線 / なし) は不可なのでトグルを
    // 無効化し、別形状で立った stale フラグをクリアする。
    let merge_supported = comic_core::shape_is_mergeable(&b.shape);
    if !merge_supported && b.merge_with_below {
        b.merge_with_below = false;
        *changed = true;
    }
    let merge_resp = ui.add_enabled(
        merge_supported,
        egui::Checkbox::new(&mut b.merge_with_below, "すぐ下の吹き出しと結合"),
    );
    if merge_supported && merge_resp.changed() {
        *changed = true;
    }
    if !merge_supported {
        merge_resp.on_disabled_hover_text("この形状は結合に対応していません");
    }

    // しっぽ表示 (off→on で stash 復元)。しっぽを描かない形状 (集中線 / 流線 / 意識 / なし)
    // ではトグルを無効化する (見えない選択可能なしっぽ形状を作らせない)。
    let tail_supported = comic_core::shape_renders_tail(&b.shape);
    let mut has_tail = b.tail.is_some();
    let tail_resp = ui.add_enabled(
        tail_supported,
        egui::Checkbox::new(&mut has_tail, "しっぽを表示"),
    );
    if tail_supported && tail_resp.changed() {
        if has_tail {
            b.tail = Some(
                tail_stash
                    .remove(&obj_id)
                    .unwrap_or_else(|| default_bubble_tail(pivot)),
            );
        } else if let Some(t) = b.tail.take() {
            tail_stash.insert(obj_id, t);
        }
        // しっぽ種別は形状プリセットに含まれる → 付与/除去で link 解除 (glow off)。
        b.shape_preset_link = None;
        *changed = true;
    }
    if !tail_supported {
        tail_resp.on_disabled_hover_text("この形状はしっぽに対応していません");
    }

    b.tail.is_some() && tail_supported
}

/// 常時表示 (ウィンドウ): 名前ヘッダ = 表示モード + 名前色 + 話者名。話者名は頻繁に変える
/// ので部品タブではなく上部に置く (ラボ `draw_window_name_header` 相当)。プレートの詳細
/// スタイル (サイズ / 塗り / 枠 / 角丸 / 余白 / オフセット) は 部品 タブ側 (`window_parts_ui`)。
fn window_name_header_ui(ui: &mut egui::Ui, w: &mut MessageWindowObject, changed: &mut bool) {
    ui.horizontal(|ui| {
        ui.label("名前");
        egui::ComboBox::from_id_salt("win_name_mode")
            .selected_text(name_plate_mode_label(w.name_plate.mode))
            .show_ui(ui, |ui| {
                for m in [
                    NamePlateMode::None,
                    NamePlateMode::Inline,
                    NamePlateMode::Boxed,
                    NamePlateMode::Above,
                ] {
                    if ui
                        .selectable_value(&mut w.name_plate.mode, m, name_plate_mode_label(m))
                        .changed()
                    {
                        *changed = true;
                    }
                }
            });
        if w.name_plate.mode != NamePlateMode::None {
            let mut col = to_c32(w.name_plate.name.color);
            if ui.color_edit_button_srgba(&mut col).changed() {
                w.name_plate.name.color = from_c32(col);
                *changed = true;
            }
        }
    });
    if w.name_plate.mode != NamePlateMode::None {
        *changed |= ui
            .add(
                egui::TextEdit::singleline(&mut w.name_plate.name.text)
                    .desired_width(f32::INFINITY)
                    .hint_text("話者名"),
            )
            .changed();
    }
}

/// セリフプリセットが捉えるスタイル各フィールドのいずれかが変わったか (本文内容と
/// preset_link は無視)。個別編集でリンクを解除する判定に使う。
fn text_style_diverged(a: &TextBlock, b: &TextBlock) -> bool {
    a.font_key != b.font_key
        || a.size_px != b.size_px
        || a.color != b.color
        || a.orientation != b.orientation
        || a.align != b.align
        || a.line_gap != b.line_gap
        || a.letter_gap != b.letter_gap
        || a.outline != b.outline
        || a.extra_outlines != b.extra_outlines
        || a.shadow != b.shadow
        || a.glow != b.glow
        || a.background != b.background
        || a.echo != b.echo
        || a.auto_tcy != b.auto_tcy
        || a.markup_enabled != b.markup_enabled
        || a.markup_rules != b.markup_rules
        || a.bold != b.bold
        || a.italic != b.italic
}

/// 本体プリセット (ShapeStylePreset) が捉えるフィールドのいずれかが変わったか。しっぽの
/// 位置は対象外 (preset は tail_kind のみ)。
fn shape_style_diverged(a: &BubbleObject, b: &BubbleObject) -> bool {
    a.shape != b.shape
        || a.tail.map(|t| t.kind) != b.tail.map(|t| t.kind)
        || a.fill != b.fill
        || a.fill_opacity != b.fill_opacity
        || a.outline != b.outline
        || a.padding_px != b.padding_px
}

/// ウィンドウプリセットが適用する視覚フィールドのいずれかが変わったか。レイアウト
/// (size/position) は preset が適用しないので対象外。本文 TEXT も対象外 (別タブで編集)。
fn window_style_diverged(a: &MessageWindowObject, b: &MessageWindowObject) -> bool {
    a.corner_px != b.corner_px
        || a.fill_mode != b.fill_mode
        || a.fill != b.fill
        || a.fill_opacity != b.fill_opacity
        || a.gradient_to != b.gradient_to
        || a.scrim_dense_side != b.scrim_dense_side
        || a.frame != b.frame
        || a.outline != b.outline
        || a.frame_gap_px != b.frame_gap_px
        || a.shadow != b.shadow
        || a.padding != b.padding
        || a.v_anchor != b.v_anchor
        || a.wrap != b.wrap
        || a.indicator != b.indicator
        || a.indicator_auto != b.indicator_auto
        || a.portrait != b.portrait
        || name_plate_style_diverged(&a.name_plate, &b.name_plate)
}

/// 名前プレートの**装飾**差 (名前の TEXT 内容とフォントは比較から除外)。
///
/// 話者名そのもの (name.text) はインスタンス固有の内容であってプリセットが定義する
/// スタイルではないので、名前を打ち替えてもウィンドウリンクは切らない。これは mIV の
/// プリセット規約 (本文内容では link を切らない = `text_style_diverged` も `text` を無視) と
/// 一貫した挙動で、ラボ `draw_window_name_header` が名前編集でも link を切るのとは意図的に
/// 異なる (Codex P3。mIV 側の規約の方が一貫しているのでこちらを採用)。プレートの
/// スタイル (mode / 色 / 塗り / 枠 / 角丸 / 余白 / オフセット / サイズ) を変えれば
/// ちゃんと link は切れる。name.font_key は名前専用フォント UI を持たないので無視。
fn name_plate_style_diverged(a: &comic_core::NamePlate, b: &comic_core::NamePlate) -> bool {
    let mut an = a.clone();
    let mut bn = b.clone();
    an.name.text = String::new();
    bn.name.text = String::new();
    an.name.font_key = String::new();
    bn.name.font_key = String::new();
    an != bn
}

/// プリセットの保存/更新対象の種別 (= どのプリセットバーから要求されたか)。常時表示エリアで
/// セリフ + 本体/ウィンドウのバーが同時に出るようになったため、要求がどの種類のプリセットを
/// 対象とするかを明示しないと、オブジェクト種別で誤判定する (Codex P2: 吹き出しのセリフバーの
/// 「保存」が本体プリセットを保存してしまう問題)。
#[derive(Clone, Copy, PartialEq, Eq)]
enum PresetTarget {
    Text,
    Shape,
    Window,
}

/// プリセットバー周りの状態。詳細パネルの外 (draw_text_panel) で結果を処理する。
#[derive(Default)]
struct PresetRequests {
    /// 現在のスタイルを新規ユーザープリセットとして保存 (対象種別付き)。
    save: Option<PresetTarget>,
    /// (対象種別, id): この user プリセットを現在のスタイルで更新。
    update: Option<(PresetTarget, String)>,
    /// この user プリセット id を削除 (id ベースなので種別非依存)。
    delete: Option<String>,
}

/// 詳細パネルの編集 UI へ渡すプリセット文脈 (一覧スナップショット + 名前入力 + 要求)。
struct PresetCtx<'a> {
    text: &'a [TextStylePreset],
    shape: &'a [ShapeStylePreset],
    window: &'a [WindowStylePreset],
    name_input: &'a mut String,
    reqs: &'a mut PresetRequests,
}

/// 共通プリセットバー: 適用ボタン群 (リンク中は点灯) + user の × 削除 + 名前入力 +
/// 「現在を保存」/「更新」。適用された id を返し、保存/更新/削除は `reqs` に立てる。
fn preset_buttons_ui(
    ui: &mut egui::Ui,
    target: PresetTarget,
    entries: &[(String, String, bool)], // (id, name, is_system)
    linked: Option<&str>,
    name_input: &mut String,
    reqs: &mut PresetRequests,
) -> Option<String> {
    let mut applied: Option<String> = None;
    ui.horizontal_wrapped(|ui| {
        for (id, name, is_system) in entries {
            let is_linked = linked == Some(id.as_str());
            let mut btn = egui::Button::new(egui::RichText::new(name).size(12.0));
            if is_linked {
                btn = btn.fill(egui::Color32::from_rgb(50, 96, 140)); // リンク中は点灯
            }
            if ui.add(btn).clicked() {
                applied = Some(id.clone());
            }
            if !is_system && ui.small_button("×").on_hover_text("削除").clicked() {
                reqs.delete = Some(id.clone());
            }
        }
    });
    ui.horizontal(|ui| {
        ui.add(
            egui::TextEdit::singleline(name_input)
                .hint_text("プリセット名")
                .desired_width(110.0),
        );
        if ui.button("現在を保存").clicked() {
            reqs.save = Some(target);
        }
        if let Some(id) = linked {
            if !crate::comic_presets::is_system_preset(id) && ui.button("更新").clicked() {
                reqs.update = Some((target, id.to_string()));
            }
        }
    });
    applied
}

/// セリフ (テキスト) プリセットバー。クリックで `t` に適用 + リンク。
fn text_preset_bar(
    ui: &mut egui::Ui,
    t: &mut TextBlock,
    presets: &mut PresetCtx,
    changed: &mut bool,
) {
    ui.label(egui::RichText::new("セリフプリセット").strong());
    let entries: Vec<(String, String, bool)> = presets
        .text
        .iter()
        .map(|p| {
            (
                p.id.clone(),
                p.name.clone(),
                crate::comic_presets::is_system_preset(&p.id),
            )
        })
        .collect();
    let applied = preset_buttons_ui(
        ui,
        PresetTarget::Text,
        &entries,
        t.preset_link.as_deref(),
        presets.name_input,
        presets.reqs,
    );
    if let Some(id) = applied {
        if let Some(p) = presets.text.iter().find(|p| p.id == id) {
            p.apply_to(t);
            *changed = true;
        }
    }
    ui.separator();
}

/// 本体 (吹き出し) プリセットバー。クリックで `b` に適用 + リンク。
fn shape_preset_bar(
    ui: &mut egui::Ui,
    b: &mut BubbleObject,
    pivot: (f32, f32),
    presets: &mut PresetCtx,
    changed: &mut bool,
) {
    ui.label(egui::RichText::new("本体プリセット").strong());
    let entries: Vec<(String, String, bool)> = presets
        .shape
        .iter()
        .map(|p| {
            (
                p.id.clone(),
                p.name.clone(),
                crate::comic_presets::is_system_preset(&p.id),
            )
        })
        .collect();
    let applied = preset_buttons_ui(
        ui,
        PresetTarget::Shape,
        &entries,
        b.shape_preset_link.as_deref(),
        presets.name_input,
        presets.reqs,
    );
    if let Some(id) = applied {
        if let Some(p) = presets.shape.iter().find(|p| p.id == id) {
            p.apply_to(b, default_bubble_tail(pivot));
            *changed = true;
        }
    }
    ui.separator();
}

/// ウィンドウプリセットバー。クリックで `w` に適用 + リンク。
fn window_preset_bar(
    ui: &mut egui::Ui,
    w: &mut MessageWindowObject,
    presets: &mut PresetCtx,
    changed: &mut bool,
) {
    ui.label(egui::RichText::new("ウィンドウプリセット").strong());
    let entries: Vec<(String, String, bool)> = presets
        .window
        .iter()
        .map(|p| {
            (
                p.id.clone(),
                p.name.clone(),
                crate::comic_presets::is_system_preset(&p.id),
            )
        })
        .collect();
    let applied = preset_buttons_ui(
        ui,
        PresetTarget::Window,
        &entries,
        w.style_preset_link.as_deref(),
        presets.name_input,
        presets.reqs,
    );
    if let Some(id) = applied {
        if let Some(p) = presets.window.iter().find(|p| p.id == id) {
            p.apply_to(w);
            *changed = true;
        }
    }
    ui.separator();
}

/// セリフ タブの中身: TextBlock の **スタイル** 編集 (フォント・サイズ・色・太字/斜体・
/// 向き・整列・袋文字・自動縦中横・記法・行間字間)。本文テキスト欄と記号挿入は常時表示の
/// `text_body_ui` 側へ分離した (ラボ `draw_text_font` + `draw_serifu_tab` 相当)。
fn text_block_ui(
    ui: &mut egui::Ui,
    t: &mut TextBlock,
    changed: &mut bool,
    open_font_picker: &mut bool,
) {
    // フォント選択 (現在のフォント名 + 選択ボタン)。ピッカーは別ダイアログで開く。
    ui.horizontal(|ui| {
        ui.label("フォント");
        let cur = if t.font_key.is_empty() || t.font_key == crate::comic_overlay::COMIC_FONT_KEY {
            "既定 (システム)".to_string()
        } else {
            t.font_key.clone()
        };
        if ui
            .button(egui::RichText::new(cur).size(12.0))
            .on_hover_text("フォントを見本から選択")
            .clicked()
        {
            *open_font_picker = true;
        }
    });
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
    text_effects_ui(ui, t, changed);
    let mut tcy = t.auto_tcy;
    if ui
        .checkbox(&mut tcy, "自動縦中横")
        .on_hover_text("縦書きで数字 2-3 桁や !? を横向きに組む")
        .changed()
    {
        t.auto_tcy = tcy;
        *changed = true;
    }
    // 記法 (記号で囲んで縦中横/横倒し)。トグル + 記号セット選択。挿入ボタンは本文欄の下。
    if ui
        .checkbox(&mut t.markup_enabled, "記法を使う (記号で縦中横/横倒し)")
        .changed()
    {
        *changed = true;
    }
    if t.markup_enabled {
        // 3 つの記号セット。先頭ペア=縦中横 / 2 番目=横倒し。現在値は open/close 一致で判定。
        let sets: [(&str, Vec<MarkupRule>); 3] = [
            ("[ ]  { }", comic_core::markup_rules_brackets()),
            ("〈 〉  《 》", comic_core::markup_rules_angle()),
            ("〚 〛  〘 〙", comic_core::markup_rules_white()),
        ];
        let cur_idx = sets
            .iter()
            .position(|(_, r)| marker_pairs_eq(&t.markup_rules, r));
        let cur_label = cur_idx.map(|i| sets[i].0).unwrap_or("カスタム");
        ui.horizontal(|ui| {
            ui.label("記号セット");
            egui::ComboBox::from_id_salt("marker_set_combo")
                .selected_text(cur_label)
                .show_ui(ui, |ui| {
                    for (i, (label, rules)) in sets.iter().enumerate() {
                        if ui.selectable_label(cur_idx == Some(i), *label).clicked() {
                            t.markup_rules = rules.clone();
                            *changed = true;
                        }
                    }
                });
        });
    }
    // 行間 / 字間。
    if ui
        .add(egui::Slider::new(&mut t.line_gap, -20.0..=80.0).text("行間"))
        .changed()
    {
        *changed = true;
    }
    if ui
        .add(egui::Slider::new(&mut t.letter_gap, -10.0..=60.0).text("字間"))
        .changed()
    {
        *changed = true;
    }
}

fn text_effects_ui(ui: &mut egui::Ui, t: &mut TextBlock, changed: &mut bool) {
    ui.add_space(4.0);
    ui.label(egui::RichText::new("文字効果").strong());

    if !t.extra_outlines.is_empty() {
        let mut remove = None;
        for (i, outline) in t.extra_outlines.iter_mut().enumerate() {
            ui.horizontal(|ui| {
                ui.label(format!("外フチ{}", i + 1));
                let mut col = to_c32(outline.color);
                if ui.color_edit_button_srgba(&mut col).changed() {
                    outline.color = from_c32(col);
                    *changed = true;
                }
                *changed |= ui
                    .add(egui::Slider::new(&mut outline.width_px, 0.0..=48.0).text("太さ"))
                    .changed();
                if ui.small_button("削除").clicked() {
                    remove = Some(i);
                }
            });
        }
        if let Some(i) = remove {
            t.extra_outlines.remove(i);
            *changed = true;
        }
    }
    if ui.button("外フチ追加").clicked() {
        let next_w = t.outline.map(|s| s.width_px).unwrap_or(3.0).max(
            t.extra_outlines
                .iter()
                .map(|s| s.width_px)
                .fold(0.0, f32::max),
        ) + 3.0;
        t.extra_outlines.push(StrokeStyle {
            color: Rgba::BLACK,
            width_px: next_w.min(48.0),
        });
        *changed = true;
    }

    let mut has_shadow = t.shadow.is_some();
    if ui.checkbox(&mut has_shadow, "影").changed() {
        t.shadow = if has_shadow {
            Some(TextShadowStyle::default())
        } else {
            None
        };
        *changed = true;
    }
    if let Some(sh) = &mut t.shadow {
        ui.horizontal(|ui| {
            ui.label("影色");
            let mut col = to_c32(sh.color);
            if ui.color_edit_button_srgba(&mut col).changed() {
                sh.color = from_c32(col);
                *changed = true;
            }
            ui.label("X");
            *changed |= ui
                .add(egui::DragValue::new(&mut sh.offset.0).speed(0.5))
                .changed();
            ui.label("Y");
            *changed |= ui
                .add(egui::DragValue::new(&mut sh.offset.1).speed(0.5))
                .changed();
        });
        *changed |= ui
            .add(egui::Slider::new(&mut sh.blur_px, 0.0..=48.0).text("ぼかし"))
            .changed();
        *changed |= ui
            .add(egui::Slider::new(&mut sh.spread_px, 0.0..=24.0).text("広がり"))
            .changed();
    }

    let mut has_glow = t.glow.is_some();
    if ui.checkbox(&mut has_glow, "発光").changed() {
        t.glow = if has_glow {
            Some(TextGlowStyle::default())
        } else {
            None
        };
        *changed = true;
    }
    if let Some(glow) = &mut t.glow {
        ui.horizontal(|ui| {
            ui.label("発光色");
            let mut col = to_c32(glow.color);
            if ui.color_edit_button_srgba(&mut col).changed() {
                glow.color = from_c32(col);
                *changed = true;
            }
        });
        *changed |= ui
            .add(egui::Slider::new(&mut glow.radius_px, 0.0..=64.0).text("半径"))
            .changed();
        *changed |= ui
            .add(egui::Slider::new(&mut glow.spread_px, 0.0..=32.0).text("広がり"))
            .changed();
    }

    let mut has_bg = t.background.is_some();
    if ui.checkbox(&mut has_bg, "背景プレート").changed() {
        t.background = if has_bg {
            Some(TextBackgroundStyle::default())
        } else {
            None
        };
        *changed = true;
    }
    if let Some(bg) = &mut t.background {
        ui.horizontal(|ui| {
            ui.label("背景色");
            let mut col = to_c32(bg.fill);
            if ui.color_edit_button_srgba(&mut col).changed() {
                bg.fill = from_c32(col);
                *changed = true;
            }
        });
        *changed |= ui
            .add(egui::Slider::new(&mut bg.padding_px, 0.0..=80.0).text("余白"))
            .changed();
        *changed |= ui
            .add(egui::Slider::new(&mut bg.corner_px, 0.0..=80.0).text("角丸"))
            .changed();
    }

    let mut has_echo = t.echo.is_some();
    if ui.checkbox(&mut has_echo, "Echo").changed() {
        t.echo = if has_echo {
            Some(TextEchoStyle::default())
        } else {
            None
        };
        *changed = true;
    }
    if let Some(echo) = &mut t.echo {
        ui.horizontal(|ui| {
            ui.label("Echo色");
            let mut col = to_c32(echo.color);
            if ui.color_edit_button_srgba(&mut col).changed() {
                echo.color = from_c32(col);
                *changed = true;
            }
            ui.label("数");
            let mut count = echo.count as i32;
            if ui
                .add(egui::DragValue::new(&mut count).range(1..=12))
                .changed()
            {
                echo.count = count.clamp(1, 12) as u32;
                *changed = true;
            }
        });
        ui.horizontal(|ui| {
            ui.label("X");
            *changed |= ui
                .add(egui::DragValue::new(&mut echo.offset.0).speed(0.5))
                .changed();
            ui.label("Y");
            *changed |= ui
                .add(egui::DragValue::new(&mut echo.offset.1).speed(0.5))
                .changed();
        });
    }
}

/// 形状の `seed` 表示 + 「再生成」ボタン (ラボ `tab_body` の seed 行)。クリックで seed を +1
/// して手続き的形状 (トゲ/こぶ/集中線/流線等) の乱数配置を振り直す。
fn shape_seed_row(ui: &mut egui::Ui, shape_seed: &mut u32, changed: &mut bool) {
    ui.horizontal(|ui| {
        ui.label(format!("seed {shape_seed}"));
        if ui.button("再生成").clicked() {
            *shape_seed = shape_seed.wrapping_add(1);
            *changed = true;
        }
    });
}

/// 吹き出し「本体」タブ (形状・形状別パラメータ・塗り・輪郭・余白)。本文は セリフ タブ、
/// しっぽは しっぽ タブ、自動サイズ・結合は常時表示エリアへ分離している。形状別スライダーの
/// 個別編集は `b.shape` を変えるので、呼び出し側の `shape_style_diverged` 判定でプリセット
/// リンクが解除される (ラボ `tab_body` の match ブロック相当)。
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

    // 形状別パラメータの微調整。自動サイズ ON のときは寸法 (半幅/半高/rx/ry) を隠し、
    // 文字量から決める。形状固有のパラメータ (トゲ数・こぶ数・辺の数・線の本数・角丸・
    // 向き・線の間隔・seed) は自動サイズでも調整可能。
    let auto = b.auto_size;
    match &mut b.shape {
        BubbleShape::Ellipse { rx, ry } => {
            if !auto {
                *changed |= ui
                    .add(egui::Slider::new(rx, 20.0..=800.0).text("rx"))
                    .changed();
                *changed |= ui
                    .add(egui::Slider::new(ry, 20.0..=800.0).text("ry"))
                    .changed();
            }
        }
        BubbleShape::RoundRect {
            half_w,
            half_h,
            corner_px,
        } => {
            if !auto {
                *changed |= ui
                    .add(egui::Slider::new(half_w, 20.0..=800.0).text("半幅"))
                    .changed();
                *changed |= ui
                    .add(egui::Slider::new(half_h, 20.0..=800.0).text("半高"))
                    .changed();
            }
            *changed |= ui
                .add(egui::Slider::new(corner_px, 0.0..=200.0).text("角丸"))
                .changed();
        }
        BubbleShape::Burst {
            rx,
            ry,
            spikes,
            jag,
            shape_seed,
        } => {
            if !auto {
                *changed |= ui
                    .add(egui::Slider::new(rx, 20.0..=800.0).text("rx"))
                    .changed();
                *changed |= ui
                    .add(egui::Slider::new(ry, 20.0..=800.0).text("ry"))
                    .changed();
            }
            *changed |= ui
                .add(egui::Slider::new(spikes, 5..=40).text("トゲ数"))
                .changed();
            *changed |= ui
                .add(egui::Slider::new(jag, 0.2..=0.9).text("トゲの深さ"))
                .changed();
            shape_seed_row(ui, shape_seed, changed);
        }
        BubbleShape::Cloud {
            rx,
            ry,
            lobes,
            amp,
            shape_seed,
        } => {
            if !auto {
                *changed |= ui
                    .add(egui::Slider::new(rx, 20.0..=800.0).text("rx"))
                    .changed();
                *changed |= ui
                    .add(egui::Slider::new(ry, 20.0..=800.0).text("ry"))
                    .changed();
            }
            *changed |= ui
                .add(egui::Slider::new(lobes, 5..=24).text("こぶ数"))
                .changed();
            *changed |= ui
                .add(egui::Slider::new(amp, 0.04..=0.4).text("こぶの深さ"))
                .changed();
            shape_seed_row(ui, shape_seed, changed);
        }
        BubbleShape::Polygon { rx, ry, sides } => {
            if !auto {
                *changed |= ui
                    .add(egui::Slider::new(rx, 20.0..=800.0).text("rx"))
                    .changed();
                *changed |= ui
                    .add(egui::Slider::new(ry, 20.0..=800.0).text("ry"))
                    .changed();
            }
            *changed |= ui
                .add(egui::Slider::new(sides, 3..=12).text("辺の数"))
                .changed();
        }
        BubbleShape::Diamond { half_w, half_h } => {
            if !auto {
                *changed |= ui
                    .add(egui::Slider::new(half_w, 20.0..=800.0).text("半幅"))
                    .changed();
                *changed |= ui
                    .add(egui::Slider::new(half_h, 20.0..=800.0).text("半高"))
                    .changed();
            }
        }
        BubbleShape::Heart { rx, ry } => {
            if !auto {
                *changed |= ui
                    .add(egui::Slider::new(rx, 20.0..=800.0).text("rx"))
                    .changed();
                *changed |= ui
                    .add(egui::Slider::new(ry, 20.0..=800.0).text("ry"))
                    .changed();
            }
        }
        BubbleShape::Arrow {
            half_w,
            half_h,
            dir_rad,
        } => {
            if !auto {
                *changed |= ui
                    .add(egui::Slider::new(half_w, 20.0..=800.0).text("長さ半分"))
                    .changed();
                *changed |= ui
                    .add(egui::Slider::new(half_h, 20.0..=800.0).text("幅半分"))
                    .changed();
            }
            let mut deg = dir_rad.to_degrees();
            if ui
                .add(egui::Slider::new(&mut deg, -180.0..=180.0).text("向き(度)"))
                .changed()
            {
                *dir_rad = deg.to_radians();
                *changed = true;
            }
        }
        BubbleShape::Soft {
            half_w,
            half_h,
            corner_px,
            shape_seed,
        } => {
            if !auto {
                *changed |= ui
                    .add(egui::Slider::new(half_w, 20.0..=800.0).text("半幅"))
                    .changed();
                *changed |= ui
                    .add(egui::Slider::new(half_h, 20.0..=800.0).text("半高"))
                    .changed();
            }
            *changed |= ui
                .add(egui::Slider::new(corner_px, 0.0..=200.0).text("角丸"))
                .changed();
            shape_seed_row(ui, shape_seed, changed);
        }
        BubbleShape::MotionLines {
            rx,
            ry,
            count,
            shape_seed,
        } => {
            if !auto {
                *changed |= ui
                    .add(egui::Slider::new(rx, 40.0..=1000.0).text("外半径rx"))
                    .changed();
                *changed |= ui
                    .add(egui::Slider::new(ry, 40.0..=1000.0).text("外半径ry"))
                    .changed();
            }
            *changed |= ui
                .add(egui::Slider::new(count, 8..=200).text("線の本数"))
                .changed();
            shape_seed_row(ui, shape_seed, changed);
        }
        BubbleShape::SpeedLines {
            half_w,
            half_h,
            dir_rad,
            count,
            shape_seed,
        } => {
            if !auto {
                *changed |= ui
                    .add(egui::Slider::new(half_w, 40.0..=1000.0).text("半幅"))
                    .changed();
                *changed |= ui
                    .add(egui::Slider::new(half_h, 40.0..=1000.0).text("半高"))
                    .changed();
            }
            *changed |= ui
                .add(egui::Slider::new(count, 8..=200).text("線の本数"))
                .changed();
            let mut deg = dir_rad.to_degrees();
            if ui
                .add(egui::Slider::new(&mut deg, -180.0..=180.0).text("向き(度)"))
                .changed()
            {
                *dir_rad = deg.to_radians();
                *changed = true;
            }
            shape_seed_row(ui, shape_seed, changed);
        }
        BubbleShape::TextOnly { half_w, half_h } => {
            if !auto {
                *changed |= ui
                    .add(egui::Slider::new(half_w, 20.0..=800.0).text("半幅"))
                    .changed();
                *changed |= ui
                    .add(egui::Slider::new(half_h, 20.0..=800.0).text("半高"))
                    .changed();
            }
            ui.label(
                egui::RichText::new("枠なし・テキストのみ (塗り/枠は描画されません)")
                    .small()
                    .weak(),
            );
        }
        BubbleShape::Concentration { rx, ry, shape_seed } => {
            if !auto {
                *changed |= ui
                    .add(egui::Slider::new(rx, 20.0..=800.0).text("rx"))
                    .changed();
                *changed |= ui
                    .add(egui::Slider::new(ry, 20.0..=800.0).text("ry"))
                    .changed();
            }
            shape_seed_row(ui, shape_seed, changed);
        }
        BubbleShape::Strokes {
            half_w,
            half_h,
            corner_px,
            shape_seed,
        } => {
            if !auto {
                *changed |= ui
                    .add(egui::Slider::new(half_w, 20.0..=800.0).text("半幅"))
                    .changed();
                *changed |= ui
                    .add(egui::Slider::new(half_h, 20.0..=800.0).text("半高"))
                    .changed();
            }
            *changed |= ui
                .add(egui::Slider::new(corner_px, 0.0..=200.0).text("角丸"))
                .changed();
            shape_seed_row(ui, shape_seed, changed);
        }
        BubbleShape::DoubleStroke {
            half_w,
            half_h,
            corner_px,
            gap_px,
        } => {
            if !auto {
                *changed |= ui
                    .add(egui::Slider::new(half_w, 20.0..=800.0).text("半幅"))
                    .changed();
                *changed |= ui
                    .add(egui::Slider::new(half_h, 20.0..=800.0).text("半高"))
                    .changed();
            }
            *changed |= ui
                .add(egui::Slider::new(corner_px, 0.0..=200.0).text("角丸"))
                .changed();
            *changed |= ui
                .add(egui::Slider::new(gap_px, 2.0..=40.0).text("線の間隔"))
                .changed();
        }
    }
    if auto {
        ui.label(
            egui::RichText::new("サイズは文字量に合わせて自動調整 (オフで手動)")
                .small()
                .weak(),
        );
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
    // 内側余白。(自動サイズ・結合トグルは常時表示エリアへ移動した。)
    if ui
        .add(egui::Slider::new(&mut b.padding_px, 0.0..=120.0).text("余白"))
        .changed()
    {
        *changed = true;
    }
}

/// 吹き出し「しっぽ」タブ (詳細): 種別 + 幅。表示 on/off は常時表示の構造トグル
/// (`bubble_struct_toggles_ui`) へ移動した。このタブはしっぽが存在するときだけ有効化される
/// ので通常 `b.tail` は Some だが、防御的に None ヒントも出す (ラボ `tab_tail` 相当)。
fn bubble_tail_ui(ui: &mut egui::Ui, b: &mut BubbleObject, changed: &mut bool) {
    let Some(t) = &mut b.tail else {
        ui.label(
            egui::RichText::new("「しっぽを表示」で会話/思考のしっぽを付けられます")
                .small()
                .color(egui::Color32::from_gray(160)),
        );
        return;
    };
    // 形式 (三角=会話 / 思考(丸))。
    ui.horizontal(|ui| {
        ui.label("形式");
        if ui
            .radio(matches!(t.kind, TailKind::Spike), "三角")
            .clicked()
        {
            t.kind = TailKind::Spike;
            *changed = true;
        }
        if ui
            .radio(matches!(t.kind, TailKind::Thought), "思考(丸)")
            .clicked()
        {
            t.kind = TailKind::Thought;
            *changed = true;
        }
    });
    // 先端 (tip) を画像座標で直接編集 (キャンバスのハンドルドラッグと等価)。
    ui.horizontal(|ui| {
        ui.label("先端");
        *changed |= ui
            .add(egui::DragValue::new(&mut t.tip.0).speed(1.0))
            .changed();
        *changed |= ui
            .add(egui::DragValue::new(&mut t.tip.1).speed(1.0))
            .changed();
    });
    // 付け根の自動配置: 中心→先端のレイが輪郭を抜ける位置に根を付ける (対象を指す)。
    // オフにすると `base_t` で手動指定 (付け根ハンドルのドラッグでも切れる)。先端は固定。
    *changed |= ui
        .checkbox(&mut t.base_auto, "付け根を自動 (対象方向)")
        .changed();
    if !t.base_auto {
        *changed |= ui
            .add(egui::Slider::new(&mut t.base_t, 0.0..=1.0).text("付け根位置"))
            .changed();
    }
    let w_label = if matches!(t.kind, TailKind::Thought) {
        "円の大きさ"
    } else {
        "付け根の太さ"
    };
    *changed |= ui
        .add(egui::Slider::new(&mut t.width_px, 4.0..=200.0).text(w_label))
        .changed();
}

/// ウィンドウの位置プリセット (`position`) + サイズモード (`size_mode`) を、ソース画像
/// 寸法 `(iw, ih)` に対して解決し、pivot (中心) と FullWidth の `half_w` を確定させる。
/// 作成時 / 位置・サイズ・余白の変更時に呼ぶ。`Free` 配置・非ウィンドウ・対象 id 不在は
/// no-op (ラボ `apply_window_placement` 相当)。`fonts` は AutoFitText の高さ算出に使う
/// (None のときは `half_h` で代替)。
fn resolve_window_placement(
    objs: &mut [AnnotationObject],
    id: u64,
    iw: f32,
    ih: f32,
    fonts: Option<&FontSet>,
) {
    let Some(idx) = objs.iter().position(|o| o.id == id) else {
        return;
    };
    let (pos, size_mode, margin) = match &objs[idx].kind {
        AnnotationKind::MessageWindow(w) => (w.position, w.size_mode, w.margin_px),
        _ => return,
    };
    // Free 配置は完全に手動 — ドラッグで置いた位置/幅をそのまま保つ。
    if matches!(pos, WindowPosition::Free) {
        return;
    }
    // 有効半高 (AutoFitText は本文から導出するので fonts が要る)。
    let hh = match &objs[idx].kind {
        AnnotationKind::MessageWindow(w) => match fonts {
            Some(f) => comic_core::effective_window_half_extents(w, f).1,
            None => w.half_h.max(1.0),
        },
        _ => return,
    };
    let (mut px, mut py) = objs[idx].pivot;
    if matches!(size_mode, SizeMode::FullWidth) {
        let new_hw = (iw * 0.5 - margin).max(40.0);
        px = iw * 0.5;
        if let AnnotationKind::MessageWindow(w) = &mut objs[idx].kind {
            w.half_w = new_hw;
        }
    }
    match pos {
        WindowPosition::Top => py = margin + hh,
        WindowPosition::Middle | WindowPosition::Center => py = ih * 0.5,
        WindowPosition::Bottom => py = ih - margin - hh,
        WindowPosition::Free => {}
    }
    objs[idx].pivot = (px, py);
}

/// メッセージウィンドウ「枠」タブ。位置 / サイズ / 角丸 / 背景 (塗り・グラデ・スクリム) /
/// 枠 (二重線・間隔) / 影 / テキスト配置 (縦位置・折り返し) / 余白。本文は セリフ タブ、
/// 名前/立ち絵/指標は 部品 タブへ分離 (ラボ `tab_window_body` 相当)。位置/サイズ/余白の
/// 変更後は呼び出し側 (`draw_text_panel`) が `resolve_window_placement` で pivot を再解決する。
fn window_body_ui(ui: &mut egui::Ui, w: &mut MessageWindowObject, changed: &mut bool) {
    // 位置プリセット。
    ui.horizontal_wrapped(|ui| {
        ui.label("位置");
        for (lbl, p) in [
            ("上", WindowPosition::Top),
            ("中", WindowPosition::Middle),
            ("下", WindowPosition::Bottom),
            ("中央", WindowPosition::Center),
            ("自由", WindowPosition::Free),
        ] {
            if ui.radio(w.position == p, lbl).clicked() {
                w.position = p;
                *changed = true;
            }
        }
    });
    // サイズモード。
    ui.horizontal_wrapped(|ui| {
        ui.label("サイズ");
        for (lbl, m) in [
            ("全幅", SizeMode::FullWidth),
            ("固定", SizeMode::Inset),
            ("文字に合わせ", SizeMode::AutoFitText),
        ] {
            if ui.radio(w.size_mode == m, lbl).clicked() {
                w.size_mode = m;
                *changed = true;
            }
        }
    });
    match w.size_mode {
        SizeMode::Inset => {
            *changed |= ui
                .add(egui::Slider::new(&mut w.half_w, 40.0..=1200.0).text("半幅"))
                .changed();
            *changed |= ui
                .add(egui::Slider::new(&mut w.half_h, 24.0..=800.0).text("半高"))
                .changed();
        }
        SizeMode::FullWidth => {
            *changed |= ui
                .add(egui::Slider::new(&mut w.half_h, 24.0..=800.0).text("半高"))
                .changed();
            *changed |= ui
                .add(egui::Slider::new(&mut w.margin_px, 0.0..=300.0).text("左右余白"))
                .changed();
        }
        SizeMode::AutoFitText => {
            ui.label(
                egui::RichText::new("文字量に合わせて自動サイズ")
                    .small()
                    .weak(),
            );
        }
    }
    *changed |= ui
        .add(egui::Slider::new(&mut w.corner_px, 0.0..=80.0).text("角丸"))
        .changed();

    // 背景 (塗り)。
    ui.add_space(2.0);
    ui.label(egui::RichText::new("背景").strong());
    ui.horizontal_wrapped(|ui| {
        ui.label("種類");
        for (lbl, m) in [
            ("なし", FillMode::None),
            ("単色", FillMode::Solid),
            ("半透明", FillMode::Translucent),
            ("スクリム", FillMode::GradientScrim),
            ("グラデ", FillMode::LinearGradient),
        ] {
            if ui.radio(w.fill_mode == m, lbl).clicked() {
                w.fill_mode = m;
                *changed = true;
            }
        }
    });
    if w.fill_mode != FillMode::None {
        if w.fill.is_none() {
            w.fill = Some(Rgba::new(20, 24, 48, 235));
        }
        if let Some(c) = &mut w.fill {
            ui.horizontal(|ui| {
                ui.label("色");
                let mut col = to_c32(*c);
                if ui.color_edit_button_srgba(&mut col).changed() {
                    *c = from_c32(col);
                    *changed = true;
                }
            });
        }
        *changed |= ui
            .add(egui::Slider::new(&mut w.fill_opacity, 0.0..=1.0).text("不透明度"))
            .changed();
        if w.fill_mode == FillMode::GradientScrim {
            ui.horizontal_wrapped(|ui| {
                ui.label("濃い側");
                for (lbl, a) in [
                    ("上", VAnchor::Top),
                    ("中", VAnchor::Center),
                    ("下", VAnchor::Bottom),
                ] {
                    if ui.radio(w.scrim_dense_side == a, lbl).clicked() {
                        w.scrim_dense_side = a;
                        *changed = true;
                    }
                }
            });
        }
        if w.fill_mode == FillMode::LinearGradient {
            if w.gradient_to.is_none() {
                w.gradient_to = Some(Rgba::new(8, 12, 40, 255));
            }
            if let Some(c) = &mut w.gradient_to {
                ui.horizontal(|ui| {
                    ui.label("下端色");
                    let mut col = to_c32(*c);
                    if ui.color_edit_button_srgba(&mut col).changed() {
                        *c = from_c32(col);
                        *changed = true;
                    }
                });
            }
        }
    }

    // 枠 (フレーム)。
    ui.add_space(2.0);
    ui.label(egui::RichText::new("枠").strong());
    ui.horizontal_wrapped(|ui| {
        ui.label("種類");
        for (lbl, f) in [
            ("なし", FrameStyle::None),
            ("単線", FrameStyle::SolidRounded),
            ("二重線", FrameStyle::DoubleLine),
        ] {
            if ui.radio(w.frame == f, lbl).clicked() {
                w.frame = f;
                *changed = true;
            }
        }
    });
    if w.frame != FrameStyle::None {
        ui.horizontal(|ui| {
            ui.label("枠色");
            let mut col = to_c32(w.outline.color);
            if ui.color_edit_button_srgba(&mut col).changed() {
                w.outline.color = from_c32(col);
                *changed = true;
            }
        });
        *changed |= ui
            .add(egui::Slider::new(&mut w.outline.width_px, 0.0..=12.0).text("枠太さ"))
            .changed();
        if w.frame == FrameStyle::DoubleLine {
            *changed |= ui
                .add(egui::Slider::new(&mut w.frame_gap_px, 2.0..=24.0).text("二重間隔"))
                .changed();
        }
    }

    // 影 (ドロップシャドウ)。
    ui.add_space(2.0);
    let mut has_shadow = w.shadow.is_some();
    if ui.checkbox(&mut has_shadow, "影").changed() {
        w.shadow = if has_shadow {
            Some(ShadowStyle::default())
        } else {
            None
        };
        *changed = true;
    }
    if let Some(sh) = &mut w.shadow {
        ui.horizontal(|ui| {
            ui.label("影色");
            let mut col = to_c32(sh.color);
            if ui.color_edit_button_srgba(&mut col).changed() {
                sh.color = from_c32(col);
                *changed = true;
            }
            ui.label("X");
            *changed |= ui
                .add(egui::DragValue::new(&mut sh.offset.0).speed(0.5))
                .changed();
            ui.label("Y");
            *changed |= ui
                .add(egui::DragValue::new(&mut sh.offset.1).speed(0.5))
                .changed();
        });
    }

    // テキスト配置 (縦位置 + 折り返し)。
    ui.add_space(2.0);
    ui.label(egui::RichText::new("テキスト配置").strong());
    ui.horizontal(|ui| {
        ui.label("縦位置");
        for (lbl, a) in [
            ("上", VAnchor::Top),
            ("中", VAnchor::Center),
            ("下", VAnchor::Bottom),
        ] {
            if ui.radio(w.v_anchor == a, lbl).clicked() {
                w.v_anchor = a;
                *changed = true;
            }
        }
    });
    *changed |= ui
        .checkbox(&mut w.wrap, "本文を折り返す (禁則処理)")
        .changed();

    // 余白 (per-side insets)。
    ui.add_space(2.0);
    ui.label("余白 (左/上/右/下)");
    ui.horizontal(|ui| {
        *changed |= ui
            .add(egui::DragValue::new(&mut w.padding.left).speed(1.0))
            .changed();
        *changed |= ui
            .add(egui::DragValue::new(&mut w.padding.top).speed(1.0))
            .changed();
        *changed |= ui
            .add(egui::DragValue::new(&mut w.padding.right).speed(1.0))
            .changed();
        *changed |= ui
            .add(egui::DragValue::new(&mut w.padding.bottom).speed(1.0))
            .changed();
    });
}

/// メッセージウィンドウ「部品」タブ。名前プレート (モード・名前・色 + 装飾) / 立ち絵枠
/// プレースホルダ / 続き指標。ラボ `draw_window_name_header` + `tab_window_parts` をまとめた。
fn window_parts_ui(ui: &mut egui::Ui, w: &mut MessageWindowObject, changed: &mut bool) {
    // ── 名前プレート (詳細スタイル。表示モード/名前色/話者名は常時表示の名前ヘッダ側) ──
    ui.label(egui::RichText::new("名前プレート").strong());
    if w.name_plate.mode == NamePlateMode::None {
        ui.label(
            egui::RichText::new("上部の「名前」で表示モードを選ぶと詳細スタイルが出ます")
                .small()
                .color(egui::Color32::from_gray(160)),
        );
    }
    if w.name_plate.mode != NamePlateMode::None {
        *changed |= ui
            .add(egui::Slider::new(&mut w.name_plate.name.size_px, 8.0..=120.0).text("文字サイズ"))
            .changed();
        if matches!(
            w.name_plate.mode,
            NamePlateMode::Boxed | NamePlateMode::Above
        ) {
            let mut has_fill = w.name_plate.fill.is_some();
            if ui.checkbox(&mut has_fill, "プレート塗り").changed() {
                w.name_plate.fill = if has_fill {
                    Some(Rgba::new(30, 32, 44, 255))
                } else {
                    None
                };
                *changed = true;
            }
            if let Some(c) = &mut w.name_plate.fill {
                ui.horizontal(|ui| {
                    ui.label("塗り色");
                    let mut col = to_c32(*c);
                    if ui.color_edit_button_srgba(&mut col).changed() {
                        *c = from_c32(col);
                        *changed = true;
                    }
                });
            }
            ui.horizontal(|ui| {
                ui.label("枠色");
                let mut col = to_c32(w.name_plate.outline.color);
                if ui.color_edit_button_srgba(&mut col).changed() {
                    w.name_plate.outline.color = from_c32(col);
                    *changed = true;
                }
            });
            *changed |= ui
                .add(
                    egui::Slider::new(&mut w.name_plate.outline.width_px, 0.0..=10.0)
                        .text("枠太さ"),
                )
                .changed();
            *changed |= ui
                .add(egui::Slider::new(&mut w.name_plate.corner_px, 0.0..=40.0).text("角丸"))
                .changed();
            *changed |= ui
                .add(egui::Slider::new(&mut w.name_plate.padding_px, 0.0..=40.0).text("余白"))
                .changed();
        }
        ui.horizontal(|ui| {
            ui.label("位置 X/Y");
            *changed |= ui
                .add(egui::DragValue::new(&mut w.name_plate.offset.0).speed(1.0))
                .changed();
            *changed |= ui
                .add(egui::DragValue::new(&mut w.name_plate.offset.1).speed(1.0))
                .changed();
        });
    }

    // ── 立ち絵枠 (プレースホルダ) ──
    ui.add_space(2.0);
    ui.label(egui::RichText::new("立ち絵枠 (プレースホルダ)").strong());
    ui.horizontal(|ui| {
        ui.label("配置");
        for (lbl, s) in [
            ("なし", PortraitSide::None),
            ("左", PortraitSide::Left),
            ("右", PortraitSide::Right),
        ] {
            if ui.radio(w.portrait.side == s, lbl).clicked() {
                w.portrait.side = s;
                *changed = true;
            }
        }
    });
    if w.portrait.side != PortraitSide::None {
        *changed |= ui
            .add(egui::Slider::new(&mut w.portrait.width_px, 40.0..=600.0).text("幅"))
            .changed();
        if w.portrait.fill.is_none() {
            w.portrait.fill = Some(Rgba::new(70, 74, 92, 255));
        }
        if let Some(c) = &mut w.portrait.fill {
            ui.horizontal(|ui| {
                ui.label("色");
                let mut col = to_c32(*c);
                if ui.color_edit_button_srgba(&mut col).changed() {
                    *c = from_c32(col);
                    *changed = true;
                }
            });
        }
        *changed |= ui
            .add(egui::Slider::new(&mut w.portrait.margin_px, 0.0..=60.0).text("余白"))
            .changed();
    }

    // ── 続き指標 ──
    ui.add_space(2.0);
    ui.label(egui::RichText::new("続き指標").strong());
    ui.horizontal_wrapped(|ui| {
        for (lbl, k) in [
            ("なし", IndicatorKind::None),
            ("三角", IndicatorKind::Triangle),
            ("山", IndicatorKind::Chevron),
            ("菱", IndicatorKind::Diamond),
            ("点々", IndicatorKind::Dots),
        ] {
            if ui.radio(w.indicator == k, lbl).clicked() {
                w.indicator = k;
                *changed = true;
            }
        }
    });
    if w.indicator != IndicatorKind::None {
        // ゲーム風「まだ続きがある」挙動: 本文が枠から溢れた時だけ指標を出す。
        *changed |= ui
            .checkbox(&mut w.indicator_auto, "テキストが溢れた時だけ表示")
            .changed();
    }
}

fn name_plate_mode_label(m: NamePlateMode) -> &'static str {
    match m {
        NamePlateMode::None => "なし",
        NamePlateMode::Inline => "ラベル",
        NamePlateMode::Boxed => "枠付き",
        NamePlateMode::Above => "上に",
    }
}

/// 吹き出し「飾り」タブ。きらきら / 花 / 泡 の手続き的装飾レイヤーを追加・編集する
/// (ラボ `tab_deco` 相当)。装飾は形状プリセットに含まれないので link は切らない。
fn bubble_deco_ui(ui: &mut egui::Ui, b: &mut BubbleObject, changed: &mut bool) {
    if ui.button("装飾を追加").clicked() {
        let mut layer = DecorationLayer::default();
        // レイヤーごとに異なる seed を割り当てる。`place_decorations` は seed 決定的なので、
        // 同一 seed のレイヤーを重ねると全く同じ位置に乗り「1 つしか描かれていない」ように
        // 見える。max(既存)+1 で衝突を避ける。
        layer.seed = b
            .decorations
            .iter()
            .map(|l| l.seed)
            .max()
            .map(|m| m.wrapping_add(1))
            .unwrap_or(0);
        b.decorations.push(layer);
        *changed = true;
    }
    if b.decorations.is_empty() {
        ui.label(
            egui::RichText::new("「装飾を追加」できらきら/花/泡を縁取りに配置できます")
                .small()
                .color(egui::Color32::from_gray(160)),
        );
        return;
    }
    let mut remove_deco: Option<usize> = None;
    for (di, layer) in b.decorations.iter_mut().enumerate() {
        ui.separator();
        ui.horizontal(|ui| {
            ui.label(format!("装飾 {}", di + 1));
            if ui.small_button("×").clicked() {
                remove_deco = Some(di);
            }
        });
        ui.horizontal(|ui| {
            ui.label("種類");
            for (label, kind) in [
                ("きらきら", DecoKind::Sparkle),
                ("花", DecoKind::Flower),
                ("泡", DecoKind::Bubble),
            ] {
                if ui.radio(layer.kind == kind, label).clicked() {
                    layer.kind = kind;
                    *changed = true;
                }
            }
        });
        ui.horizontal_wrapped(|ui| {
            ui.label("配置");
            for (label, pl) in [
                ("輪郭上", DecoPlacement::Outline),
                ("外側", DecoPlacement::Outside),
                ("内側", DecoPlacement::Inside),
                ("しっぽ", DecoPlacement::Tail),
            ] {
                if ui.radio(layer.placement == pl, label).clicked() {
                    layer.placement = pl;
                    *changed = true;
                }
            }
        });
        *changed |= ui
            .add(egui::Slider::new(&mut layer.density, 0.5..=12.0).text("密度"))
            .changed();
        *changed |= ui
            .add(egui::Slider::new(&mut layer.size_ratio, 0.04..=0.6).text("大きさ"))
            .changed();
        ui.horizontal(|ui| {
            ui.label("色");
            let mut c = to_c32(layer.color);
            if ui.color_edit_button_srgba(&mut c).changed() {
                layer.color = from_c32(c);
                *changed = true;
            }
        });

        // 縁取り (0px = なし) + 縁取り色。
        *changed |= ui
            .add(egui::Slider::new(&mut layer.outline_width, 0.0..=10.0).text("縁取り太さ"))
            .changed();
        if layer.outline_width > 0.0 {
            ui.horizontal(|ui| {
                ui.label("縁取り色");
                let mut c = to_c32(layer.outline_color);
                if ui.color_edit_button_srgba(&mut c).changed() {
                    layer.outline_color = from_c32(c);
                    *changed = true;
                }
            });
        }

        // 種別固有の形状コントロール。
        match layer.kind {
            DecoKind::Sparkle => {
                *changed |= ui
                    .add(egui::Slider::new(&mut layer.points, 3..=12).text("とがり数"))
                    .changed();
            }
            DecoKind::Flower => {
                *changed |= ui
                    .add(egui::Slider::new(&mut layer.petals, 3..=10).text("花びら数"))
                    .changed();
                ui.horizontal(|ui| {
                    ui.label("中央色");
                    let mut c = to_c32(layer.center_color);
                    if ui.color_edit_button_srgba(&mut c).changed() {
                        layer.center_color = from_c32(c);
                        *changed = true;
                    }
                });
            }
            DecoKind::Bubble => {
                *changed |= ui
                    .checkbox(&mut layer.gradient, "半透明グラデ (泡)")
                    .changed();
            }
        }

        ui.horizontal(|ui| {
            ui.label(format!("seed {}", layer.seed));
            if ui.button("再生成").clicked() {
                layer.seed = layer.seed.wrapping_add(1);
                *changed = true;
            }
        });
    }
    if let Some(di) = remove_deco {
        b.decorations.remove(di);
        *changed = true;
    }
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

    // ── Undo / Redo の純ロジック (Inc 6) ──────────────────────────────

    /// 1 オブジェクトの状態を作るヘルパー (id だけで区別できれば十分)。
    fn state(ids: &[u64]) -> Vec<AnnotationObject> {
        ids.iter()
            .map(|&id| stamp_obj(id, (0.0, 0.0), 10.0, 10.0))
            .collect()
    }

    #[test]
    fn commit_coalesces_only_on_change() {
        let mut undo = Vec::new();
        let mut redo = Vec::new();
        let mut base = state(&[1]);
        // 変化なし → push されない。
        comic_commit_pending(&mut undo, &mut redo, &mut base, &state(&[1]));
        assert!(undo.is_empty());
        // 変化あり → 旧ベースラインを 1 つだけ push、ベースライン更新。
        comic_commit_pending(&mut undo, &mut redo, &mut base, &state(&[1, 2]));
        assert_eq!(undo.len(), 1);
        assert_eq!(undo[0], state(&[1]));
        assert_eq!(base, state(&[1, 2]));
    }

    #[test]
    fn undo_then_redo_round_trips() {
        let mut undo = Vec::new();
        let mut redo = Vec::new();
        let mut base = state(&[1]);
        // [1] → [1,2] を commit。
        comic_commit_pending(&mut undo, &mut redo, &mut base, &state(&[1, 2]));
        // undo: 作業状態 [1,2] を渡すと [1] が返り、redo に [1,2] が積まれる。
        let after_undo = comic_undo_step(&mut undo, &mut redo, &mut base, &state(&[1, 2]));
        assert_eq!(after_undo, Some(state(&[1])));
        assert_eq!(base, state(&[1]));
        assert_eq!(redo.len(), 1);
        // redo: [1] を渡すと [1,2] が返り、元に戻る。
        let after_redo = comic_redo_step(&mut undo, &mut redo, &mut base, &state(&[1]));
        assert_eq!(after_redo, Some(state(&[1, 2])));
        assert_eq!(base, state(&[1, 2]));
        assert!(redo.is_empty());
    }

    #[test]
    fn undo_commits_pending_edit_first() {
        // ラボ do_undo と同じ: 未コミットの編集があるとき undo はまずそれを commit してから戻す。
        let mut undo = Vec::new();
        let mut redo = Vec::new();
        let mut base = state(&[1]); // ベースライン = [1]、作業状態 = [1,2] (未コミット)
        let after_undo = comic_undo_step(&mut undo, &mut redo, &mut base, &state(&[1, 2]));
        // 未コミット [1,2] が commit されてから戻るので、戻り先は [1]。
        assert_eq!(after_undo, Some(state(&[1])));
        // redo には未コミットだった [1,2] が積まれている。
        assert_eq!(redo, vec![state(&[1, 2])]);
    }

    #[test]
    fn new_edit_clears_redo() {
        let mut undo = Vec::new();
        let mut redo = Vec::new();
        let mut base = state(&[1]);
        comic_commit_pending(&mut undo, &mut redo, &mut base, &state(&[1, 2]));
        comic_undo_step(&mut undo, &mut redo, &mut base, &state(&[1, 2]));
        assert_eq!(redo.len(), 1);
        // undo 後に別の編集を commit すると redo は捨てられる (分岐した履歴)。
        comic_commit_pending(&mut undo, &mut redo, &mut base, &state(&[1, 3]));
        assert!(redo.is_empty());
        assert_eq!(base, state(&[1, 3]));
    }

    #[test]
    fn empty_stacks_are_noops() {
        let mut undo: Vec<Vec<AnnotationObject>> = Vec::new();
        let mut redo: Vec<Vec<AnnotationObject>> = Vec::new();
        let mut base = state(&[1]);
        // 何も積まれていなければ undo/redo は None で状態を変えない。
        assert_eq!(
            comic_undo_step(&mut undo, &mut redo, &mut base, &state(&[1])),
            None
        );
        assert_eq!(
            comic_redo_step(&mut undo, &mut redo, &mut base, &state(&[1])),
            None
        );
        assert_eq!(base, state(&[1]));
    }

    #[test]
    fn undo_stack_respects_cap() {
        let mut undo = Vec::new();
        let mut redo = Vec::new();
        let mut base = state(&[0]);
        // CAP + 5 回 commit して、最古が捨てられ深さが CAP で頭打ちになることを確認。
        for i in 1..=(COMIC_UNDO_CAP as u64 + 5) {
            comic_commit_pending(&mut undo, &mut redo, &mut base, &state(&[i]));
        }
        assert_eq!(undo.len(), COMIC_UNDO_CAP);
        // 先頭 (最古) は remove(0) されているので state(&[0]) ではない。
        assert_ne!(undo[0], state(&[0]));
    }

    // ── ウィンドウ配置解決 (Inc 4d) ──────────────────────────────────

    /// 指定 position / size_mode の窓を 1 個だけ持つ objs を作る。pivot は (0,0) 始点。
    fn window_objs(pos: WindowPosition, size_mode: SizeMode) -> Vec<AnnotationObject> {
        let w = MessageWindowObject {
            position: pos,
            size_mode,
            half_w: 100.0,
            half_h: 60.0,
            margin_px: 48.0,
            ..MessageWindowObject::default()
        };
        vec![AnnotationObject::new_message_window(1, (0.0, 0.0), w)]
    }

    #[test]
    fn placement_free_is_noop() {
        let mut objs = window_objs(WindowPosition::Free, SizeMode::Inset);
        objs[0].pivot = (123.0, 456.0);
        resolve_window_placement(&mut objs, 1, 2000.0, 1000.0, None);
        assert_eq!(objs[0].pivot, (123.0, 456.0), "Free はドラッグ位置を保持");
    }

    #[test]
    fn placement_top_and_bottom_use_margin_and_half_height() {
        // Inset (half_h=60, margin=48)。Top: py = margin + hh = 108。Bottom: ih - margin - hh。
        let mut objs = window_objs(WindowPosition::Top, SizeMode::Inset);
        resolve_window_placement(&mut objs, 1, 2000.0, 1000.0, None);
        assert!((objs[0].pivot.1 - (48.0 + 60.0)).abs() < 1e-3);

        let mut objs = window_objs(WindowPosition::Bottom, SizeMode::Inset);
        resolve_window_placement(&mut objs, 1, 2000.0, 1000.0, None);
        assert!((objs[0].pivot.1 - (1000.0 - 48.0 - 60.0)).abs() < 1e-3);
    }

    #[test]
    fn placement_center_and_middle_use_image_midline() {
        for pos in [WindowPosition::Center, WindowPosition::Middle] {
            let mut objs = window_objs(pos, SizeMode::Inset);
            resolve_window_placement(&mut objs, 1, 2000.0, 1000.0, None);
            assert!((objs[0].pivot.1 - 500.0).abs() < 1e-3, "{pos:?}");
        }
    }

    #[test]
    fn placement_fullwidth_recomputes_half_w_and_centers_x() {
        // FullWidth: half_w = iw/2 - margin、px = iw/2。
        let mut objs = window_objs(WindowPosition::Bottom, SizeMode::FullWidth);
        resolve_window_placement(&mut objs, 1, 2000.0, 1000.0, None);
        assert!((objs[0].pivot.0 - 1000.0).abs() < 1e-3, "px = iw/2");
        if let AnnotationKind::MessageWindow(w) = &objs[0].kind {
            assert!(
                (w.half_w - (1000.0 - 48.0)).abs() < 1e-3,
                "half_w = iw/2 - margin"
            );
        } else {
            panic!("window");
        }
    }

    // ── 記法の記号挿入 (Inc 4a 仕上げ) ────────────────────────────────

    #[test]
    fn insert_markers_wraps_selection() {
        // "あいうえお" の char[1..4)=「いうえ」を [ ] で囲む。
        let mut s = "あいうえお".to_string();
        let caret = insert_markers(&mut s, 1, 4, '[', ']');
        assert_eq!(s, "あ[いうえ]お");
        // caret = 囲んだ内容の直後・閉じ記号の手前 = start(1) + 1 + (end-start)(3) = 5。
        assert_eq!(caret, 5);
        assert_eq!(s.chars().nth(caret - 1), Some('え')); // 直前 = 内容末尾
        assert_eq!(s.chars().nth(caret), Some(']')); // 直後 = 閉じ記号
    }

    #[test]
    fn insert_markers_empty_selection_inserts_pair_at_caret() {
        let mut s = "ab".to_string();
        let caret = insert_markers(&mut s, 2, 2, '〈', '〉');
        assert_eq!(s, "ab〈〉");
        assert_eq!(caret, 3); // open と close の間
    }

    #[test]
    fn insert_markers_clamps_out_of_range() {
        let mut s = "x".to_string();
        // start/end が範囲外でも panic せず末尾にクランプ。
        let caret = insert_markers(&mut s, 99, 99, '{', '}');
        assert_eq!(s, "x{}");
        assert_eq!(caret, 2);
    }

    #[test]
    fn marker_pairs_eq_matches_same_set_ignores_dir() {
        let brackets = comic_core::markup_rules_brackets();
        let angle = comic_core::markup_rules_angle();
        assert!(marker_pairs_eq(
            &brackets,
            &comic_core::markup_rules_brackets()
        ));
        assert!(!marker_pairs_eq(&brackets, &angle));
        // 長さ違いは不一致。
        assert!(!marker_pairs_eq(&brackets, &brackets[..1]));
    }
}
