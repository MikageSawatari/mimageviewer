//! 「余白カットフィット」用の中身 bounding box 検出。
//!
//! スキャン画像 (自炊漫画など) の白/黒一色の余白を、**画像を加工せず**表示時に
//! 詰めて中身を拡大表示するための矩形を求める。返すのは正規化座標 (0..1) の Rect で、
//! 描画側 (`draw_fs_image`) はこの矩形をウィンドウにフィットさせる (= 余白分ズームインして
//! 再センタリングする) だけ。ピクセルは一切変えないので補正/AI/キャッシュ/エクスポート
//! パイプラインへ影響しない。
//!
//! ## 設計方針 (「全部映る」を最優先)
//!
//! 実スキャンはノド (綴じ side) の糊汚れ・端のゴミ・スキャナの明るさ勾配が普通に入る。
//! 「中身を切らない」を最優先に、次の段で頑健化している:
//!
//! 1. **検出用に縮小** (長辺 `DETECT_LONG_SIDE` へ面積平均): サブピクセルのノイズを潰し、
//!    細い線は連続なので残す。検出も速くなる。
//! 2. **余白色を縁の median で推定**: 四隅 1 点ではなく上下左右の縁ピクセルの中央値を取り、
//!    1 隅のゴミ・勾配に強くする。
//! 3. **広めの白許容** (`tol`, luma 距離): 色味の細かい差・薄いゴミを余白扱いにする (luma で
//!    判定するので色相差は無視)。
//! 4. **連結成分で「点 vs 線」を判定**: 中身マスクを 8 連結でラベリングし、面積が
//!    `MIN_COMPONENT_AREA` 未満の孤立塊 (= 点/ゴミ) は捨てる。線・文字・本文ブロックは残る。
//!    枠外へはみ出す線は本文ブロックと連結しているので必ず残る。
//! 5. **残った成分の bbox の union** を取り、**セーフティパッド**で少し外へ広げる
//!    (端ぎりぎりの線が画面端で切れないように)。
//! 6. 縁が余白で埋まっていない (= フルブリード) / トリム量が極小 のときは `None` で
//!    通常フィットにフォールバック (= 迷ったら切らない)。

use egui::{Color32, ColorImage, Rect, pos2};

/// 余白判定の luma 許容差 (0..255)。広めにして色味の差・薄いゴミを余白扱いにする。
/// 実機で調整要望が出たら設定昇格する想定の固定値。
pub const DEFAULT_TOLERANCE: u8 = 24;

/// 検出に使う縮小後の長辺 px。これより大きい画像は面積平均で縮小してから判定する。
const DETECT_LONG_SIDE: usize = 1000;

/// 縁ピクセルのうち、これ以上の割合が余白色でないと「一様な余白がある」とみなさない
/// (= フルブリード扱いで `None`)。
const BORDER_MARGIN_FRAC: f32 = 0.80;

/// 連結成分の面積 (縮小後 px) がこれ未満なら孤立した点/ゴミとみなして捨てる。
/// 本文や線 (縦横どちらも) はこれを超えるので残る。**ページ番号(ノンブル)などの数字は
/// この値以上の面積があるので残り、スキャンの糊汚れ・端のゴミ(より小さい孤立点)は捨てる**
/// ように調整した値。ゴミが残って余白が詰まらない場合はこれを上げ、ページ番号や小さな中身が
/// 消える場合は下げる (実機で調整して設定昇格する想定の固定値)。
/// なお枠外へ伸びる線などは本文と連結しているのでこの値に関係なく残る。
const MIN_COMPONENT_AREA: usize = 28;

/// 検出 bbox を外側へ広げるセーフティパッド (各辺、正規化比)。端の線が切れないように。
const SAFETY_PAD_FRAC: f32 = 0.006;

/// どの辺もこの割合未満しかトリムされないなら「余白なし」とみなして `None` (無駄なズーム抑止)。
const MIN_TRIM_FRACTION: f32 = 0.01;

#[inline]
fn luma(c: Color32) -> u8 {
    // 0.299R + 0.587G + 0.114B ≒ (77R + 150G + 29B) / 256
    ((77 * c.r() as u32 + 150 * c.g() as u32 + 29 * c.b() as u32) >> 8) as u8
}

/// 検出用に luma バッファへ縮小する (面積平均)。長辺が `DETECT_LONG_SIDE` 以下ならそのまま。
fn downscale_luma(img: &ColorImage) -> (usize, usize, Vec<u8>) {
    let [w, h] = img.size;
    let factor = (w.max(h)).div_ceil(DETECT_LONG_SIDE).max(1);
    if factor == 1 {
        let lum: Vec<u8> = img.pixels.iter().map(|&c| luma(c)).collect();
        return (w, h, lum);
    }
    let sw = w.div_ceil(factor);
    let sh = h.div_ceil(factor);
    let px = &img.pixels;
    let mut lum = vec![0u8; sw * sh];
    for by in 0..sh {
        let y0 = by * factor;
        let y1 = ((by + 1) * factor).min(h);
        for bx in 0..sw {
            let x0 = bx * factor;
            let x1 = ((bx + 1) * factor).min(w);
            let mut sum = 0u32;
            let mut cnt = 0u32;
            for yy in y0..y1 {
                let row = yy * w;
                for xx in x0..x1 {
                    sum += luma(px[row + xx]) as u32;
                    cnt += 1;
                }
            }
            lum[by * sw + bx] = (sum / cnt.max(1)) as u8;
        }
    }
    (sw, sh, lum)
}

/// 上下左右の縁ピクセルの median を余白色 (luma) として推定する。
fn border_median_luma(lum: &[u8], w: usize, h: usize) -> u8 {
    let mut samples: Vec<u8> = Vec::with_capacity(2 * (w + h));
    for x in 0..w {
        samples.push(lum[x]); // top
        samples.push(lum[(h - 1) * w + x]); // bottom
    }
    for y in 0..h {
        samples.push(lum[y * w]); // left
        samples.push(lum[y * w + (w - 1)]); // right
    }
    samples.sort_unstable();
    samples[samples.len() / 2]
}

/// 縁のうち余白色 (margin ± tol) の割合。
fn border_margin_fraction(lum: &[u8], w: usize, h: usize, margin: u8, tol: u8) -> f32 {
    let mut total = 0usize;
    let mut hit = 0usize;
    let mut test = |v: u8| {
        total += 1;
        if v.abs_diff(margin) <= tol {
            hit += 1;
        }
    };
    for x in 0..w {
        test(lum[x]);
        test(lum[(h - 1) * w + x]);
    }
    for y in 0..h {
        test(lum[y * w]);
        test(lum[y * w + (w - 1)]);
    }
    if total == 0 {
        0.0
    } else {
        hit as f32 / total as f32
    }
}

/// 中身マスクを 8 連結でラベリングし、面積 `min_area` 以上の成分の bbox の union を返す。
/// (minx, miny, maxx, maxy) inclusive。該当成分が無ければ `None`。
fn union_bbox_of_large_components(
    mask: &[bool],
    w: usize,
    h: usize,
    min_area: usize,
) -> Option<(usize, usize, usize, usize)> {
    let mut visited = vec![false; w * h];
    let mut stack: Vec<(usize, usize)> = Vec::new();
    let mut res: Option<(usize, usize, usize, usize)> = None;
    for sy in 0..h {
        for sx in 0..w {
            let si = sy * w + sx;
            if !mask[si] || visited[si] {
                continue;
            }
            // この成分を flood fill (8 連結)。
            stack.clear();
            stack.push((sx, sy));
            visited[si] = true;
            let mut area = 0usize;
            let (mut x0, mut y0, mut x1, mut y1) = (sx, sy, sx, sy);
            while let Some((cx, cy)) = stack.pop() {
                area += 1;
                x0 = x0.min(cx);
                y0 = y0.min(cy);
                x1 = x1.max(cx);
                y1 = y1.max(cy);
                let xlo = cx.saturating_sub(1);
                let xhi = (cx + 1).min(w - 1);
                let ylo = cy.saturating_sub(1);
                let yhi = (cy + 1).min(h - 1);
                for ny in ylo..=yhi {
                    for nx in xlo..=xhi {
                        let ni = ny * w + nx;
                        if mask[ni] && !visited[ni] {
                            visited[ni] = true;
                            stack.push((nx, ny));
                        }
                    }
                }
            }
            if area >= min_area {
                res = Some(match res {
                    None => (x0, y0, x1, y1),
                    Some((rx0, ry0, rx1, ry1)) => {
                        (rx0.min(x0), ry0.min(y0), rx1.max(x1), ry1.max(y1))
                    }
                });
            }
        }
    }
    res
}

/// 画像の中身 (非余白) の bounding box を正規化座標 (0..1) で返す。検出できない /
/// フルブリード / トリム量が極小 のときは `None` (= 通常フィットにフォールバック)。
pub fn detect_content_bbox(img: &ColorImage, tol: u8) -> Option<Rect> {
    let [ow, oh] = img.size;
    if ow < 8 || oh < 8 {
        return None;
    }
    let (w, h, lum) = downscale_luma(img);
    if w < 8 || h < 8 {
        return None;
    }

    let margin = border_median_luma(&lum, w, h);
    // 縁が余白で埋まっていない = フルブリード扱い → 切らない。
    if border_margin_fraction(&lum, w, h, margin, tol) < BORDER_MARGIN_FRAC {
        return None;
    }

    // 中身マスク (luma が margin から tol より離れている = ink)。
    let mask: Vec<bool> = lum.iter().map(|&v| v.abs_diff(margin) > tol).collect();
    let (x0, y0, x1, y1) = union_bbox_of_large_components(&mask, w, h, MIN_COMPONENT_AREA)?;

    // トリム量が小さすぎるなら余白なし扱い (パッド前で判定)。
    let trimmed = x0 + (w - 1 - x1) + y0 + (h - 1 - y1);
    let min_trim = (((w + h) as f32) * MIN_TRIM_FRACTION) as usize;
    if trimmed < min_trim.max(1) {
        return None;
    }

    // 正規化 (右/下は +1 して inclusive→exclusive) + セーフティパッド。
    let nx0 = (x0 as f32 / w as f32 - SAFETY_PAD_FRAC).max(0.0);
    let ny0 = (y0 as f32 / h as f32 - SAFETY_PAD_FRAC).max(0.0);
    let nx1 = ((x1 + 1) as f32 / w as f32 + SAFETY_PAD_FRAC).min(1.0);
    let ny1 = ((y1 + 1) as f32 / h as f32 + SAFETY_PAD_FRAC).min(1.0);
    if nx1 - nx0 < 0.05 || ny1 - ny0 < 0.05 {
        return None; // 退化したケースは諦める
    }
    Some(Rect::from_min_max(pos2(nx0, ny0), pos2(nx1, ny1)))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn white(w: usize, h: usize) -> ColorImage {
        ColorImage::new([w, h], vec![Color32::WHITE; w * h])
    }
    fn set(img: &mut ColorImage, x: usize, y: usize, c: Color32) {
        let w = img.size[0];
        img.pixels[y * w + x] = c;
    }
    fn fill(img: &mut ColorImage, x0: usize, y0: usize, x1: usize, y1: usize, c: Color32) {
        for y in y0..y1 {
            for x in x0..x1 {
                set(img, x, y, c);
            }
        }
    }

    #[test]
    fn no_margin_solid_returns_none() {
        assert!(detect_content_bbox(&white(64, 64), DEFAULT_TOLERANCE).is_none());
    }

    #[test]
    fn centered_block_detected() {
        // 64x64 白地、中央 [16,48) に黒ブロック。
        let mut img = white(64, 64);
        fill(&mut img, 16, 16, 48, 48, Color32::BLACK);
        let b = detect_content_bbox(&img, DEFAULT_TOLERANCE).expect("中身検出");
        // 0.25..0.75 付近 (セーフティパッド分の余裕を見て assert)。
        assert!((b.min.x - 0.25).abs() < 0.03, "left {}", b.min.x);
        assert!((b.min.y - 0.25).abs() < 0.03, "top {}", b.min.y);
        assert!((b.max.x - 0.75).abs() < 0.03, "right {}", b.max.x);
        assert!((b.max.y - 0.75).abs() < 0.03, "bottom {}", b.max.y);
    }

    #[test]
    fn isolated_dot_in_margin_is_ignored() {
        // 中央ブロック + 隅に孤立した小さな点 (ゴミ)。点は捨てられ bbox はブロックのみ。
        let mut img = white(64, 64);
        fill(&mut img, 20, 20, 44, 44, Color32::BLACK); // 24x24 本体
        fill(&mut img, 2, 2, 5, 5, Color32::BLACK); // 3x3=9px の孤立点 (< MIN_COMPONENT_AREA)
        let b = detect_content_bbox(&img, DEFAULT_TOLERANCE).expect("検出");
        // 点 (x,y≈2..5) は無視され、左/上はブロック (20/64≒0.31) 付近まで詰む。
        assert!(b.min.x > 0.20, "孤立点を無視して左を詰める: {}", b.min.x);
        assert!(b.min.y > 0.20, "孤立点を無視して上を詰める: {}", b.min.y);
    }

    #[test]
    fn tiny_dirt_dropped_but_pagenumber_sized_kept() {
        // 本体 + 隅の小さなゴミ (面積 9 < MIN) + 下のページ番号サイズの孤立マーク (面積 36 >= MIN)。
        // ゴミは捨てられ、ページ番号サイズは残る (= ゴミは bbox を引っ張らないが番号は残る)。
        let mut img = white(80, 80);
        fill(&mut img, 30, 20, 60, 60, Color32::BLACK); // 本体 30x40
        fill(&mut img, 4, 72, 7, 75, Color32::BLACK); // 3x3=9px の隅ゴミ → drop
        fill(&mut img, 12, 72, 18, 78, Color32::BLACK); // 6x6=36px のページ番号相当 → keep
        let b = detect_content_bbox(&img, DEFAULT_TOLERANCE).expect("検出");
        // 隅ゴミ (x≈4/80=0.05) は無視され、左はページ番号 (x=12/80=0.15) で決まる。
        assert!(
            b.min.x > 0.10,
            "隅ゴミは bbox を引っ張らない (左がゴミ位置 0.05 ではない): {}",
            b.min.x
        );
        // ページ番号サイズは残るので下端まで含む。
        assert!(
            b.max.y > 0.9,
            "ページ番号サイズの孤立マークは残る: {}",
            b.max.y
        );
    }

    #[test]
    fn thin_vertical_line_to_edge_is_kept() {
        // 中央ブロックから左端 (x=1) へ伸びる細い横線 (枠外へはみ出す線の代表)。
        // 線は本体と連結 → 大きな成分 → 残る → 左 bbox は端近くまで広がる (切られない)。
        let mut img = white(80, 80);
        fill(&mut img, 30, 30, 60, 60, Color32::BLACK); // 本体
        fill(&mut img, 1, 44, 30, 46, Color32::BLACK); // 本体から左端へ伸びる横線 (2px 太)
        let b = detect_content_bbox(&img, DEFAULT_TOLERANCE).expect("検出");
        // 線が左端 (x=1) まで伸びているので、左 bbox は 0.05 未満まで詰む (切られていない)。
        assert!(b.min.x < 0.06, "端へ伸びる線が残る: {}", b.min.x);
    }

    #[test]
    fn gradient_border_still_detects() {
        // 上が白・下がやや暗い (勾配)。四隅は揃わないが median 推定で検出は走る。
        let w = 80;
        let h = 80;
        let mut img = white(w, h);
        for y in 0..h {
            let v = (255 - (y as u32 * 30 / h as u32)) as u8; // 255→225 の緩い勾配
            for x in 0..w {
                set(&mut img, x, y, Color32::from_gray(v));
            }
        }
        fill(&mut img, 24, 24, 56, 56, Color32::BLACK);
        // 旧実装 (四隅一致ゲート) なら None になりがちだが、median + 広め tol で検出されること。
        assert!(
            detect_content_bbox(&img, DEFAULT_TOLERANCE).is_some(),
            "緩い勾配でも検出は走る"
        );
    }

    #[test]
    fn full_bleed_border_returns_none() {
        // 縁まで中身がある (フルブリード) → 切らない。
        let mut img = white(64, 64);
        fill(&mut img, 0, 0, 64, 64, Color32::from_gray(40)); // ほぼ全面 ink
        assert!(detect_content_bbox(&img, DEFAULT_TOLERANCE).is_none());
    }
}
