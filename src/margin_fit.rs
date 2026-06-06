//! 「余白カットフィット」用の中身 bounding box 検出。
//!
//! スキャン系画像 (自炊漫画など) の白/黒一色の余白を、**画像を加工せず**表示時に
//! 詰めて拡大表示するための矩形を求める。返すのは正規化座標 (0..1) の Rect で、
//! 描画側 (`draw_fs_image`) はこの矩形をウィンドウにフィットさせる (= 余白分ズームインして
//! 再センタリングする) だけ。ピクセルは一切変えないので補正/AI/キャッシュ/エクスポート
//! パイプラインへ影響しない。
//!
//! 余白が一様でない (フルブリードの絵 / グラデ背景 / テクスチャ余白) 場合は検出を諦めて
//! `None` を返し、呼び出し側は通常フィットにフォールバックする (= 中身を切らない安全側)。

use egui::{Color32, ColorImage, Rect, pos2};

/// 余白判定の色一致許容差 (per-channel, 0..255)。固定値。実機で調整要望が出たら設定昇格する。
pub const DEFAULT_TOLERANCE: u8 = 16;

/// 行/列を「余白」とみなす上限非余白割合。スキャンのゴミ・JPEG ノイズを許容する。
const NONMARGIN_ROW_FRACTION: f32 = 0.01;

/// トリム後がこの割合未満しか縮まないなら「余白なし」とみなして None (無駄なズーム抑止)。
const MIN_TRIM_FRACTION: f32 = 0.01;

#[inline]
fn within_tol(a: Color32, b: Color32, tol: u8) -> bool {
    let d = |x: u8, y: u8| x.abs_diff(y) <= tol;
    d(a.r(), b.r()) && d(a.g(), b.g()) && d(a.b(), b.b())
}

/// 画像の中身 (非余白) の bounding box を正規化座標 (0..1) で返す。
///
/// - 四隅の色が互いに `tol` 以内で揃っているときだけ「余白色」を確定する (= 一様余白)。
/// - 各辺から内側へ、行/列の非余白割合が `NONMARGIN_ROW_FRACTION` 以下の間だけ詰める。
/// - 中身が画像全体とほぼ同じ (どの辺も `MIN_TRIM_FRACTION` 未満) なら `None`。
pub fn detect_content_bbox(img: &ColorImage, tol: u8) -> Option<Rect> {
    let [w, h] = img.size;
    if w < 8 || h < 8 {
        return None;
    }
    let px = &img.pixels;
    let at = |x: usize, y: usize| px[y * w + x];

    // 四隅サンプルで余白色を確定 (一様でなければ諦める)。
    let c0 = at(0, 0);
    let c1 = at(w - 1, 0);
    let c2 = at(0, h - 1);
    let c3 = at(w - 1, h - 1);
    if !(within_tol(c0, c1, tol) && within_tol(c0, c2, tol) && within_tol(c0, c3, tol)) {
        return None;
    }
    let margin = c0;
    let max_nonmargin_row = ((w as f32) * NONMARGIN_ROW_FRACTION) as usize;
    let max_nonmargin_col = ((h as f32) * NONMARGIN_ROW_FRACTION) as usize;

    let row_is_margin = |y: usize| -> bool {
        let mut n = 0usize;
        for x in 0..w {
            if !within_tol(at(x, y), margin, tol) {
                n += 1;
                if n > max_nonmargin_row {
                    return false;
                }
            }
        }
        true
    };

    // top / bottom
    let mut top = 0usize;
    while top < h && row_is_margin(top) {
        top += 1;
    }
    if top >= h {
        return None; // 全面が余白色 (真っ白/真っ黒) → 切らない
    }
    let mut bottom = h - 1;
    while bottom > top && row_is_margin(bottom) {
        bottom -= 1;
    }

    // 中身の行範囲 [top, bottom] 内だけで列を判定する。
    let col_is_margin = |x: usize| -> bool {
        let mut n = 0usize;
        for y in top..=bottom {
            if !within_tol(at(x, y), margin, tol) {
                n += 1;
                if n > max_nonmargin_col {
                    return false;
                }
            }
        }
        true
    };
    let mut left = 0usize;
    while left < w && col_is_margin(left) {
        left += 1;
    }
    let mut right = w - 1;
    while right > left && col_is_margin(right) {
        right -= 1;
    }

    // トリム量が小さすぎるなら余白なし扱い。
    let trimmed = top + (h - 1 - bottom) + left + (w - 1 - right);
    let min_trim = (((w + h) as f32) * MIN_TRIM_FRACTION) as usize;
    if trimmed < min_trim.max(1) {
        return None;
    }

    // 包含のため右/下は +1 して [left, top] .. [right+1, bottom+1] にする。
    let nx0 = left as f32 / w as f32;
    let ny0 = top as f32 / h as f32;
    let nx1 = (right + 1) as f32 / w as f32;
    let ny1 = (bottom + 1) as f32 / h as f32;
    Some(Rect::from_min_max(pos2(nx0, ny0), pos2(nx1, ny1)))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn solid(w: usize, h: usize, c: Color32) -> ColorImage {
        ColorImage::new([w, h], vec![c; w * h])
    }

    #[test]
    fn no_margin_returns_none() {
        // 全面同色 → 切らない
        let img = solid(32, 32, Color32::WHITE);
        assert!(detect_content_bbox(&img, DEFAULT_TOLERANCE).is_none());
    }

    #[test]
    fn centered_block_is_detected() {
        // 白地の中央 (8..24, 8..24) に黒ブロックを置く 32x32
        let w = 32;
        let h = 32;
        let mut img = solid(w, h, Color32::WHITE);
        for y in 8..24 {
            for x in 8..24 {
                img.pixels[y * w + x] = Color32::BLACK;
            }
        }
        let bbox = detect_content_bbox(&img, DEFAULT_TOLERANCE).expect("中身を検出");
        // 左上 (8/32=0.25, 8/32=0.25), 右下 ((23+1)/32=0.75)
        assert!((bbox.min.x - 0.25).abs() < 1e-6, "left {}", bbox.min.x);
        assert!((bbox.min.y - 0.25).abs() < 1e-6, "top {}", bbox.min.y);
        assert!((bbox.max.x - 0.75).abs() < 1e-6, "right {}", bbox.max.x);
        assert!((bbox.max.y - 0.75).abs() < 1e-6, "bottom {}", bbox.max.y);
    }

    #[test]
    fn non_uniform_corners_returns_none() {
        // 四隅がバラバラ (フルブリード相当) → 諦める
        let w = 32;
        let h = 32;
        let mut img = solid(w, h, Color32::WHITE);
        img.pixels[0] = Color32::from_rgb(10, 200, 30);
        assert!(detect_content_bbox(&img, DEFAULT_TOLERANCE).is_none());
    }

    #[test]
    fn black_margin_detected() {
        // 黒余白 + 中央に白ブロック
        let w = 40;
        let h = 40;
        let mut img = solid(w, h, Color32::BLACK);
        for y in 10..30 {
            for x in 5..35 {
                img.pixels[y * w + x] = Color32::WHITE;
            }
        }
        let bbox = detect_content_bbox(&img, DEFAULT_TOLERANCE).expect("黒余白でも検出");
        assert!((bbox.min.x - 5.0 / 40.0).abs() < 1e-6);
        assert!((bbox.min.y - 10.0 / 40.0).abs() < 1e-6);
        assert!((bbox.max.x - 35.0 / 40.0).abs() < 1e-6);
        assert!((bbox.max.y - 30.0 / 40.0).abs() < 1e-6);
    }
}
