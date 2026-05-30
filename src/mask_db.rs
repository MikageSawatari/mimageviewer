//! 消しゴムマスクの永続管理。
//!
//! `%APPDATA%/mimageviewer/mask.db` にマスク情報を保存する。
//! マスクは 1bit/pixel にパックし、deflate 圧縮して BLOB に格納する。
//!
//! 縦線/横線/直線はベクタオブジェクト (`LineObject`) として `vectors` 列に
//! JSON 文字列で保存する。囲み/筆はビットマップ側にラスタライズ済み。
//!
//! # Shape 拡張 (隠蔽加工機能対応、2026-05)
//!
//! 隠蔽加工機能 (`docs/conceal-feature-plan.md`) と消しゴム機能で 8 ツール
//! (Select / Brush / Lasso / Line / Vert / Horiz / Rect / Ellipse) を共用するため、
//! ベクタオブジェクトの統一表現として [`Shape`] enum を導入する。`Shape` は
//! `Line` / `Rect` / `Ellipse` の 3 variant を持ち、JSON では `{"type": "..."}`
//! 形式のタグ付きでシリアライズされる。
//!
//! ## 後方互換性 (リリース済みデータ対応)
//!
//! `mask_db` は既にリリース済みで、旧形式の `LineObject` JSON
//! (`{"kind": "diag", "p0": [..], "p1": [..], "thickness": ..}` — `"type"` キー
//! なし) が各ユーザーの中央 DB / サイドカーに保存されている。`Shape` の
//! `Deserialize` は **明示的な `"type"` キー判定** で以下を区別する:
//!
//! - `"type"` フィールド有 → タグ付き新形式として解釈、不明な `"type"` 値は
//!   エラー (silently fallback しない)
//! - `"type"` フィールド無し + `"kind"` / `"p0"` / `"p1"` 有 → 旧 LineObject
//!   として解釈し、`Shape::Line` に変換
//! - いずれにも該当しない → エラー
//!
//! `Serialize` は常に新形式 (`"type": "..."`) で書く。旧データを一度開いて
//! 保存し直すと自動的に新形式へ移行する。

use std::io::{Read, Write};
use std::path::PathBuf;

use flate2::Compression;
use flate2::read::DeflateDecoder;
use flate2::write::DeflateEncoder;
use serde::{Deserialize, Deserializer, Serialize};

/// ベクタ線オブジェクトの種別。作成時のツールで決まる。
/// 作成時の挙動 (初期幾何) のみに影響し、保存後の編集では区別しない。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum LineKind {
    #[serde(rename = "vert")]
    Vertical,
    #[serde(rename = "horiz")]
    Horizontal,
    #[serde(rename = "diag")]
    Diagonal,
}

/// 1 本のベクタ線オブジェクト。
///
/// `p0` → `p1` を結ぶ中心軸に沿った、厚さ `thickness` の矩形としてラスタライズする。
/// 縦/横線も内部的にはこの形式で保存し、rasterize 時に差異はない。
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct LineObject {
    pub kind: LineKind,
    pub p0: (f32, f32),
    pub p1: (f32, f32),
    pub thickness: f32,
}

impl LineObject {
    /// オブジェクトの中心点 (回転基準)。
    pub fn center(&self) -> (f32, f32) {
        ((self.p0.0 + self.p1.0) * 0.5, (self.p0.1 + self.p1.1) * 0.5)
    }

    /// オブジェクトを (dx, dy) だけ平行移動する。
    pub fn translate(&mut self, dx: f32, dy: f32) {
        self.p0.0 += dx;
        self.p0.1 += dy;
        self.p1.0 += dx;
        self.p1.1 += dy;
    }

    /// 指定中心周りに `angle` [rad] 回転する。
    pub fn rotate_around(&mut self, cx: f32, cy: f32, angle: f32) {
        let (s, c) = angle.sin_cos();
        let rot = |p: (f32, f32)| -> (f32, f32) {
            let dx = p.0 - cx;
            let dy = p.1 - cy;
            (cx + dx * c - dy * s, cy + dx * s + dy * c)
        };
        self.p0 = rot(self.p0);
        self.p1 = rot(self.p1);
    }

    /// 4 隅の矩形コーナーを返す (ラスタライズ/ヒットテスト用)。
    /// `extra_thickness` は判定に少し余裕を持たせる用途。
    pub fn corners(&self, extra_thickness: f32) -> [(f32, f32); 4] {
        let dx = self.p1.0 - self.p0.0;
        let dy = self.p1.1 - self.p0.1;
        let len = (dx * dx + dy * dy).sqrt().max(1e-6);
        let nx = -dy / len;
        let ny = dx / len;
        let half = (self.thickness * 0.5 + extra_thickness).max(0.0);
        [
            (self.p0.0 + nx * half, self.p0.1 + ny * half),
            (self.p1.0 + nx * half, self.p1.1 + ny * half),
            (self.p1.0 - nx * half, self.p1.1 - ny * half),
            (self.p0.0 - nx * half, self.p0.1 - ny * half),
        ]
    }
}

/// ベクタ群を既存の 1bit マスク上にラスタライズする (in-place OR)。
pub fn rasterize_vectors_into(mask: &mut [bool], vectors: &[LineObject], w: usize, h: usize) {
    for v in vectors {
        let pts = v.corners(0.0);
        scanline_fill_polygon(mask, &pts, w, h, true);
    }
}

// ── Shape 拡張 (隠蔽加工対応、消しゴムと共有) ────────────────────────────
//
// `Shape` enum は 3 variant (Line / Rect / Ellipse) を持つ統一ベクタ表現。
// JSON では `{"type": "line" | "rect" | "ellipse", ...}` 形式 (タグ付き)。
//
// 旧 `LineObject` の素 JSON も読めるよう、`Deserialize` は明示的な `"type"` キー
// 判定で legacy / tagged を区別する (Codex review P1 指摘)。
// ─────────────────────────────────────────────────────────────────────────

/// 8 ツール共通のベクタオブジェクト統一表現。
///
/// - `Line`: 直線・縦線・横線の 3 ツール (中心軸 + 太さの矩形帯)
/// - `Rect`: 軸並行 or 任意回転の矩形
/// - `Ellipse`: 軸並行 or 任意回転の楕円
///
/// JSON 形式 (常にタグ付きで書き、両形式を読む):
///
/// ```json
/// {"type":"line","kind":"diag","p0":[10,10],"p1":[90,10],"thickness":4}
/// {"type":"rect","op":"subtract","center":[100,100],"half_w":40,"half_h":20,"rotation_rad":0}
/// {"type":"ellipse","center":[200,200],"rx":30,"ry":20,"rotation_rad":0}
/// ```
///
/// 旧 `LineObject` 素 JSON (`{"kind":"diag", "p0":..., "p1":..., "thickness":..}`)
/// も `Shape::Line` として読める。`op` 未指定の既存 Shape は `add` として扱う。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ShapeOp {
    Add,
    Subtract,
}

impl Default for ShapeOp {
    fn default() -> Self {
        Self::Add
    }
}

impl ShapeOp {
    pub fn is_add(&self) -> bool {
        matches!(self, Self::Add)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum Shape {
    Line {
        #[serde(default, skip_serializing_if = "ShapeOp::is_add")]
        op: ShapeOp,
        kind: LineKind,
        p0: (f32, f32),
        p1: (f32, f32),
        thickness: f32,
    },
    Rect {
        #[serde(default, skip_serializing_if = "ShapeOp::is_add")]
        op: ShapeOp,
        center: (f32, f32),
        half_w: f32,
        half_h: f32,
        rotation_rad: f32,
    },
    Ellipse {
        #[serde(default, skip_serializing_if = "ShapeOp::is_add")]
        op: ShapeOp,
        center: (f32, f32),
        rx: f32,
        ry: f32,
        rotation_rad: f32,
    },
}

impl Shape {
    /// この Shape が最終マスクへどう合成されるか。
    ///
    /// `Add` はマスクを追加し、`Subtract` はそれまでのビットマップ/Shape 結果を削る。
    /// 既存データは `op` 未指定なので `Add` として読み込む。
    pub fn op(&self) -> ShapeOp {
        match self {
            Shape::Line { op, .. } | Shape::Rect { op, .. } | Shape::Ellipse { op, .. } => *op,
        }
    }

    /// 同じ幾何形状のまま合成操作だけを変える。
    pub fn with_op(mut self, new_op: ShapeOp) -> Self {
        match &mut self {
            Shape::Line { op, .. } | Shape::Rect { op, .. } | Shape::Ellipse { op, .. } => {
                *op = new_op;
            }
        }
        self
    }

    /// オブジェクトの中心点 (回転基準)。
    pub fn center(&self) -> (f32, f32) {
        match self {
            Shape::Line { p0, p1, .. } => ((p0.0 + p1.0) * 0.5, (p0.1 + p1.1) * 0.5),
            Shape::Rect { center, .. } | Shape::Ellipse { center, .. } => *center,
        }
    }

    /// オブジェクトを (dx, dy) だけ平行移動する。
    pub fn translate(&mut self, dx: f32, dy: f32) {
        match self {
            Shape::Line { p0, p1, .. } => {
                p0.0 += dx;
                p0.1 += dy;
                p1.0 += dx;
                p1.1 += dy;
            }
            Shape::Rect { center, .. } | Shape::Ellipse { center, .. } => {
                center.0 += dx;
                center.1 += dy;
            }
        }
    }

    /// 指定中心周りに `angle` [rad] 回転する。
    /// Rect / Ellipse は `rotation_rad` フィールドにも積算される。
    pub fn rotate_around(&mut self, cx: f32, cy: f32, angle: f32) {
        let (s, c) = angle.sin_cos();
        let rot = |p: (f32, f32)| -> (f32, f32) {
            let dx = p.0 - cx;
            let dy = p.1 - cy;
            (cx + dx * c - dy * s, cy + dx * s + dy * c)
        };
        match self {
            Shape::Line { p0, p1, .. } => {
                *p0 = rot(*p0);
                *p1 = rot(*p1);
            }
            Shape::Rect {
                center,
                rotation_rad,
                ..
            }
            | Shape::Ellipse {
                center,
                rotation_rad,
                ..
            } => {
                *center = rot(*center);
                *rotation_rad += angle;
            }
        }
    }

    /// 画像サイズ変更時のスケーリング (mask_db::get_full でリスケール時に使う)。
    ///
    /// **制限事項**: 非等比スケール (`sx != sy`) で回転済み `Rect` / `Ellipse` は
    /// 数学的には剪断 (shear) を含む変換になり、現状の `center / half_w / half_h /
    /// rotation_rad` 表現では正確に再現できない。当面は近似 (center と各半径を
    /// 各軸で伸縮、回転角度は据え置き) で扱い、PDF ページの DPI 違いなど非等比
    /// スケールが起きる経路は仕様外 (回転 0 か isotropic の場合のみ正確) とする。
    pub fn scale_xy(&mut self, sx: f32, sy: f32) {
        match self {
            Shape::Line {
                p0, p1, thickness, ..
            } => {
                p0.0 *= sx;
                p0.1 *= sy;
                p1.0 *= sx;
                p1.1 *= sy;
                *thickness *= (sx + sy) * 0.5;
            }
            Shape::Rect {
                center,
                half_w,
                half_h,
                ..
            } => {
                center.0 *= sx;
                center.1 *= sy;
                *half_w *= sx;
                *half_h *= sy;
            }
            Shape::Ellipse { center, rx, ry, .. } => {
                center.0 *= sx;
                center.1 *= sy;
                *rx *= sx;
                *ry *= sy;
            }
        }
    }

    /// `Shape::Line` の場合だけ旧 `LineObject` に変換 (Phase 2b 移行期間用)。
    pub fn as_legacy_line(&self) -> Option<LineObject> {
        match self {
            Shape::Line {
                kind,
                p0,
                p1,
                thickness,
                ..
            } => Some(LineObject {
                kind: *kind,
                p0: *p0,
                p1: *p1,
                thickness: *thickness,
            }),
            _ => None,
        }
    }
}

impl From<LineObject> for Shape {
    fn from(line: LineObject) -> Self {
        Shape::Line {
            op: ShapeOp::Add,
            kind: line.kind,
            p0: line.p0,
            p1: line.p1,
            thickness: line.thickness,
        }
    }
}

// `Shape` の `Deserialize` は手書き。明示的に `"type"` キー存在を判定し、
// 不明な `"type"` 値や missing キーで silently legacy にフォールバックしない
// (Codex review P1)。
impl<'de> Deserialize<'de> for Shape {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        use serde::de::Error as DeError;
        let v = serde_json::Value::deserialize(d)?;
        let obj = v
            .as_object()
            .ok_or_else(|| DeError::custom("Shape must be a JSON object, got non-object"))?;

        if let Some(ty) = obj.get("type") {
            // タグ付き新形式: "type" の値を厳格に判定し、未知タイプはエラー
            let ty_str = ty
                .as_str()
                .ok_or_else(|| DeError::custom("Shape 'type' must be a string"))?;
            match ty_str {
                "line" => {
                    let f: TaggedLineFields =
                        serde_json::from_value(v.clone()).map_err(DeError::custom)?;
                    Ok(Shape::Line {
                        op: f.op,
                        kind: f.kind,
                        p0: f.p0,
                        p1: f.p1,
                        thickness: f.thickness,
                    })
                }
                "rect" => {
                    let f: TaggedRectFields =
                        serde_json::from_value(v.clone()).map_err(DeError::custom)?;
                    Ok(Shape::Rect {
                        op: f.op,
                        center: f.center,
                        half_w: f.half_w,
                        half_h: f.half_h,
                        rotation_rad: f.rotation_rad,
                    })
                }
                "ellipse" => {
                    let f: TaggedEllipseFields =
                        serde_json::from_value(v.clone()).map_err(DeError::custom)?;
                    Ok(Shape::Ellipse {
                        op: f.op,
                        center: f.center,
                        rx: f.rx,
                        ry: f.ry,
                        rotation_rad: f.rotation_rad,
                    })
                }
                other => Err(DeError::custom(format!(
                    "unknown Shape type: '{other}' (expected 'line' / 'rect' / 'ellipse')"
                ))),
            }
        } else if obj.contains_key("kind") && obj.contains_key("p0") && obj.contains_key("p1") {
            // 旧 LineObject (タグなし): 必須キー揃いを確認してから legacy として解釈
            let legacy: LegacyLineFields = serde_json::from_value(v).map_err(DeError::custom)?;
            Ok(Shape::Line {
                op: ShapeOp::Add,
                kind: legacy.kind,
                p0: legacy.p0,
                p1: legacy.p1,
                thickness: legacy.thickness,
            })
        } else {
            Err(DeError::custom(
                "Shape requires 'type' (new tagged format) or 'kind'+'p0'+'p1' (legacy LineObject)",
            ))
        }
    }
}

// タグ付き形式の中身を `serde_json::from_value` で取り出すためのヘルパー構造体。
// `"type"` フィールドは Shape::deserialize で先に判別しているため、ここでは
// その他のフィールドだけ受ける (`type` を含む追加フィールドは serde が無視する)。
#[derive(Deserialize)]
struct TaggedLineFields {
    #[serde(default)]
    op: ShapeOp,
    kind: LineKind,
    p0: (f32, f32),
    p1: (f32, f32),
    thickness: f32,
}

#[derive(Deserialize)]
struct TaggedRectFields {
    #[serde(default)]
    op: ShapeOp,
    center: (f32, f32),
    half_w: f32,
    half_h: f32,
    #[serde(default)]
    rotation_rad: f32,
}

#[derive(Deserialize)]
struct TaggedEllipseFields {
    #[serde(default)]
    op: ShapeOp,
    center: (f32, f32),
    rx: f32,
    ry: f32,
    #[serde(default)]
    rotation_rad: f32,
}

#[derive(Deserialize)]
struct LegacyLineFields {
    kind: LineKind,
    p0: (f32, f32),
    p1: (f32, f32),
    thickness: f32,
}

/// 軸並行 or 回転矩形の 4 corners を計算する。
/// `(center, half_w, half_h, rotation_rad)` → 反時計回りに `[NW, NE, SE, SW]`
/// (rotation 0 のとき)。
pub fn rect_corners(
    center: (f32, f32),
    half_w: f32,
    half_h: f32,
    rotation_rad: f32,
) -> [(f32, f32); 4] {
    let (s, c) = rotation_rad.sin_cos();
    let local = [
        (-half_w, -half_h),
        (half_w, -half_h),
        (half_w, half_h),
        (-half_w, half_h),
    ];
    let mut out = [(0.0_f32, 0.0_f32); 4];
    for i in 0..4 {
        let (lx, ly) = local[i];
        out[i] = (center.0 + c * lx - s * ly, center.1 + s * lx + c * ly);
    }
    out
}

/// 楕円 (回転対応) を 1bit マスクにラスタライズする (in-place)。
///
/// アルゴリズム: bbox を [`rect_corners`] と同じ向きで算出し、その中の各画素中心を
/// 逆回転して canonical 楕円方程式 `u²/rx² + v²/ry² <= 1` で in/out 判定する。
pub fn scanline_fill_ellipse(
    mask: &mut [bool],
    center: (f32, f32),
    rx: f32,
    ry: f32,
    rotation_rad: f32,
    w: usize,
    h: usize,
    value: bool,
) {
    if rx <= 0.0 || ry <= 0.0 {
        return;
    }
    // bbox 計算: 回転楕円の半径 (任意角度)
    //   bbox 半幅 = sqrt(rx² cos²θ + ry² sin²θ)
    //   bbox 半高 = sqrt(rx² sin²θ + ry² cos²θ)
    let (s, c) = rotation_rad.sin_cos();
    let hw = (rx * rx * c * c + ry * ry * s * s).sqrt();
    let hh = (rx * rx * s * s + ry * ry * c * c).sqrt();
    let min_x = (center.0 - hw).floor().max(0.0) as usize;
    let max_x = ((center.0 + hw).ceil()).min(w as f32) as usize;
    let min_y = (center.1 - hh).floor().max(0.0) as usize;
    let max_y = ((center.1 + hh).ceil()).min(h as f32) as usize;
    let inv_rx2 = 1.0 / (rx * rx);
    let inv_ry2 = 1.0 / (ry * ry);
    // 逆回転: (u, v) = R(-θ) (px, py) = (c·px + s·py, -s·px + c·py)
    for y in min_y..max_y {
        let py = (y as f32 + 0.5) - center.1;
        let row = y * w;
        for x in min_x..max_x {
            let px = (x as f32 + 0.5) - center.0;
            let u = c * px + s * py;
            let v = -s * px + c * py;
            if u * u * inv_rx2 + v * v * inv_ry2 <= 1.0 {
                mask[row + x] = value;
            }
        }
    }
}

/// `Shape` を 1bit マスクへラスタライズする (in-place)。
///
/// `value=true` でマスクを ON、`false` で OFF (消去) として塗る。
pub fn rasterize_shape_into(mask: &mut [bool], shape: &Shape, w: usize, h: usize, value: bool) {
    match shape {
        Shape::Line {
            p0, p1, thickness, ..
        } => {
            // 既存の LineObject::corners と同じ「中心軸 + 太さの矩形帯」表現。
            let line = LineObject {
                kind: LineKind::Diagonal, // kind は corners 計算に影響しない
                p0: *p0,
                p1: *p1,
                thickness: *thickness,
            };
            let pts = line.corners(0.0);
            scanline_fill_polygon(mask, &pts, w, h, value);
        }
        Shape::Rect {
            center,
            half_w,
            half_h,
            rotation_rad,
            ..
        } => {
            let pts = rect_corners(*center, *half_w, *half_h, *rotation_rad);
            scanline_fill_polygon(mask, &pts, w, h, value);
        }
        Shape::Ellipse {
            center,
            rx,
            ry,
            rotation_rad,
            ..
        } => {
            scanline_fill_ellipse(mask, *center, *rx, *ry, *rotation_rad, w, h, value);
        }
    }
}

/// `Shape` 群を既存の 1bit マスクに作成順で重ねる。
///
/// ビットマップマスクを下地にし、各 Shape の `op` に応じて `Add` は `true`、
/// `Subtract` は `false` を塗る。これにより、消去モードで作った矩形/楕円/線は
/// 既存 Shape を削除せず「上から削るオブジェクト」として振る舞う。
pub fn rasterize_shapes_into(mask: &mut [bool], shapes: &[Shape], w: usize, h: usize) {
    for s in shapes {
        let value = matches!(s.op(), ShapeOp::Add);
        rasterize_shape_into(mask, s, w, h, value);
    }
}

/// `Shape` 群を JSON 文字列にシリアライズする (空なら None)。
/// 出力は常に新タグ付き形式。
pub fn shapes_to_json(shapes: &[Shape]) -> Option<String> {
    if shapes.is_empty() {
        return None;
    }
    serde_json::to_string(shapes).ok()
}

/// JSON 文字列から `Shape` 群を読む。旧 `LineObject` 配列 / 新タグ付き配列 /
/// 混在配列いずれも受ける (個別要素の `Deserialize` で分岐)。
/// パース失敗時は空 Vec (既存 `vectors_from_json` と同じ寛容な挙動)。
pub fn shapes_from_json(s: &str) -> Vec<Shape> {
    serde_json::from_str(s).unwrap_or_default()
}

/// 円形ブラシで from → to を 1bit マスクへ塗る/消す。
///
/// ブラシ中心は画像外座標でもよく、円が画像範囲に重なる部分だけをクリップして処理する。
/// `true` を返すのは少なくとも 1 pixel の値が変わった場合。
pub fn paint_brush_line_bitmap(
    mask: &mut [bool],
    w: usize,
    h: usize,
    from: (f32, f32),
    to: (f32, f32),
    radius: f32,
    value: bool,
) -> bool {
    if w == 0 || h == 0 || mask.len() < w.saturating_mul(h) {
        return false;
    }
    let radius = radius.max(1.0);
    let dx = to.0 - from.0;
    let dy = to.1 - from.1;
    let dist = (dx * dx + dy * dy).sqrt();
    let steps = (dist / (radius * 0.5)).ceil().max(1.0) as usize;
    let mut changed = false;

    for step in 0..=steps {
        let t = step as f32 / steps as f32;
        let cx = from.0 + dx * t;
        let cy = from.1 + dy * t;

        let x0 = ((cx - radius).floor().max(0.0).min(w as f32)) as usize;
        let y0 = ((cy - radius).floor().max(0.0).min(h as f32)) as usize;
        let x1 = ((cx + radius).ceil().max(0.0).min(w as f32)) as usize;
        let y1 = ((cy + radius).ceil().max(0.0).min(h as f32)) as usize;
        if x0 >= x1 || y0 >= y1 {
            continue;
        }

        let r_sq = radius * radius;
        for py in y0..y1 {
            for px in x0..x1 {
                let ddx = px as f32 + 0.5 - cx;
                let ddy = py as f32 + 0.5 - cy;
                if ddx * ddx + ddy * ddy <= r_sq {
                    let idx = py * w + px;
                    if mask[idx] != value {
                        mask[idx] = value;
                        changed = true;
                    }
                }
            }
        }
    }

    changed
}

/// スキャンライン方式の多角形塗り。エラサーモードのビットマップ塗りと
/// ベクタラスタライズで共用する。`value=true` で塗り、`false` で消去。
pub fn scanline_fill_polygon(
    mask: &mut [bool],
    pts: &[(f32, f32)],
    w: usize,
    h: usize,
    value: bool,
) {
    if pts.len() < 3 {
        return;
    }
    let min_y = pts.iter().map(|p| p.1).fold(f32::MAX, f32::min).max(0.0) as usize;
    let max_y = pts
        .iter()
        .map(|p| p.1)
        .fold(f32::MIN, f32::max)
        .min(h as f32) as usize;
    let n = pts.len();
    let mut intersections = Vec::with_capacity(8);
    for y in min_y..max_y {
        let scan_y = y as f32 + 0.5;
        intersections.clear();
        for i in 0..n {
            let (x0, y0) = pts[i];
            let (x1, y1) = pts[(i + 1) % n];
            if (y0 <= scan_y && y1 > scan_y) || (y1 <= scan_y && y0 > scan_y) {
                let t = (scan_y - y0) / (y1 - y0);
                intersections.push(x0 + t * (x1 - x0));
            }
        }
        intersections.sort_by(|a, b| a.partial_cmp(b).unwrap());
        for pair in intersections.chunks(2) {
            if pair.len() == 2 {
                let px0 = (pair[0].max(0.0) as usize).min(w);
                let px1 = (pair[1].max(0.0).ceil() as usize).min(w);
                for px in px0..px1 {
                    mask[y * w + px] = value;
                }
            }
        }
    }
}

/// ベクタ群を JSON 文字列にシリアライズする。空なら None。
pub fn vectors_to_json(vectors: &[LineObject]) -> Option<String> {
    if vectors.is_empty() {
        return None;
    }
    serde_json::to_string(vectors).ok()
}

/// JSON 文字列からベクタ群をデシリアライズする。失敗時は空。
pub fn vectors_from_json(s: &str) -> Vec<LineObject> {
    serde_json::from_str(s).unwrap_or_default()
}

/// マスク永続化 DB。
pub struct MaskDb {
    conn: rusqlite::Connection,
}

impl MaskDb {
    pub fn open() -> Result<Self, rusqlite::Error> {
        Self::open_at(&Self::db_path())
    }

    /// 任意のパスで DB を開く。テスト・統合テスト用。
    pub fn open_at(path: &std::path::Path) -> Result<Self, rusqlite::Error> {
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let conn = rusqlite::Connection::open(path)?;
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS masks (
                path       TEXT PRIMARY KEY,
                mask_data  BLOB    NOT NULL,
                width      INTEGER NOT NULL,
                height     INTEGER NOT NULL,
                vectors    TEXT
            )",
        )?;
        // 既存 DB には vectors 列が無い可能性があるので ALTER で追加する。
        // 既に列があればエラーになるが無視する。
        let _ = conn.execute("ALTER TABLE masks ADD COLUMN vectors TEXT", []);
        Ok(Self { conn })
    }

    fn db_path() -> PathBuf {
        crate::data_dir::get().join("mask.db")
    }

    /// マスク (ビットマップのみ) を取得する。互換用。
    pub fn get(&self, key: &str, expected_w: usize, expected_h: usize) -> Option<Vec<bool>> {
        self.get_full(key, expected_w, expected_h).map(|(m, _)| m)
    }

    /// マスクとベクタ群をまとめて取得する (`Vec<Shape>` 形式)。
    /// 画像サイズが保存時と異なる場合 (PDF 再レンダリング等) はビットマップをリスケールし、
    /// ベクタ座標も比率で伸縮する (`Shape::scale_xy` 経由)。
    ///
    /// JSON 互換性: 旧版が保存した `Vec<LineObject>` JSON も `shapes_from_json` で
    /// `Vec<Shape::Line>` として読める。新版は常にタグ付き `Vec<Shape>` を書き戻す。
    pub fn get_full(
        &self,
        key: &str,
        expected_w: usize,
        expected_h: usize,
    ) -> Option<(Vec<bool>, Vec<Shape>)> {
        let mut stmt = self
            .conn
            .prepare_cached("SELECT mask_data, width, height, vectors FROM masks WHERE path = ?1")
            .ok()?;
        let (blob, w, h, vectors_json): (Vec<u8>, usize, usize, Option<String>) = stmt
            .query_row([key], |row| {
                Ok((
                    row.get::<_, Vec<u8>>(0)?,
                    row.get::<_, i64>(1)? as usize,
                    row.get::<_, i64>(2)? as usize,
                    row.get::<_, Option<String>>(3)?,
                ))
            })
            .ok()?;

        let mut mask = decompress_mask(&blob, w, h)?;
        let mut shapes = vectors_json
            .as_deref()
            .map(shapes_from_json)
            .unwrap_or_default();

        if w != expected_w || h != expected_h {
            mask = rescale_mask(&mask, w, h, expected_w, expected_h);
            let sx = expected_w as f32 / w.max(1) as f32;
            let sy = expected_h as f32 / h.max(1) as f32;
            for s in &mut shapes {
                s.scale_xy(sx, sy);
            }
        }
        Some((mask, shapes))
    }

    /// マスク＋ベクタを保存する。ビットマップが全 false でベクタも空なら削除する。
    pub fn set(
        &self,
        key: &str,
        mask: &[bool],
        shapes: &[Shape],
        w: usize,
        h: usize,
    ) -> rusqlite::Result<()> {
        let bitmap_empty = !mask.iter().any(|&m| m);
        if bitmap_empty && shapes.is_empty() {
            return self.delete(key);
        }
        self.upsert_mask(key, mask, shapes, w, h)
    }

    /// マスクを削除する。
    pub fn delete(&self, key: &str) -> rusqlite::Result<()> {
        self.conn
            .execute("DELETE FROM masks WHERE path = ?1", [key])?;
        Ok(())
    }

    /// 名前付きスロットにマスクを保存する。`set` と異なりビットマップ全 false でも保存する。
    pub fn set_slot(
        &self,
        slot: usize,
        mask: &[bool],
        shapes: &[Shape],
        w: usize,
        h: usize,
    ) -> rusqlite::Result<()> {
        self.upsert_mask(&slot_key(slot), mask, shapes, w, h)
    }

    /// 既に 1bit/pixel + deflate 圧縮済みの生バイト列を直接保存する。
    /// サイドカー (mimageviewer.dat) からのインポート時に使用する (再圧縮を避ける)。
    pub fn set_raw(
        &self,
        key: &str,
        compressed: &[u8],
        vectors_json: Option<&str>,
        w: usize,
        h: usize,
    ) -> rusqlite::Result<()> {
        self.conn.execute(
            "INSERT INTO masks (path, mask_data, width, height, vectors)
             VALUES (?1, ?2, ?3, ?4, ?5)
             ON CONFLICT(path) DO UPDATE SET mask_data = ?2, width = ?3, height = ?4, vectors = ?5",
            rusqlite::params![key, compressed, w as i64, h as i64, vectors_json],
        )?;
        Ok(())
    }

    /// 名前付きスロットからマスク (ビットマップのみ) を取得する。互換用。
    pub fn get_slot(&self, slot: usize, expected_w: usize, expected_h: usize) -> Option<Vec<bool>> {
        self.get(&slot_key(slot), expected_w, expected_h)
    }

    /// 名前付きスロットからマスクとベクタ群を取得する。
    pub fn get_slot_full(
        &self,
        slot: usize,
        expected_w: usize,
        expected_h: usize,
    ) -> Option<(Vec<bool>, Vec<Shape>)> {
        self.get_full(&slot_key(slot), expected_w, expected_h)
    }

    /// スロットの元のサイズ (width, height) を返す。存在しなければ None。
    /// 一括適用で元サイズのままデータを配る場合に使う。
    pub fn slot_size(&self, slot: usize) -> Option<(usize, usize)> {
        let mut stmt = self
            .conn
            .prepare_cached("SELECT width, height FROM masks WHERE path = ?1")
            .ok()?;
        stmt.query_row([slot_key(slot)], |row| {
            Ok((
                row.get::<_, i64>(0)? as usize,
                row.get::<_, i64>(1)? as usize,
            ))
        })
        .ok()
    }

    /// 指定プレフィックスで始まるパスを持つマスクエントリのキー集合を返す。
    /// フォルダ単位の「このフォルダ内でマスクを持つページ」列挙に使う。
    /// スロットキー (`__slot_*`) は除外する。
    pub fn load_mask_keys(&self, prefix: &str) -> std::collections::HashSet<String> {
        let mut set = std::collections::HashSet::new();
        let Ok(mut stmt) = self.conn.prepare_cached(
            "SELECT path FROM masks WHERE path LIKE ?1 ESCAPE '\\' AND path NOT LIKE '\\_\\_slot\\_%' ESCAPE '\\'"
        ) else {
            return set;
        };
        let escaped = prefix
            .replace('\\', "\\\\")
            .replace('%', "\\%")
            .replace('_', "\\_")
            .replace('[', "\\[");
        let pattern = format!("{escaped}%");
        let Ok(rows) = stmt.query_map([&pattern], |row| row.get::<_, String>(0)) else {
            return set;
        };
        for r in rows.flatten() {
            set.insert(r);
        }
        set
    }

    fn upsert_mask(
        &self,
        key: &str,
        mask: &[bool],
        shapes: &[Shape],
        w: usize,
        h: usize,
    ) -> rusqlite::Result<()> {
        let blob = compress_mask(mask);
        let shapes_json = shapes_to_json(shapes);
        self.conn.execute(
            "INSERT INTO masks (path, mask_data, width, height, vectors)
             VALUES (?1, ?2, ?3, ?4, ?5)
             ON CONFLICT(path) DO UPDATE SET mask_data = ?2, width = ?3, height = ?4, vectors = ?5",
            rusqlite::params![key, blob, w as i64, h as i64, shapes_json],
        )?;
        Ok(())
    }
}

fn slot_key(slot: usize) -> String {
    format!("__slot_{}", slot)
}

/// マスク (Vec<bool>) を 1bit/pixel にパックし deflate 圧縮する。
pub fn compress_mask(mask: &[bool]) -> Vec<u8> {
    // 1bit/pixel にパック
    let byte_count = (mask.len() + 7) / 8;
    let mut packed = vec![0u8; byte_count];
    for (i, &m) in mask.iter().enumerate() {
        if m {
            packed[i / 8] |= 1 << (7 - (i % 8));
        }
    }

    // deflate 圧縮
    let mut encoder = DeflateEncoder::new(Vec::new(), Compression::default());
    encoder.write_all(&packed).unwrap_or_default();
    encoder.finish().unwrap_or_default()
}

/// deflate 展開して 1bit/pixel をアンパックする。
fn decompress_mask(blob: &[u8], w: usize, h: usize) -> Option<Vec<bool>> {
    let total = w * h;
    let byte_count = (total + 7) / 8;

    let mut decoder = DeflateDecoder::new(blob);
    let mut packed = Vec::new();
    decoder.read_to_end(&mut packed).ok()?;

    if packed.len() < byte_count {
        return None;
    }

    let mut mask = vec![false; total];
    for i in 0..total {
        if packed[i / 8] & (1 << (7 - (i % 8))) != 0 {
            mask[i] = true;
        }
    }
    Some(mask)
}

/// マスクを最近傍法でリスケールする。
pub fn rescale_mask(
    src: &[bool],
    src_w: usize,
    src_h: usize,
    dst_w: usize,
    dst_h: usize,
) -> Vec<bool> {
    let mut dst = vec![false; dst_w * dst_h];
    let x_ratio = src_w as f32 / dst_w as f32;
    let y_ratio = src_h as f32 / dst_h as f32;
    for dy in 0..dst_h {
        let sy = ((dy as f32 * y_ratio) as usize).min(src_h.saturating_sub(1));
        for dx in 0..dst_w {
            let sx = ((dx as f32 * x_ratio) as usize).min(src_w.saturating_sub(1));
            dst[dy * dst_w + dx] = src[sy * src_w + sx];
        }
    }
    dst
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_compress() {
        let mut mask = vec![false; 1000];
        mask[10] = true;
        mask[100] = true;
        mask[999] = true;

        let compressed = compress_mask(&mask);
        let decompressed = decompress_mask(&compressed, 100, 10).unwrap();
        assert_eq!(mask, decompressed);
    }

    #[test]
    fn empty_mask_compresses() {
        let mask = vec![false; 5000];
        let compressed = compress_mask(&mask);
        assert!(
            compressed.len() < 50,
            "empty mask should compress well: {} bytes",
            compressed.len()
        );
    }

    #[test]
    fn full_mask_roundtrip() {
        let mask = vec![true; 512 * 512];
        let compressed = compress_mask(&mask);
        let decompressed = decompress_mask(&compressed, 512, 512).unwrap();
        assert_eq!(mask, decompressed);
    }

    #[test]
    fn vector_rasterize_and_serialize() {
        let v = LineObject {
            kind: LineKind::Diagonal,
            p0: (10.0, 10.0),
            p1: (90.0, 10.0),
            thickness: 4.0,
        };
        let json = vectors_to_json(&[v]).unwrap();
        let back = vectors_from_json(&json);
        assert_eq!(back.len(), 1);

        let mut mask = vec![false; 100 * 20];
        rasterize_vectors_into(&mut mask, &[v], 100, 20);
        // 中心軸 y=10, thickness=4 → y=8..12 の範囲で x=10..90 が塗られているはず
        assert!(mask[10 * 100 + 50]);
        assert!(!mask[50]); // y=0 行 (x=50) は塗られない
    }

    #[test]
    fn paint_brush_line_bitmap_paints_when_center_is_outside_image() {
        let mut mask = vec![false; 10 * 10];

        let changed =
            paint_brush_line_bitmap(&mut mask, 10, 10, (-1.5, 5.0), (-1.5, 5.0), 3.0, true);

        assert!(changed);
        assert!(mask[5 * 10], "left edge should be painted");
        assert!(!mask[5 * 10 + 5], "far pixels should stay untouched");
    }

    #[test]
    fn paint_brush_line_bitmap_ignores_non_overlapping_outside_brush() {
        let mut mask = vec![false; 10 * 10];

        let changed =
            paint_brush_line_bitmap(&mut mask, 10, 10, (-10.0, 5.0), (-10.0, 5.0), 3.0, true);

        assert!(!changed);
        assert!(mask.iter().all(|&v| !v));
    }

    #[test]
    fn scanline_fill_polygon_clips_points_outside_image() {
        let mut mask = vec![false; 10 * 10];
        let points = [(-5.0, -5.0), (5.0, -5.0), (5.0, 5.0), (-5.0, 5.0)];

        scanline_fill_polygon(&mut mask, &points, 10, 10, true);

        assert!(mask[0], "top-left clipped area should be filled");
        assert!(mask[4 * 10 + 4], "inside clipped polygon should be filled");
        assert!(!mask[5 * 10 + 4], "rows below polygon should stay clear");
        assert!(
            !mask[4 * 10 + 5],
            "columns outside polygon should stay clear"
        );
    }

    #[test]
    fn empty_vectors_serialize_to_none() {
        assert!(vectors_to_json(&[]).is_none());
    }

    // ── Shape enum / マイグレーション関連テスト (Phase 0、Codex P1 反映) ─

    #[test]
    fn shape_line_tagged_roundtrip() {
        let s = Shape::Line {
            op: ShapeOp::Add,
            kind: LineKind::Diagonal,
            p0: (10.0, 20.0),
            p1: (100.0, 200.0),
            thickness: 4.0,
        };
        let json = shapes_to_json(&[s]).unwrap();
        // タグ付き形式で出ているか
        assert!(json.contains("\"type\":\"line\""));
        assert!(
            !json.contains("\"op\""),
            "default add op should stay omitted for compact/back-compatible JSON: {json}"
        );
        let back = shapes_from_json(&json);
        assert_eq!(back.len(), 1);
        assert_eq!(back[0], s);
    }

    #[test]
    fn shape_rect_tagged_roundtrip() {
        let s = Shape::Rect {
            op: ShapeOp::Add,
            center: (50.0, 60.0),
            half_w: 20.0,
            half_h: 10.0,
            rotation_rad: 0.5,
        };
        let json = shapes_to_json(&[s]).unwrap();
        assert!(json.contains("\"type\":\"rect\""));
        let back = shapes_from_json(&json);
        assert_eq!(back, vec![s]);
    }

    #[test]
    fn shape_ellipse_tagged_roundtrip() {
        let s = Shape::Ellipse {
            op: ShapeOp::Add,
            center: (100.0, 100.0),
            rx: 30.0,
            ry: 15.0,
            rotation_rad: 0.0,
        };
        let json = shapes_to_json(&[s]).unwrap();
        assert!(json.contains("\"type\":\"ellipse\""));
        let back = shapes_from_json(&json);
        assert_eq!(back, vec![s]);
    }

    #[test]
    fn shape_op_defaults_to_add_when_missing() {
        let json = r#"[{"type":"rect","center":[50.0,50.0],"half_w":10.0,"half_h":5.0,"rotation_rad":0.0}]"#;
        let shapes = shapes_from_json(json);
        assert_eq!(shapes.len(), 1);
        assert_eq!(shapes[0].op(), ShapeOp::Add);
    }

    #[test]
    fn shape_subtract_tagged_roundtrip() {
        let s = Shape::Rect {
            op: ShapeOp::Subtract,
            center: (50.0, 60.0),
            half_w: 20.0,
            half_h: 10.0,
            rotation_rad: 0.5,
        };
        let json = shapes_to_json(&[s]).unwrap();
        assert!(json.contains("\"op\":\"subtract\""));
        let back = shapes_from_json(&json);
        assert_eq!(back, vec![s]);
    }

    #[test]
    fn shape_deser_legacy_line_object_json() {
        // 旧 LineObject の素 JSON。リリース済みの DB / サイドカーに残っている形式。
        let legacy = r#"[{"kind":"diag","p0":[10.0,20.0],"p1":[100.0,200.0],"thickness":4.0}]"#;
        let shapes = shapes_from_json(legacy);
        assert_eq!(shapes.len(), 1);
        assert_eq!(
            shapes[0],
            Shape::Line {
                op: ShapeOp::Add,
                kind: LineKind::Diagonal,
                p0: (10.0, 20.0),
                p1: (100.0, 200.0),
                thickness: 4.0,
            }
        );
    }

    #[test]
    fn shape_deser_legacy_vert_and_horiz() {
        // 縦線 / 横線も `kind` で区別された旧形式を保持
        let legacy = r#"[
            {"kind":"vert","p0":[5.0,0.0],"p1":[5.0,100.0],"thickness":2.0},
            {"kind":"horiz","p0":[0.0,5.0],"p1":[100.0,5.0],"thickness":3.0}
        ]"#;
        let shapes = shapes_from_json(legacy);
        assert_eq!(shapes.len(), 2);
        match shapes[0] {
            Shape::Line {
                kind: LineKind::Vertical,
                ..
            } => {}
            _ => panic!("expected vertical line, got {:?}", shapes[0]),
        }
        match shapes[1] {
            Shape::Line {
                kind: LineKind::Horizontal,
                ..
            } => {}
            _ => panic!("expected horizontal line, got {:?}", shapes[1]),
        }
    }

    #[test]
    fn shape_deser_mixed_legacy_and_tagged() {
        // 1 つの配列に旧 LineObject (タグなし) と新 Shape (タグ付き) が混在しても読める。
        let mixed = r#"[
            {"kind":"diag","p0":[0.0,0.0],"p1":[10.0,10.0],"thickness":1.0},
            {"type":"rect","center":[50.0,50.0],"half_w":10.0,"half_h":5.0,"rotation_rad":0.0},
            {"type":"line","kind":"vert","p0":[20.0,0.0],"p1":[20.0,30.0],"thickness":2.0},
            {"type":"ellipse","center":[100.0,100.0],"rx":15.0,"ry":10.0,"rotation_rad":0.0}
        ]"#;
        let shapes = shapes_from_json(mixed);
        assert_eq!(shapes.len(), 4);
        assert!(matches!(shapes[0], Shape::Line { .. }));
        assert!(matches!(shapes[1], Shape::Rect { .. }));
        assert!(matches!(shapes[2], Shape::Line { .. }));
        assert!(matches!(shapes[3], Shape::Ellipse { .. }));
    }

    #[test]
    fn shape_deser_empty_array() {
        let shapes = shapes_from_json("[]");
        assert!(shapes.is_empty());
    }

    #[test]
    fn shape_deser_unknown_type_is_error() {
        // 未知の "type" は legacy にフォールバックせず、その要素は配列ごと parse 失敗
        // (Codex P1: silently fallback してはいけない)
        let bad = r#"[{"type":"diamond","center":[0,0],"half_w":1,"half_h":1,"rotation_rad":0}]"#;
        let parse_result: Result<Vec<Shape>, _> = serde_json::from_str(bad);
        assert!(
            parse_result.is_err(),
            "unknown 'type' should be an error, got: {:?}",
            parse_result
        );
    }

    #[test]
    fn shape_deser_tagged_with_missing_field_is_error() {
        // タグ付きで必須フィールド欠落 → legacy にフォールバックせずエラー
        let bad = r#"[{"type":"line","kind":"diag","p0":[0,0]}]"#; // p1 / thickness 欠落
        let parse_result: Result<Vec<Shape>, _> = serde_json::from_str(bad);
        assert!(
            parse_result.is_err(),
            "tagged with missing field should be error, got: {:?}",
            parse_result
        );
    }

    #[test]
    fn shape_deser_legacy_extra_fields_ignored() {
        // 将来追加されたフィールドが旧 JSON にあっても無視されて読める
        let with_extra =
            r#"[{"kind":"diag","p0":[0,0],"p1":[10,10],"thickness":2.0,"future_field":42}]"#;
        let shapes = shapes_from_json(with_extra);
        assert_eq!(shapes.len(), 1);
        assert!(matches!(shapes[0], Shape::Line { .. }));
    }

    #[test]
    fn shape_deser_lenient_fallback_on_corrupt() {
        // `shapes_from_json` は寛容に空配列を返す (既存 vectors_from_json と同じ挙動)。
        // 厳密エラーが必要な場合は `serde_json::from_str::<Vec<Shape>>` を直接使う。
        let corrupt = r#"this is not json"#;
        let shapes = shapes_from_json(corrupt);
        assert!(shapes.is_empty());
    }

    #[test]
    fn rect_corners_axis_aligned() {
        let pts = rect_corners((100.0, 100.0), 10.0, 20.0, 0.0);
        // NW, NE, SE, SW
        assert_eq!(pts[0], (90.0, 80.0));
        assert_eq!(pts[1], (110.0, 80.0));
        assert_eq!(pts[2], (110.0, 120.0));
        assert_eq!(pts[3], (90.0, 120.0));
    }

    #[test]
    fn rect_corners_rotated_90deg() {
        let pts = rect_corners((0.0, 0.0), 10.0, 5.0, std::f32::consts::FRAC_PI_2);
        // 90° 回転: x, y が入れ替わる (符号は座標系依存)
        // (-10, -5) -> (5, -10)
        // (+10, -5) -> (5, +10)
        // (+10, +5) -> (-5, +10)
        // (-10, +5) -> (-5, -10)
        let eps = 1e-4;
        assert!((pts[0].0 - 5.0).abs() < eps);
        assert!((pts[0].1 - (-10.0)).abs() < eps);
        assert!((pts[2].0 - (-5.0)).abs() < eps);
        assert!((pts[2].1 - 10.0).abs() < eps);
    }

    #[test]
    fn rasterize_shape_rect_axis_aligned() {
        // 100x100 マスクに中心 (50, 50)、half=10×5、回転 0 の矩形を塗る
        let mut mask = vec![false; 100 * 100];
        let shape = Shape::Rect {
            op: ShapeOp::Add,
            center: (50.0, 50.0),
            half_w: 10.0,
            half_h: 5.0,
            rotation_rad: 0.0,
        };
        rasterize_shape_into(&mut mask, &shape, 100, 100, true);
        // 中心が塗られている
        assert!(mask[50 * 100 + 50]);
        // 端 (40, 50) と (59, 50) の内側 (half_w=10 なので画素 40..=59 が塗られる想定)
        assert!(mask[50 * 100 + 41]);
        assert!(mask[50 * 100 + 58]);
        // 外: x=30 / x=70
        assert!(!mask[50 * 100 + 30]);
        assert!(!mask[50 * 100 + 70]);
        // 外: y=30 / y=70
        assert!(!mask[30 * 100 + 50]);
        assert!(!mask[70 * 100 + 50]);
    }

    #[test]
    fn rasterize_shape_ellipse_axis_aligned() {
        // 200x200 マスクに中心 (100, 100)、rx=50、ry=30、回転 0 の楕円
        let mut mask = vec![false; 200 * 200];
        let shape = Shape::Ellipse {
            op: ShapeOp::Add,
            center: (100.0, 100.0),
            rx: 50.0,
            ry: 30.0,
            rotation_rad: 0.0,
        };
        rasterize_shape_into(&mut mask, &shape, 200, 200, true);
        // 中心は塗られる
        assert!(mask[100 * 200 + 100]);
        // 短軸方向の縁 (rx=50): 内側 x=140 は塗られる、外側 x=160 は塗られない
        assert!(mask[100 * 200 + 140]);
        assert!(!mask[100 * 200 + 160]);
        // 長軸 (ry=30): 内側 y=120 は塗られる、外側 y=140 は塗られない
        assert!(mask[120 * 200 + 100]);
        assert!(!mask[140 * 200 + 100]);
        // 楕円の対角 (短軸/長軸の外側コーナー): (140, 120) は楕円外
        // u²/2500 + v²/900 = 1600/2500 + 400/900 = 0.64 + 0.444 = 1.08 > 1
        assert!(!mask[120 * 200 + 140]);
    }

    #[test]
    fn rasterize_shape_ellipse_rotated_90() {
        // 90° 回転で rx と ry が入れ替わったように見える
        let mut mask = vec![false; 200 * 200];
        let shape = Shape::Ellipse {
            op: ShapeOp::Add,
            center: (100.0, 100.0),
            rx: 50.0,
            ry: 30.0,
            rotation_rad: std::f32::consts::FRAC_PI_2,
        };
        rasterize_shape_into(&mut mask, &shape, 200, 200, true);
        // 90° 回転後は、見かけ上 rx=30 (横方向)、ry=50 (縦方向) になる
        assert!(mask[100 * 200 + 100]); // 中心
        assert!(mask[100 * 200 + 125]); // 横 25 < 30 内
        assert!(!mask[100 * 200 + 140]); // 横 40 > 30 外
        assert!(mask[140 * 200 + 100]); // 縦 40 < 50 内
        assert!(!mask[160 * 200 + 100]); // 縦 60 > 50 外
    }

    #[test]
    fn rasterize_shape_rect_rotated_45() {
        // 45° 回転で対角線方向に細長くなる
        let mut mask = vec![false; 100 * 100];
        let shape = Shape::Rect {
            op: ShapeOp::Add,
            center: (50.0, 50.0),
            half_w: 20.0,
            half_h: 5.0,
            rotation_rad: std::f32::consts::FRAC_PI_4,
        };
        rasterize_shape_into(&mut mask, &shape, 100, 100, true);
        // 中心は塗られている
        assert!(mask[50 * 100 + 50]);
        // 主対角線方向 (x+y 増加) には伸びる: (60, 60) はまだ内側
        assert!(mask[60 * 100 + 60]);
        // 副対角線方向 (x+y 一定 = 100 だが y-x 増加) には伸びない:
        // (40, 60) は中心からの local 座標 (-10, 10) → 回転後 u = c*(-10) + s*10 = 0,
        // v = -s*(-10) + c*10 = 7.07 + 7.07 = 14.14 > 5 なので外
        assert!(!mask[60 * 100 + 40]);
    }

    #[test]
    fn rasterize_shapes_apply_op_in_creation_order() {
        let mut mask = vec![false; 40 * 40];
        let shapes = vec![
            Shape::Rect {
                op: ShapeOp::Add,
                center: (20.0, 20.0),
                half_w: 12.0,
                half_h: 12.0,
                rotation_rad: 0.0,
            },
            Shape::Rect {
                op: ShapeOp::Subtract,
                center: (20.0, 20.0),
                half_w: 4.0,
                half_h: 4.0,
                rotation_rad: 0.0,
            },
            Shape::Ellipse {
                op: ShapeOp::Add,
                center: (20.0, 20.0),
                rx: 2.0,
                ry: 2.0,
                rotation_rad: 0.0,
            },
        ];
        rasterize_shapes_into(&mut mask, &shapes, 40, 40);

        assert!(mask[20 * 40 + 10], "first add shape should paint");
        assert!(
            !mask[20 * 40 + 16],
            "subtract shape should cut the add shape"
        );
        assert!(
            mask[20 * 40 + 20],
            "later add shape should paint over subtract"
        );
    }

    #[test]
    fn shape_translate_rotate_compose() {
        let mut s = Shape::Rect {
            op: ShapeOp::Add,
            center: (50.0, 50.0),
            half_w: 10.0,
            half_h: 5.0,
            rotation_rad: 0.0,
        };
        s.translate(10.0, -20.0);
        match s {
            Shape::Rect { center, .. } => assert_eq!(center, (60.0, 30.0)),
            _ => panic!(),
        }
        s.rotate_around(60.0, 30.0, std::f32::consts::FRAC_PI_2);
        match s {
            Shape::Rect {
                center,
                rotation_rad,
                ..
            } => {
                assert!((center.0 - 60.0).abs() < 1e-4);
                assert!((center.1 - 30.0).abs() < 1e-4);
                assert!((rotation_rad - std::f32::consts::FRAC_PI_2).abs() < 1e-4);
            }
            _ => panic!(),
        }
    }

    #[test]
    fn shape_scale_isotropic() {
        let mut s = Shape::Rect {
            op: ShapeOp::Add,
            center: (100.0, 100.0),
            half_w: 20.0,
            half_h: 10.0,
            rotation_rad: 0.0,
        };
        s.scale_xy(2.0, 2.0);
        match s {
            Shape::Rect {
                center,
                half_w,
                half_h,
                ..
            } => {
                assert_eq!(center, (200.0, 200.0));
                assert_eq!(half_w, 40.0);
                assert_eq!(half_h, 20.0);
            }
            _ => panic!(),
        }
    }

    #[test]
    fn shape_from_line_object_roundtrip() {
        let line = LineObject {
            kind: LineKind::Diagonal,
            p0: (1.0, 2.0),
            p1: (3.0, 4.0),
            thickness: 5.0,
        };
        let shape: Shape = line.into();
        let back = shape.as_legacy_line().unwrap();
        assert_eq!(back, line);
    }

    #[test]
    fn shape_as_legacy_line_returns_none_for_rect_ellipse() {
        let r = Shape::Rect {
            op: ShapeOp::Add,
            center: (0.0, 0.0),
            half_w: 1.0,
            half_h: 1.0,
            rotation_rad: 0.0,
        };
        assert!(r.as_legacy_line().is_none());
        let e = Shape::Ellipse {
            op: ShapeOp::Add,
            center: (0.0, 0.0),
            rx: 1.0,
            ry: 1.0,
            rotation_rad: 0.0,
        };
        assert!(e.as_legacy_line().is_none());
    }

    #[test]
    fn shapes_to_json_empty_returns_none() {
        assert!(shapes_to_json(&[]).is_none());
    }

    #[test]
    fn set_raw_preserves_legacy_string_then_read_as_shape() {
        // サイドカー復元経路: 旧 LineObject の素 JSON を set_raw でそのまま DB に貼る。
        // get_full は LineObject を返すが、新しい shapes_from_json でも読めることを確認。
        let tmp = std::env::temp_dir().join(format!(
            "mimageviewer_mask_db_set_raw_legacy_{}.db",
            std::process::id()
        ));
        let _ = std::fs::remove_file(&tmp);
        let db = MaskDb::open_at(&tmp).expect("open db");
        let w = 100;
        let h = 100;
        // 1x1 = true なダミーマスク
        let mut mask = vec![false; w * h];
        mask[0] = true;
        let compressed = compress_mask(&mask);
        let legacy_json = r#"[{"kind":"diag","p0":[10.0,20.0],"p1":[80.0,20.0],"thickness":4.0}]"#;
        db.set_raw("legacy_key", &compressed, Some(legacy_json), w, h)
            .expect("set_raw");
        // 既存 API (vectors_from_json → Vec<LineObject>) でも読める
        let (got_mask, got_legacy) = db.get_full("legacy_key", w, h).expect("get_full");
        assert_eq!(got_mask, mask);
        assert_eq!(got_legacy.len(), 1);
        // 同じ JSON を shapes_from_json で読むと Shape::Line として復元される
        let got_shapes = shapes_from_json(legacy_json);
        assert_eq!(got_shapes.len(), 1);
        assert!(matches!(got_shapes[0], Shape::Line { .. }));
        let _ = std::fs::remove_file(&tmp);
    }
}
