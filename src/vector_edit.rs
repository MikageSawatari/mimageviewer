//! ベクタオブジェクト編集の共通基盤。
//!
//! 消しゴム (Erase) / 隠蔽加工 (Conceal) の Select ツールで共有される、
//! [`crate::mask_db::Shape`] のハンドル編集 (本体移動 / 端点 / 角 / 辺中点 /
//! 回転ハンドル) を担う。詳細仕様は
//! [docs/conceal-feature-plan.md §5](../../docs/conceal-feature-plan.md) と
//! [docs/conceal-impl-plan-codex-brief.md §5](../../docs/conceal-impl-plan-codex-brief.md)。
//!
//! # 設計の要点
//!
//! - **画像座標で計算する**: `compute_handle_layout` / `hit_test` / `apply_drag`
//!   は **画像ピクセル空間**で動く。スクリーン座標との変換は呼び出し側 (UI
//!   レイヤ) で行う
//! - **画面 px 一定のハンドル**: 回転ハンドルの bbox からの距離、ヒット半径、
//!   ハンドルの描画サイズはすべて「スクリーン px → 画像 px」に `scale` を
//!   割って算出する (ズーム時もハンドル操作性を保つ)
//! - **Shift / Alt 修飾子**: `apply_drag` が `egui::Modifiers` を取り、
//!   Shift で軸拘束 / 等比 / 角度スナップ、Alt で中心固定リサイズに分岐
//! - **状態は呼び出し側に持たせる**: `DragState` enum は App 側で保持。
//!   `apply_drag` は state を見て base から増分計算した新 Shape を返すだけの
//!   純関数 (= テスト容易)
//!
//! # 使用パターン
//!
//! ```ignore
//! // ホバー判定
//! let layout = vector_edit::compute_handle_layout(&shape, scale);
//! let hover = vector_edit::hit_test(&layout, cursor_img, scale);
//! ctx.set_cursor_icon(vector_edit::cursor_icon_for(hover, &shape));
//!
//! // クリック開始でドラッグ状態を作る
//! if primary_pressed {
//!     if let Some(target) = hover {
//!         let drag = vector_edit::begin_drag(target, idx, shape, cursor_img);
//!         self.conceal_drag = Some(drag);
//!     }
//! }
//!
//! // ドラッグ中の更新
//! if let Some(drag) = &self.conceal_drag {
//!     let new_shape = vector_edit::apply_drag(drag, cursor_img, &modifiers);
//!     self.conceal_shapes[drag.idx()] = new_shape;
//! }
//! ```

use eframe::egui;

use crate::mask_db::{Shape, rect_corners};

// ── 定数 ────────────────────────────────────────────────────────────────

/// ハンドルのヒット半径 (スクリーン px)。画像 px に変換するには `/ scale` する。
pub const HANDLE_HIT_RADIUS_PX: f32 = 12.0;

/// 回転ハンドルの bbox 上辺中点からの距離 (スクリーン px)。
pub const ROTATE_HANDLE_OFFSET_PX: f32 = 28.0;

/// 描画時のハンドル円の半径 (スクリーン px)。
pub const HANDLE_DRAW_RADIUS_PX: f32 = 5.0;

/// 回転ハンドルの円の半径 (スクリーン px、視覚的に他と区別)。
pub const ROTATE_HANDLE_DRAW_RADIUS_PX: f32 = 6.0;

/// Shift+回転のスナップ角度 (15°)。
pub const ROTATE_SNAP_DEG: f32 = 15.0;

/// Shift+端点 (Line) のスナップ角度集合 [°] (`0/45/90`)。
const LINE_SNAP_ANGLES_DEG: &[f32] = &[0.0, 45.0, 90.0, 135.0, 180.0, -45.0, -90.0, -135.0];

// ── HoverTarget / DragState / HandleLayout ──────────────────────────────

/// マウスホバー時にどのハンドルに乗っているかの論理識別子。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HoverTarget {
    /// 本体内部 (移動用)。
    Body,
    /// Line の端点 (`which_p1=true` で p1、`false` で p0)。
    Endpoint { which_p1: bool },
    /// Rect / Ellipse の bbox 4 角 (0=NW, 1=NE, 2=SE, 3=SW、回転 0 のとき)。
    Corner { idx: u8 },
    /// Rect / Ellipse の bbox 4 辺中点 (0=N, 1=E, 2=S, 3=W、回転 0 のとき)。
    EdgeMidpoint { idx: u8 },
    /// Rect / Ellipse の専用回転ハンドル (bbox 上辺中点から伸びる棒の先)。
    RotateHandle,
}

/// ドラッグ操作の状態。`apply_drag` で base からの増分を毎フレーム計算する。
#[derive(Debug, Clone, Copy)]
pub enum DragState {
    /// 本体ドラッグ: 平行移動。
    Pan {
        idx: usize,
        base: Shape,
        origin: (f32, f32),
    },
    /// Line の端点ドラッグ。
    Endpoint {
        idx: usize,
        base: Shape,
        which_p1: bool,
        origin: (f32, f32),
    },
    /// Rect / Ellipse の角 / 辺中点ハンドルドラッグ (リサイズ)。
    ///
    /// `anchor` は通常は反対側の固定点 (角ハンドルなら対角の角)。
    /// Alt+ドラッグでは中心固定になるため、開始時の center を anchor として保持する。
    Resize {
        idx: usize,
        base: Shape,
        target: HoverTarget,
        origin: (f32, f32),
        anchor: (f32, f32),
    },
    /// Rect / Ellipse の回転ハンドルドラッグ。
    ///
    /// `start_angle` はドラッグ開始時刻の `center → origin` 角度 (rad)。
    /// 現在の `center → cur` 角度との差分を base に加算して新 rotation を作る。
    Rotate {
        idx: usize,
        base: Shape,
        center: (f32, f32),
        start_angle: f32,
    },
}

impl DragState {
    pub fn idx(&self) -> usize {
        match self {
            DragState::Pan { idx, .. }
            | DragState::Endpoint { idx, .. }
            | DragState::Resize { idx, .. }
            | DragState::Rotate { idx, .. } => *idx,
        }
    }
}

/// ハンドル位置の集合 (画像ピクセル座標)。
///
/// - 本体ヒットには `body_corners` (4 頂点の多角形) を使う
/// - Line には `endpoints` (始点・終点) のみ
/// - Rect / Ellipse には `corners` (4 角) + `edge_midpoints` (4 辺中点) +
///   `rotate_handle` (回転ハンドル先端)
#[derive(Debug, Clone)]
pub struct HandleLayout {
    /// 本体のヒット領域 (回転考慮済みの 4 隅、NW→NE→SE→SW)。
    pub body_corners: [(f32, f32); 4],
    /// Line のみ: 端点 (p0, p1)。
    pub endpoints: Option<[(f32, f32); 2]>,
    /// Rect / Ellipse のみ: 4 角 (回転後)。
    pub corners: Option<[(f32, f32); 4]>,
    /// Rect / Ellipse のみ: 4 辺中点 (回転後)。
    pub edge_midpoints: Option<[(f32, f32); 4]>,
    /// Rect / Ellipse のみ: 回転ハンドル先端 (bbox 上辺中点から ROTATE_HANDLE_OFFSET_PX
    /// だけ画像座標で離れた点。`scale` で計算済み)。
    pub rotate_handle: Option<(f32, f32)>,
}

// ── compute_handle_layout ────────────────────────────────────────────────

/// shape からハンドル配置 (画像ピクセル空間) を計算する。
///
/// `scale` はスクリーン px / 画像 px の倍率。回転ハンドルの距離だけが scale に
/// 依存する (= ズーム時もスクリーン上で一定距離に見える)。
pub fn compute_handle_layout(shape: &Shape, scale: f32) -> HandleLayout {
    let scale = scale.max(1e-6);
    let rotate_offset_img = ROTATE_HANDLE_OFFSET_PX / scale;
    match shape {
        Shape::Line {
            p0, p1, thickness, ..
        } => {
            // Line の本体は p0 → p1 を中心軸、太さ thickness の矩形帯。
            let body = line_corners(*p0, *p1, *thickness);
            HandleLayout {
                body_corners: body,
                endpoints: Some([*p0, *p1]),
                corners: None,
                edge_midpoints: None,
                rotate_handle: None,
            }
        }
        Shape::Rect {
            center,
            half_w,
            half_h,
            rotation_rad,
        } => {
            let body = rect_corners(*center, *half_w, *half_h, *rotation_rad);
            let edges = rect_edge_midpoints(*center, *half_w, *half_h, *rotation_rad);
            let rotate = rotate_handle_pos(*center, *half_h, *rotation_rad, rotate_offset_img);
            HandleLayout {
                body_corners: body,
                endpoints: None,
                corners: Some(body),
                edge_midpoints: Some(edges),
                rotate_handle: Some(rotate),
            }
        }
        Shape::Ellipse {
            center,
            rx,
            ry,
            rotation_rad,
        } => {
            // Ellipse の bbox 領域 (Rect と同じ算出)。本体ヒットは bbox で判定する。
            let body = rect_corners(*center, *rx, *ry, *rotation_rad);
            let edges = rect_edge_midpoints(*center, *rx, *ry, *rotation_rad);
            let rotate = rotate_handle_pos(*center, *ry, *rotation_rad, rotate_offset_img);
            HandleLayout {
                body_corners: body,
                endpoints: None,
                corners: Some(body),
                edge_midpoints: Some(edges),
                rotate_handle: Some(rotate),
            }
        }
    }
}

/// 中心軸 (p0 → p1) と太さから矩形帯の 4 隅を返す ([`crate::mask_db::LineObject::corners`] と同じ算法)。
fn line_corners(p0: (f32, f32), p1: (f32, f32), thickness: f32) -> [(f32, f32); 4] {
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

/// 回転矩形の 4 辺の中点を返す (回転前の N, E, S, W に対応、出力順は N→E→S→W)。
fn rect_edge_midpoints(
    center: (f32, f32),
    half_w: f32,
    half_h: f32,
    rotation_rad: f32,
) -> [(f32, f32); 4] {
    let (s, c) = rotation_rad.sin_cos();
    let local = [
        (0.0, -half_h), // N (上辺中点、回転 0 で y- 方向)
        (half_w, 0.0),  // E (右辺中点)
        (0.0, half_h),  // S (下辺中点)
        (-half_w, 0.0), // W (左辺中点)
    ];
    let mut out = [(0.0_f32, 0.0_f32); 4];
    for i in 0..4 {
        let (lx, ly) = local[i];
        out[i] = (center.0 + c * lx - s * ly, center.1 + s * lx + c * ly);
    }
    out
}

/// 回転ハンドルの先端位置を返す。bbox 上辺中点から、上辺の外側法線方向に
/// `offset_img` だけ離れた点。
fn rotate_handle_pos(
    center: (f32, f32),
    half_h: f32,
    rotation_rad: f32,
    offset_img: f32,
) -> (f32, f32) {
    let (s, c) = rotation_rad.sin_cos();
    let lx = 0.0;
    let ly = -half_h - offset_img;
    (center.0 + c * lx - s * ly, center.1 + s * lx + c * ly)
}

// ── hit_test ─────────────────────────────────────────────────────────────

/// 画像座標 `pos` がハンドルのどれかにヒットするか調べる。
///
/// `scale` はヒット半径をスクリーン px → 画像 px に換算するため。
/// 判定順: 回転ハンドル → 角ハンドル → 辺中点 → 端点 → 本体。
pub fn hit_test(layout: &HandleLayout, pos: (f32, f32), scale: f32) -> Option<HoverTarget> {
    let scale = scale.max(1e-6);
    let hit_r_img = HANDLE_HIT_RADIUS_PX / scale;
    let hit_r2 = hit_r_img * hit_r_img;

    // 1. 回転ハンドル: 最優先 (bbox 外にあるので競合せず分離)
    if let Some(rh) = layout.rotate_handle {
        if dist_sq(pos, rh) <= hit_r2 {
            return Some(HoverTarget::RotateHandle);
        }
    }
    // 2. 角ハンドル
    if let Some(cs) = layout.corners {
        for (i, &c) in cs.iter().enumerate() {
            if dist_sq(pos, c) <= hit_r2 {
                return Some(HoverTarget::Corner { idx: i as u8 });
            }
        }
    }
    // 3. 辺中点ハンドル
    if let Some(es) = layout.edge_midpoints {
        for (i, &m) in es.iter().enumerate() {
            if dist_sq(pos, m) <= hit_r2 {
                return Some(HoverTarget::EdgeMidpoint { idx: i as u8 });
            }
        }
    }
    // 4. 端点ハンドル (Line)
    if let Some(eps) = layout.endpoints {
        // p0 を which_p1=false, p1 を which_p1=true で返す
        let d0 = dist_sq(pos, eps[0]);
        let d1 = dist_sq(pos, eps[1]);
        if d0 <= hit_r2 && d0 <= d1 {
            return Some(HoverTarget::Endpoint { which_p1: false });
        }
        if d1 <= hit_r2 {
            return Some(HoverTarget::Endpoint { which_p1: true });
        }
    }
    // 5. 本体
    if point_in_polygon(pos, &layout.body_corners) {
        return Some(HoverTarget::Body);
    }
    None
}

fn dist_sq(a: (f32, f32), b: (f32, f32)) -> f32 {
    let dx = a.0 - b.0;
    let dy = a.1 - b.1;
    dx * dx + dy * dy
}

/// 多角形内判定 (奇数交差判定)。Pos2 不使用、`(f32, f32)` で扱う。
fn point_in_polygon(p: (f32, f32), poly: &[(f32, f32)]) -> bool {
    let mut inside = false;
    let n = poly.len();
    if n < 3 {
        return false;
    }
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

// ── cursor_icon_for ──────────────────────────────────────────────────────

/// HoverTarget に応じたカーソル形状を返す。
///
/// - 角ハンドルは shape の `rotation_rad` を見て NW-SE / NE-SW を選ぶ
/// - 辺中点ハンドルは N/S なら垂直、E/W なら水平 (回転 45° 以上なら入れ替え)
/// - 回転ハンドルは PointingHand (↻ 補助アイコンは描画側で別途載せる)
/// - Endpoint / Body は標準カーソル (= Move / Default)
pub fn cursor_icon_for(target: HoverTarget, shape: &Shape) -> egui::CursorIcon {
    match target {
        HoverTarget::Body => egui::CursorIcon::Move,
        HoverTarget::Endpoint { .. } => egui::CursorIcon::Crosshair,
        HoverTarget::RotateHandle => egui::CursorIcon::PointingHand,
        HoverTarget::Corner { idx } => corner_cursor(idx, shape_rotation(shape)),
        HoverTarget::EdgeMidpoint { idx } => edge_cursor(idx, shape_rotation(shape)),
    }
}

fn shape_rotation(shape: &Shape) -> f32 {
    match shape {
        Shape::Line { .. } => 0.0,
        Shape::Rect { rotation_rad, .. } | Shape::Ellipse { rotation_rad, .. } => *rotation_rad,
    }
}

/// 角ハンドルのカーソル。回転 0 では NW/SE が NwSe、NE/SW が NeSw。
/// 回転 45°〜135° で水平/垂直リサイズに切り替わる (回転 90° で NW→NE 位置になる)。
fn corner_cursor(idx: u8, rotation_rad: f32) -> egui::CursorIcon {
    // canonical (回転 0) では NW=NwSe, NE=NeSw, SE=NwSe, SW=NeSw
    let base = if idx == 0 || idx == 2 {
        // NW or SE
        0
    } else {
        // NE or SW
        1
    };
    // 回転を 45° バケットに丸めて 8 方向に正規化 (0..=7)
    let bucket = rotation_bucket_8(rotation_rad);
    // 偶数 bucket (0, 2, 4, 6 = 0, 90, 180, 270°) で base or 90° 反転、
    // 奇数 bucket (45, 135, 225, 315°) で水平 / 垂直に縮退
    if bucket % 2 == 0 {
        // 直交バケット: 90° ごとに NeSw ⇄ NwSe が入れ替わる
        let swap = (bucket / 2) % 2 == 1;
        match (base, swap) {
            (0, false) | (1, true) => egui::CursorIcon::ResizeNwSe,
            _ => egui::CursorIcon::ResizeNeSw,
        }
    } else {
        // 45° バケット: 角ハンドルが水平 or 垂直に乗る
        let horizontal = (bucket / 2) % 2 == 0;
        if (base == 0) ^ horizontal {
            egui::CursorIcon::ResizeHorizontal
        } else {
            egui::CursorIcon::ResizeVertical
        }
    }
}

/// 辺中点ハンドルのカーソル。回転 0 では N/S=Vertical, E/W=Horizontal。
fn edge_cursor(idx: u8, rotation_rad: f32) -> egui::CursorIcon {
    // canonical では idx 0=N, 1=E, 2=S, 3=W
    // base direction: 0=vertical (N/S), 1=horizontal (E/W)
    let base = if idx == 0 || idx == 2 { 0 } else { 1 };
    let bucket = rotation_bucket_8(rotation_rad);
    if bucket % 2 == 0 {
        let swap = (bucket / 2) % 2 == 1;
        match (base, swap) {
            (0, false) | (1, true) => egui::CursorIcon::ResizeVertical,
            _ => egui::CursorIcon::ResizeHorizontal,
        }
    } else {
        // 45° バケット: 斜めなので NeSw / NwSe にマップ
        let swap = (bucket / 2) % 2 == 1;
        match (base, swap) {
            (0, false) | (1, true) => egui::CursorIcon::ResizeNeSw,
            _ => egui::CursorIcon::ResizeNwSe,
        }
    }
}

/// 回転角を 8 方向の bucket (0..=7、0=0°, 1=45°, ..., 7=315°) に正規化。
fn rotation_bucket_8(rotation_rad: f32) -> u8 {
    let deg = rotation_rad.to_degrees().rem_euclid(360.0);
    let bucket = ((deg + 22.5) / 45.0).floor() as i32;
    (bucket.rem_euclid(8)) as u8
}

// ── begin_drag ───────────────────────────────────────────────────────────

/// ホバーターゲットと現在位置から初期 [`DragState`] を作る。
///
/// Resize 時の `anchor` は target に応じて固定点 (反対角・反対辺中点) を計算する。
/// Alt+リサイズ (中心固定) への切替は `apply_drag` の側で行うので、ここでは
/// 通常のリサイズ用 anchor を入れる。
pub fn begin_drag(target: HoverTarget, idx: usize, base: Shape, cur: (f32, f32)) -> DragState {
    match target {
        HoverTarget::Body => DragState::Pan {
            idx,
            base,
            origin: cur,
        },
        HoverTarget::Endpoint { which_p1 } => DragState::Endpoint {
            idx,
            base,
            which_p1,
            origin: cur,
        },
        HoverTarget::RotateHandle => {
            let center = base.center();
            let start_angle = (cur.1 - center.1).atan2(cur.0 - center.0);
            DragState::Rotate {
                idx,
                base,
                center,
                start_angle,
            }
        }
        HoverTarget::Corner { idx: ci } => {
            // 反対角を anchor に
            let anchor = opposite_corner(&base, ci);
            DragState::Resize {
                idx,
                base,
                target,
                origin: cur,
                anchor,
            }
        }
        HoverTarget::EdgeMidpoint { idx: ei } => {
            // 反対辺中点を anchor に
            let anchor = opposite_edge_midpoint(&base, ei);
            DragState::Resize {
                idx,
                base,
                target,
                origin: cur,
                anchor,
            }
        }
    }
}

/// 与えた角インデックス `ci` の反対角の画像座標を返す。
fn opposite_corner(shape: &Shape, ci: u8) -> (f32, f32) {
    let (center, half_w, half_h, rot) = bbox_params(shape);
    let corners = rect_corners(center, half_w, half_h, rot);
    let opp = match ci {
        0 => 2, // NW vs SE
        1 => 3, // NE vs SW
        2 => 0,
        3 => 1,
        _ => 0,
    };
    corners[opp as usize]
}

/// 与えた辺中点インデックス `ei` の反対辺中点を返す。
fn opposite_edge_midpoint(shape: &Shape, ei: u8) -> (f32, f32) {
    let (center, half_w, half_h, rot) = bbox_params(shape);
    let mids = rect_edge_midpoints(center, half_w, half_h, rot);
    let opp = match ei {
        0 => 2, // N vs S
        1 => 3, // E vs W
        2 => 0,
        3 => 1,
        _ => 0,
    };
    mids[opp as usize]
}

/// Rect / Ellipse の (center, half_w, half_h, rotation) を取り出す (Line は半サイズ 0 で代用)。
fn bbox_params(shape: &Shape) -> ((f32, f32), f32, f32, f32) {
    match shape {
        Shape::Rect {
            center,
            half_w,
            half_h,
            rotation_rad,
        } => (*center, *half_w, *half_h, *rotation_rad),
        Shape::Ellipse {
            center,
            rx,
            ry,
            rotation_rad,
        } => (*center, *rx, *ry, *rotation_rad),
        Shape::Line { .. } => (shape.center(), 0.0, 0.0, 0.0),
    }
}

// ── apply_drag ───────────────────────────────────────────────────────────

/// ドラッグ更新: `state` (base + drag 種別) と現在位置 `cur` から新しい Shape を計算する。
///
/// `modifiers`:
/// - Shift: 軸拘束 (Pan は v1 では何もしない) / 等比リサイズ / 角度スナップ
/// - Alt: 中心固定リサイズ (反対辺/角も連動)
///
/// 純関数 (副作用なし、テスト容易)。
pub fn apply_drag(state: &DragState, cur: (f32, f32), modifiers: &egui::Modifiers) -> Shape {
    match *state {
        DragState::Pan { base, origin, .. } => {
            let dx = cur.0 - origin.0;
            let dy = cur.1 - origin.1;
            let mut s = base;
            s.translate(dx, dy);
            s
        }
        DragState::Endpoint {
            base,
            which_p1,
            origin,
            ..
        } => apply_endpoint_drag(base, which_p1, origin, cur, modifiers),
        DragState::Resize {
            base,
            target,
            origin: _,
            anchor,
            ..
        } => apply_resize_drag(base, target, cur, anchor, modifiers),
        DragState::Rotate {
            base,
            center,
            start_angle,
            ..
        } => apply_rotate_drag(base, center, start_angle, cur, modifiers),
    }
}

fn apply_endpoint_drag(
    base: Shape,
    which_p1: bool,
    origin: (f32, f32),
    cur: (f32, f32),
    modifiers: &egui::Modifiers,
) -> Shape {
    let mut s = base;
    if let Shape::Line {
        ref mut p0,
        ref mut p1,
        ..
    } = s
    {
        // 固定端
        let fixed = if which_p1 { *p0 } else { *p1 };
        let mut moved = cur;
        if modifiers.shift {
            // 角度を 0/45/90/... にスナップ
            let dx = moved.0 - fixed.0;
            let dy = moved.1 - fixed.1;
            let len = (dx * dx + dy * dy).sqrt();
            if len > 0.5 {
                let angle = dy.atan2(dx).to_degrees();
                let snapped = snap_to_angles(angle, LINE_SNAP_ANGLES_DEG);
                let r = snapped.to_radians();
                moved = (fixed.0 + len * r.cos(), fixed.1 + len * r.sin());
            }
        }
        if which_p1 {
            *p1 = moved;
        } else {
            *p0 = moved;
        }
        // origin は未使用 (端点ドラッグは絶対位置を取り扱う)
        let _ = origin;
        return s;
    }
    s
}

/// 一連の許容角度集合の中で `deg` に最も近いものを返す。
fn snap_to_angles(deg: f32, allowed: &[f32]) -> f32 {
    let mut best = allowed[0];
    let mut best_d = f32::INFINITY;
    for &a in allowed {
        let d = angle_diff_deg(deg, a).abs();
        if d < best_d {
            best_d = d;
            best = a;
        }
    }
    best
}

/// 角度差を -180..=180 に正規化。
fn angle_diff_deg(a: f32, b: f32) -> f32 {
    let mut d = (a - b).rem_euclid(360.0);
    if d > 180.0 {
        d -= 360.0;
    }
    d
}

/// Resize ドラッグの本体。
///
/// 戦略: shape のローカル座標系 (回転考慮) で `anchor → cur` を見て、
/// その投影から新しい half_w / half_h を求める。Alt のときは center を据え置いて
/// anchor を center に置き換えた相対ベクトルで再計算する。
fn apply_resize_drag(
    base: Shape,
    target: HoverTarget,
    cur: (f32, f32),
    anchor: (f32, f32),
    modifiers: &egui::Modifiers,
) -> Shape {
    let (orig_center, orig_hw, orig_hh, rotation) = bbox_params(&base);

    // shape ローカル座標 (rotation を打ち消す)
    let (sin_r, cos_r) = rotation.sin_cos();
    let to_local = |p: (f32, f32), origin: (f32, f32)| -> (f32, f32) {
        let dx = p.0 - origin.0;
        let dy = p.1 - origin.1;
        (cos_r * dx + sin_r * dy, -sin_r * dx + cos_r * dy)
    };

    // Alt: 中心固定。anchor を center に上書きし、center を据え置く。
    let (use_anchor, keep_center) = if modifiers.alt {
        (orig_center, true)
    } else {
        (anchor, false)
    };

    // anchor → cur のローカル差分から、目的の dx, dy を取り出す
    let local_cur = to_local(cur, use_anchor);

    // 角と辺中点で意味が違う
    let (mut new_hw, mut new_hh) = (orig_hw, orig_hh);
    let dx = local_cur.0.abs();
    let dy = local_cur.1.abs();

    match target {
        HoverTarget::Corner { .. } => {
            if keep_center {
                // 中心固定: dx/dy はそのまま半サイズ
                new_hw = dx.max(1.0);
                new_hh = dy.max(1.0);
            } else {
                // 反対角を anchor: dx/dy はフルサイズ → /2 で半サイズ
                new_hw = (dx * 0.5).max(1.0);
                new_hh = (dy * 0.5).max(1.0);
            }
            if modifiers.shift {
                // 等比: 元の half_w/half_h 比を維持
                let aspect = if orig_hh.abs() > 1e-3 {
                    orig_hw / orig_hh
                } else {
                    1.0
                };
                // 新サイズの長辺を維持し、短辺を比率で縮める
                if new_hw / new_hh > aspect {
                    new_hh = new_hw / aspect;
                } else {
                    new_hw = new_hh * aspect;
                }
            }
        }
        HoverTarget::EdgeMidpoint { idx } => {
            // 辺中点: 水平方向 (E/W、idx=1/3) なら hw のみ、垂直 (N/S、idx=0/2) なら hh のみ
            let horizontal = idx == 1 || idx == 3;
            if horizontal {
                if keep_center {
                    new_hw = dx.max(1.0);
                } else {
                    new_hw = (dx * 0.5).max(1.0);
                }
            } else if keep_center {
                new_hh = dy.max(1.0);
            } else {
                new_hh = (dy * 0.5).max(1.0);
            }
            // Shift+辺中点は無効 (本来軸固定なので等比拘束が無意味)
        }
        _ => {}
    }

    // 新 center: 中心固定なら orig_center、そうでなければ anchor と cur の中点
    let new_center = if keep_center {
        orig_center
    } else {
        match target {
            HoverTarget::Corner { .. } => ((anchor.0 + cur.0) * 0.5, (anchor.1 + cur.1) * 0.5),
            HoverTarget::EdgeMidpoint { idx } => {
                let horizontal = idx == 1 || idx == 3;
                if horizontal {
                    ((anchor.0 + cur.0) * 0.5, anchor.1)
                } else {
                    (anchor.0, (anchor.1 + cur.1) * 0.5)
                }
                // ↑ rotation=0 の近似。rotation 付きで辺中点ハンドルを使うと
                //   center が完璧には乗らないが、辺中点リサイズはほぼ軸並行で
                //   使われるケースが大半なので Phase 2 では許容。
            }
            _ => orig_center,
        }
    };

    match base {
        Shape::Rect { rotation_rad, .. } => Shape::Rect {
            center: new_center,
            half_w: new_hw,
            half_h: new_hh,
            rotation_rad,
        },
        Shape::Ellipse { rotation_rad, .. } => Shape::Ellipse {
            center: new_center,
            rx: new_hw,
            ry: new_hh,
            rotation_rad,
        },
        _ => base, // Line には角/辺ハンドルは無い (begin_drag 側で起きない)
    }
}

fn apply_rotate_drag(
    base: Shape,
    center: (f32, f32),
    start_angle: f32,
    cur: (f32, f32),
    modifiers: &egui::Modifiers,
) -> Shape {
    let cur_angle = (cur.1 - center.1).atan2(cur.0 - center.0);
    let mut delta = cur_angle - start_angle;
    if modifiers.shift {
        // 15° スナップ: base の rotation_rad に delta を足した最終値を 15° 刻みに丸める
        let base_rot_deg = match base {
            Shape::Rect { rotation_rad, .. } | Shape::Ellipse { rotation_rad, .. } => {
                rotation_rad.to_degrees()
            }
            _ => 0.0,
        };
        let final_deg = base_rot_deg + delta.to_degrees();
        let snapped = (final_deg / ROTATE_SNAP_DEG).round() * ROTATE_SNAP_DEG;
        delta = (snapped - base_rot_deg).to_radians();
    }
    let mut s = base;
    s.rotate_around(center.0, center.1, delta);
    s
}

// ── draw_handles ─────────────────────────────────────────────────────────

/// ハンドルを painter で描画する。
///
/// `selected=true` で実線、`false` で半透明 (= 通常表示。Phase 2 では選択されていない
/// オブジェクトのハンドルは描かない方針なので呼び出し側で出し分けする)。
/// `hovered` が `Some` ならそのハンドルだけ強調色で塗る。
/// `to_screen` は画像座標 → スクリーン座標の変換。
pub fn draw_handles(
    painter: &egui::Painter,
    layout: &HandleLayout,
    selected: bool,
    hovered: Option<HoverTarget>,
    to_screen: &dyn Fn((f32, f32)) -> egui::Pos2,
) {
    let stroke_color = if selected {
        egui::Color32::from_rgb(180, 220, 255)
    } else {
        egui::Color32::from_rgba_unmultiplied(180, 220, 255, 140)
    };
    let fill = egui::Color32::from_rgb(40, 60, 90);
    let hover_fill = egui::Color32::from_rgb(255, 180, 60);

    // 本体の枠
    let body_pts: Vec<egui::Pos2> = layout.body_corners.iter().map(|&p| to_screen(p)).collect();
    if body_pts.len() == 4 {
        painter.add(egui::Shape::closed_line(
            body_pts.clone(),
            egui::Stroke::new(1.5, stroke_color),
        ));
    }

    // 端点
    if let Some(eps) = layout.endpoints {
        for (i, &p) in eps.iter().enumerate() {
            let is_hovered = hovered == Some(HoverTarget::Endpoint { which_p1: i == 1 });
            let f = if is_hovered { hover_fill } else { fill };
            painter.circle(
                to_screen(p),
                HANDLE_DRAW_RADIUS_PX,
                f,
                egui::Stroke::new(1.5, stroke_color),
            );
        }
    }
    // 角
    if let Some(cs) = layout.corners {
        for (i, &p) in cs.iter().enumerate() {
            let is_hovered = hovered == Some(HoverTarget::Corner { idx: i as u8 });
            let f = if is_hovered { hover_fill } else { fill };
            painter.circle(
                to_screen(p),
                HANDLE_DRAW_RADIUS_PX,
                f,
                egui::Stroke::new(1.5, stroke_color),
            );
        }
    }
    // 辺中点
    if let Some(es) = layout.edge_midpoints {
        for (i, &p) in es.iter().enumerate() {
            let is_hovered = hovered == Some(HoverTarget::EdgeMidpoint { idx: i as u8 });
            let f = if is_hovered { hover_fill } else { fill };
            // 辺中点は四角形で描画 (角と区別)
            let center = to_screen(p);
            let r = HANDLE_DRAW_RADIUS_PX;
            painter.rect(
                egui::Rect::from_center_size(center, egui::vec2(r * 2.0, r * 2.0)),
                0.0,
                f,
                egui::Stroke::new(1.5, stroke_color),
                egui::StrokeKind::Inside,
            );
        }
    }
    // 回転ハンドル: 棒 + 円 + ↻ アイコン
    if let Some(rh) = layout.rotate_handle {
        let rh_screen = to_screen(rh);
        // 棒の元: bbox 上辺中点 (= edge_midpoints[0])
        if let Some(es) = layout.edge_midpoints {
            let top = to_screen(es[0]);
            painter.line_segment([top, rh_screen], egui::Stroke::new(1.5, stroke_color));
        }
        let is_hovered = hovered == Some(HoverTarget::RotateHandle);
        let f = if is_hovered { hover_fill } else { fill };
        painter.circle(
            rh_screen,
            ROTATE_HANDLE_DRAW_RADIUS_PX,
            f,
            egui::Stroke::new(1.5, stroke_color),
        );
        // ↻ アイコン (半透明 16px)
        painter.text(
            rh_screen + egui::vec2(0.0, -ROTATE_HANDLE_DRAW_RADIUS_PX - 10.0),
            egui::Align2::CENTER_CENTER,
            "↻",
            egui::FontId::proportional(16.0),
            egui::Color32::from_rgba_unmultiplied(255, 255, 255, 160),
        );
    }
}

// ── テスト ───────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mask_db::{LineKind, Shape};

    fn rect_at(cx: f32, cy: f32, hw: f32, hh: f32, rot: f32) -> Shape {
        Shape::Rect {
            center: (cx, cy),
            half_w: hw,
            half_h: hh,
            rotation_rad: rot,
        }
    }

    fn ellipse_at(cx: f32, cy: f32, rx: f32, ry: f32, rot: f32) -> Shape {
        Shape::Ellipse {
            center: (cx, cy),
            rx,
            ry,
            rotation_rad: rot,
        }
    }

    fn line_at(p0: (f32, f32), p1: (f32, f32), th: f32) -> Shape {
        Shape::Line {
            kind: LineKind::Diagonal,
            p0,
            p1,
            thickness: th,
        }
    }

    #[test]
    fn handle_layout_rect_axis_aligned() {
        let r = rect_at(100.0, 100.0, 30.0, 20.0, 0.0);
        let layout = compute_handle_layout(&r, 1.0);
        // body / corners は同じ (回転 0)
        let cs = layout.corners.unwrap();
        // NW (-, -) なので (70, 80)
        assert!((cs[0].0 - 70.0).abs() < 1e-3 && (cs[0].1 - 80.0).abs() < 1e-3);
        // SE (+, +) なので (130, 120)
        assert!((cs[2].0 - 130.0).abs() < 1e-3 && (cs[2].1 - 120.0).abs() < 1e-3);
        // 辺中点 N は (100, 80)
        let mids = layout.edge_midpoints.unwrap();
        assert!((mids[0].0 - 100.0).abs() < 1e-3 && (mids[0].1 - 80.0).abs() < 1e-3);
        // 回転ハンドル: 上辺中点から 28px / scale=1.0 = 28 だけ上 → (100, 52)
        let rh = layout.rotate_handle.unwrap();
        assert!((rh.0 - 100.0).abs() < 1e-3 && (rh.1 - 52.0).abs() < 1e-3);
    }

    #[test]
    fn hit_test_rect_corner_inside_and_outside() {
        let r = rect_at(100.0, 100.0, 30.0, 20.0, 0.0);
        let layout = compute_handle_layout(&r, 1.0);

        // NW 角の中心ヒット
        let h = hit_test(&layout, (70.0, 80.0), 1.0);
        assert_eq!(h, Some(HoverTarget::Corner { idx: 0 }));
        // 角ハンドルから少しずれる (ヒット半径内)
        let h2 = hit_test(&layout, (75.0, 84.0), 1.0);
        assert_eq!(h2, Some(HoverTarget::Corner { idx: 0 }));
        // 完全に外 (本体内のクリック)
        let h3 = hit_test(&layout, (100.0, 100.0), 1.0);
        assert_eq!(h3, Some(HoverTarget::Body));
        // 完全な外側
        let h4 = hit_test(&layout, (200.0, 200.0), 1.0);
        assert_eq!(h4, None);
    }

    #[test]
    fn hit_test_rotate_handle_above_top_edge() {
        let r = rect_at(100.0, 100.0, 30.0, 20.0, 0.0);
        let layout = compute_handle_layout(&r, 1.0);
        // 上辺中点から 28px 上の点が回転ハンドル
        let h = hit_test(&layout, (100.0, 52.0), 1.0);
        assert_eq!(h, Some(HoverTarget::RotateHandle));
    }

    #[test]
    fn hit_test_endpoint_line() {
        let l = line_at((10.0, 10.0), (90.0, 10.0), 4.0);
        let layout = compute_handle_layout(&l, 1.0);
        // p0 ヒット
        let h0 = hit_test(&layout, (10.0, 10.0), 1.0);
        assert_eq!(h0, Some(HoverTarget::Endpoint { which_p1: false }));
        // p1 ヒット
        let h1 = hit_test(&layout, (90.0, 10.0), 1.0);
        assert_eq!(h1, Some(HoverTarget::Endpoint { which_p1: true }));
        // 中央は本体
        let hb = hit_test(&layout, (50.0, 10.0), 1.0);
        assert_eq!(hb, Some(HoverTarget::Body));
    }

    #[test]
    fn apply_drag_pan_translates() {
        let r = rect_at(100.0, 100.0, 30.0, 20.0, 0.0);
        let drag = DragState::Pan {
            idx: 0,
            base: r,
            origin: (100.0, 100.0),
        };
        let new = apply_drag(&drag, (150.0, 200.0), &egui::Modifiers::NONE);
        if let Shape::Rect { center, .. } = new {
            assert!((center.0 - 150.0).abs() < 1e-3);
            assert!((center.1 - 200.0).abs() < 1e-3);
        } else {
            panic!("expected Rect");
        }
    }

    #[test]
    fn apply_drag_resize_corner_anchor_opposite() {
        // 反対角 anchor で SE 角ハンドルを動かす → 新サイズはドラッグ距離の半分
        let r = rect_at(100.0, 100.0, 30.0, 20.0, 0.0);
        let layout = compute_handle_layout(&r, 1.0);
        let anchor = layout.corners.unwrap()[0]; // NW
        let drag = DragState::Resize {
            idx: 0,
            base: r,
            target: HoverTarget::Corner { idx: 2 }, // SE
            origin: (130.0, 120.0),
            anchor,
        };
        // SE を (170, 140) へ → 新 hw = (170-70)/2 = 50, 新 hh = (140-80)/2 = 30
        let new = apply_drag(&drag, (170.0, 140.0), &egui::Modifiers::NONE);
        if let Shape::Rect {
            center,
            half_w,
            half_h,
            ..
        } = new
        {
            assert!((half_w - 50.0).abs() < 1.0);
            assert!((half_h - 30.0).abs() < 1.0);
            // center は anchor と cur の中点
            assert!((center.0 - 120.0).abs() < 1.0);
            assert!((center.1 - 110.0).abs() < 1.0);
        } else {
            panic!("expected Rect");
        }
    }

    #[test]
    fn apply_drag_resize_alt_center_anchor() {
        // Alt: 中心固定
        let r = rect_at(100.0, 100.0, 30.0, 20.0, 0.0);
        let layout = compute_handle_layout(&r, 1.0);
        let anchor = layout.corners.unwrap()[0]; // NW
        let drag = DragState::Resize {
            idx: 0,
            base: r,
            target: HoverTarget::Corner { idx: 2 }, // SE
            origin: (130.0, 120.0),
            anchor,
        };
        let mods = egui::Modifiers {
            alt: true,
            ..egui::Modifiers::NONE
        };
        // SE を (140, 130) へ、中心固定 → 新 hw = 40, hh = 30, center 保持
        let new = apply_drag(&drag, (140.0, 130.0), &mods);
        if let Shape::Rect {
            center,
            half_w,
            half_h,
            ..
        } = new
        {
            assert!((half_w - 40.0).abs() < 1.0);
            assert!((half_h - 30.0).abs() < 1.0);
            assert!((center.0 - 100.0).abs() < 1e-3);
            assert!((center.1 - 100.0).abs() < 1e-3);
        } else {
            panic!("expected Rect");
        }
    }

    #[test]
    fn apply_drag_resize_shift_equal_aspect() {
        // Shift+角: 等比 (元の aspect = 30/20 = 1.5 を維持)
        let r = rect_at(100.0, 100.0, 30.0, 20.0, 0.0);
        let layout = compute_handle_layout(&r, 1.0);
        let anchor = layout.corners.unwrap()[0]; // NW
        let drag = DragState::Resize {
            idx: 0,
            base: r,
            target: HoverTarget::Corner { idx: 2 },
            origin: (130.0, 120.0),
            anchor,
        };
        let mods = egui::Modifiers {
            shift: true,
            ..egui::Modifiers::NONE
        };
        // SE を (170, 200) へ → without shift: hw=50, hh=60。aspect 1.5 維持で hw=60*1.5=90? いや
        // hw/hh ratio が aspect (1.5) より小なら hw を hh*1.5 に、大なら hh を hw/1.5 に。
        // hw/hh = 50/60 = 0.83 < 1.5 → hw = hh * 1.5 = 90
        let new = apply_drag(&drag, (170.0, 200.0), &mods);
        if let Shape::Rect { half_w, half_h, .. } = new {
            assert!(
                (half_w / half_h - 1.5).abs() < 0.05,
                "aspect must match: hw={half_w}, hh={half_h}"
            );
        } else {
            panic!("expected Rect");
        }
    }

    #[test]
    fn apply_drag_rotate_shift_snap_15deg() {
        let r = rect_at(100.0, 100.0, 30.0, 20.0, 0.0);
        // 開始時の center → origin 角度 = 0° (origin が center の右)
        let drag = DragState::Rotate {
            idx: 0,
            base: r,
            center: (100.0, 100.0),
            start_angle: 0.0,
        };
        let mods = egui::Modifiers {
            shift: true,
            ..egui::Modifiers::NONE
        };
        // 17° に相当する点へ動かす → 15° に snap
        let target_deg = 17.0_f32;
        let r17 = target_deg.to_radians();
        let cur = (100.0 + r17.cos() * 100.0, 100.0 + r17.sin() * 100.0);
        let new = apply_drag(&drag, cur, &mods);
        if let Shape::Rect { rotation_rad, .. } = new {
            let deg = rotation_rad.to_degrees();
            assert!((deg - 15.0).abs() < 0.5, "expected 15° snap, got {deg}°");
        } else {
            panic!("expected Rect");
        }
    }

    #[test]
    fn apply_drag_endpoint_shift_angle_snap() {
        // (0, 0) → (100, 5) で 17° に近いベクトル。Shift で 0° スナップ → p1 = (100, 0)
        let l = line_at((0.0, 0.0), (100.0, 0.0), 4.0);
        let drag = DragState::Endpoint {
            idx: 0,
            base: l,
            which_p1: true,
            origin: (100.0, 0.0),
        };
        let mods = egui::Modifiers {
            shift: true,
            ..egui::Modifiers::NONE
        };
        let new = apply_drag(&drag, (100.0, 5.0), &mods);
        if let Shape::Line { p0, p1, .. } = new {
            assert_eq!(p0, (0.0, 0.0));
            // 角度 0° スナップ → x が約 100.06、y が約 0
            assert!(p1.0 > 95.0 && p1.0 < 105.0);
            assert!(p1.1.abs() < 1.0);
        } else {
            panic!("expected Line");
        }
    }

    #[test]
    fn cursor_icon_rotation_buckets() {
        // 回転 0 では NW 角は NwSe
        let r0 = rect_at(0.0, 0.0, 10.0, 10.0, 0.0);
        assert_eq!(
            cursor_icon_for(HoverTarget::Corner { idx: 0 }, &r0),
            egui::CursorIcon::ResizeNwSe
        );
        // 回転 90° では NW 位置は NE になり、カーソルは NeSw
        let r90 = rect_at(0.0, 0.0, 10.0, 10.0, std::f32::consts::FRAC_PI_2);
        assert_eq!(
            cursor_icon_for(HoverTarget::Corner { idx: 0 }, &r90),
            egui::CursorIcon::ResizeNeSw
        );
        // 辺中点 N は回転 0 で垂直カーソル
        assert_eq!(
            cursor_icon_for(HoverTarget::EdgeMidpoint { idx: 0 }, &r0),
            egui::CursorIcon::ResizeVertical
        );
        // 回転ハンドルは PointingHand
        assert_eq!(
            cursor_icon_for(HoverTarget::RotateHandle, &r0),
            egui::CursorIcon::PointingHand
        );
    }

    #[test]
    fn begin_drag_corner_anchor_is_opposite() {
        let r = rect_at(100.0, 100.0, 30.0, 20.0, 0.0);
        // SE 角からドラッグ開始 → anchor は NW 角
        let drag = begin_drag(HoverTarget::Corner { idx: 2 }, 0, r, (130.0, 120.0));
        if let DragState::Resize { anchor, .. } = drag {
            assert!((anchor.0 - 70.0).abs() < 1e-3);
            assert!((anchor.1 - 80.0).abs() < 1e-3);
        } else {
            panic!("expected Resize");
        }
    }

    #[test]
    fn begin_drag_rotate_records_start_angle() {
        let r = rect_at(100.0, 100.0, 30.0, 20.0, 0.0);
        // 開始位置 (200, 100) は center から 0° 方向
        let drag = begin_drag(HoverTarget::RotateHandle, 0, r, (200.0, 100.0));
        if let DragState::Rotate {
            center,
            start_angle,
            ..
        } = drag
        {
            assert_eq!(center, (100.0, 100.0));
            assert!(start_angle.abs() < 1e-3, "expected ~0 rad");
        } else {
            panic!("expected Rotate");
        }
    }

    #[test]
    fn point_in_polygon_basic() {
        let sq = [(0.0, 0.0), (10.0, 0.0), (10.0, 10.0), (0.0, 10.0)];
        assert!(point_in_polygon((5.0, 5.0), &sq));
        assert!(!point_in_polygon((11.0, 5.0), &sq));
        assert!(!point_in_polygon((5.0, -1.0), &sq));
    }

    #[test]
    fn ellipse_handle_layout_matches_bbox() {
        let e = ellipse_at(100.0, 100.0, 40.0, 20.0, 0.0);
        let layout = compute_handle_layout(&e, 1.0);
        let cs = layout.corners.unwrap();
        // bbox NW = (60, 80)
        assert!((cs[0].0 - 60.0).abs() < 1e-3 && (cs[0].1 - 80.0).abs() < 1e-3);
        // 回転ハンドルは ry = 20 の上端 + 28 = (100, 52)
        let rh = layout.rotate_handle.unwrap();
        assert!((rh.0 - 100.0).abs() < 1e-3 && (rh.1 - 52.0).abs() < 1e-3);
    }
}
