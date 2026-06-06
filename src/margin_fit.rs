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
/// (= フルブリード扱いで `None`)。**漫画は絵が一部の辺だけ裁ち落とし (ブリード) で端まで
/// 届くことが多い** (例: 左・上・下は端まで絵があり右だけ余白)。閾値を高くすると、こうした
/// ページを「縁が余白でない」と誤判定して丸ごとカットしなくなり、余白のある辺 (右) も
/// 詰まらない。0.50 にして「縁の過半が余白色なら、余白のある辺だけ詰める」方針にする。
/// 縁の余白色は median 推定なので、過半が余白なら色推定は正しく、緩めても誤検出しない。
/// (= 真のフルブリード: 縁の半分以上が絵 なら従来どおり `None`)。
const BORDER_MARGIN_FRAC: f32 = 0.50;

/// 推定した余白色 (luma) がこの「中間調」帯 (DARK_MAX..LIGHT_MIN) に入るときは切らない。
/// 漫画の余白は紙白か一様ベタ黒なので、中間調が余白色として出るのは「絵 (スクリーントーンの
/// 平均など) を余白と誤推定している」サイン (BORDER_MARGIN_FRAC を 0.50 に下げた副作用で、
/// 絵が縁の過半を占めると median が絵の色になり、本物の余白側を中身扱いして絵を切りうる)。
/// 中間調余白は信頼できないので `None` (= 迷ったら切らない / 全部映る優先)。実機で調整可。
const MARGIN_PAPER_LIGHT_MIN: u8 = 205;
const MARGIN_PAPER_DARK_MAX: u8 = 50;

/// 連結成分の面積 (縮小後 px) がこれ未満なら孤立した点/ゴミとみなして捨てる。
/// 本文や線 (縦横どちらも) はこれを超えるので残る。**ページ番号(ノンブル)などの数字を残し、
/// より小さいスキャンの糊汚れ・端のゴミは捨てる** 境目。
///
/// 実機ログ (4095px のページを 5 分の 1 縮小 → 568x819) で、ページ番号「4」=面積17 /
/// 「5」=面積20 と判明。28 だと番号まで落として余白側でカットしていたので、番号 (17/20) を
/// 残す 15 にした。これで番号が「中身」扱いになり bbox が番号まで広がる → 番号は残し、その
/// 外側の余白だけ切れる。
///
/// トレードオフ: 縮小が強い (高解像度の) ページでは番号もゴミも縮小後は小さく、面積で
/// 完全には分離できない。番号は全ページにあり優先度が高いので「番号を残す」側に寄せた値。
/// ゴミが残って余白が詰まらない場合は上げ、番号や小さな中身が消える場合は下げる
/// (実機で調整して設定昇格する想定の固定値)。
/// なお枠外へ伸びる線などは本文と連結しているのでこの値に関係なく残る。
const MIN_COMPONENT_AREA: usize = 15;

/// 検出 bbox を外側へ広げるセーフティパッド (各辺、正規化比)。端の線が切れないように。
const SAFETY_PAD_FRAC: f32 = 0.006;

/// どの辺もこの割合未満しかトリムされないなら「余白なし」とみなして `None` (無駄なズーム抑止)。
const MIN_TRIM_FRACTION: f32 = 0.01;

/// 各辺でトリムできる最大割合 (上限)。中身が極小/隅に偏った余白だらけのページで、その一部分
/// だけが巨大に拡大される「やりすぎ」を防ぐ。20% なら最悪でも中央 60% 四方は必ず表示される。
/// 通常の漫画ページ (余白 5〜10%) では上限に達しないので影響しない。実機で調整して設定昇格
/// する想定の固定値。
const MAX_TRIM_FRAC: f32 = 0.20;

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

/// 紙白寄り (luma >= MARGIN_PAPER_LIGHT_MIN) のサンプルの割合。
fn light_fraction(samples: impl Iterator<Item = u8>) -> f32 {
    let mut total = 0usize;
    let mut light = 0usize;
    for v in samples {
        total += 1;
        if v >= MARGIN_PAPER_LIGHT_MIN {
            light += 1;
        }
    }
    if total == 0 {
        0.0
    } else {
        light as f32 / total as f32
    }
}

/// 上下左右いずれかの縁 (1px ライン) が紙白寄りで過半 (60%) を占めるか。
/// 「ベタ黒余白」と推定したのに別の辺が真っ白なら、黒い絵が縁の過半を占めて black margin と
/// 誤推定し本物の白余白側を中身扱いしている疑い (反転) の検出に使う。
fn any_border_side_is_light(lum: &[u8], w: usize, h: usize) -> bool {
    const T: f32 = 0.60;
    light_fraction((0..w).map(|x| lum[x])) >= T // top
        || light_fraction((0..w).map(|x| lum[(h - 1) * w + x])) >= T // bottom
        || light_fraction((0..h).map(|y| lum[y * w])) >= T // left
        || light_fraction((0..h).map(|y| lum[y * w + (w - 1)])) >= T // right
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
    // 余白色が中間調 = 絵を余白と誤推定している疑い → 切らない (反転して絵を切る事故を防ぐ)。
    if margin > MARGIN_PAPER_DARK_MAX && margin < MARGIN_PAPER_LIGHT_MIN {
        return None;
    }
    // 余白色が暗い (ベタ黒余白) 推定なのに、いずれかの縁が真っ白なら、黒い絵が縁の過半を
    // 占めて black margin と誤推定し本物の白余白側を中身扱いしている疑い → 切らない。
    // (正当な全周ベタ黒余白はどの縁も白くないので通る。Codex P2 対応)
    if margin <= MARGIN_PAPER_DARK_MAX && any_border_side_is_light(&lum, w, h) {
        return None;
    }
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

    // 正規化 (右/下は +1 して inclusive→exclusive) + セーフティパッド + トリム上限。
    // 各辺のトリムは MAX_TRIM_FRAC まで (隅の小さな中身だけに巨大ズームする "やりすぎ" を防ぐ)。
    let nx0 = (x0 as f32 / w as f32 - SAFETY_PAD_FRAC)
        .max(0.0)
        .min(MAX_TRIM_FRAC);
    let ny0 = (y0 as f32 / h as f32 - SAFETY_PAD_FRAC)
        .max(0.0)
        .min(MAX_TRIM_FRAC);
    let nx1 = ((x1 + 1) as f32 / w as f32 + SAFETY_PAD_FRAC)
        .min(1.0)
        .max(1.0 - MAX_TRIM_FRAC);
    let ny1 = ((y1 + 1) as f32 / h as f32 + SAFETY_PAD_FRAC)
        .min(1.0)
        .max(1.0 - MAX_TRIM_FRAC);
    if nx1 - nx0 < 0.05 || ny1 - ny0 < 0.05 {
        return None; // 退化したケースは諦める
    }
    Some(Rect::from_min_max(pos2(nx0, ny0), pos2(nx1, ny1)))
}

/// 連結成分を全て (面積・bbox) 列挙する (診断用、min_area フィルタなし)。
fn all_components(mask: &[bool], w: usize, h: usize) -> Vec<(usize, usize, usize, usize, usize)> {
    let mut visited = vec![false; w * h];
    let mut stack: Vec<(usize, usize)> = Vec::new();
    let mut out = Vec::new();
    for sy in 0..h {
        for sx in 0..w {
            let si = sy * w + sx;
            if !mask[si] || visited[si] {
                continue;
            }
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
            out.push((area, x0, y0, x1, y1));
        }
    }
    out
}

/// 1 連結成分の診断情報。
#[derive(Clone, Debug)]
pub struct DiagComponent {
    /// 縮小後 px の面積。
    pub area: usize,
    /// 正規化 (0..1) の bbox。
    pub rect: Rect,
    /// 面積が `MIN_COMPONENT_AREA` 以上 (= 中身として bbox に効く) か。
    pub kept: bool,
}

/// 余白カット検出の診断結果 (デバッグログ用)。
pub struct MarginFitDiag {
    /// 検出に使った縮小後サイズ。
    pub downscaled: (usize, usize),
    /// 推定した余白色 (luma)。
    pub margin_luma: u8,
    /// 縁が余白色だった割合。
    pub border_margin_frac: f32,
    /// 中身とみなす最小面積 (`MIN_COMPONENT_AREA`)。
    pub min_area: usize,
    /// 各辺のトリム上限 (`MAX_TRIM_FRAC`)。
    pub max_trim_frac: f32,
    /// 使った luma 許容差。
    pub tol: u8,
    /// 全連結成分 (面積降順)。
    pub components: Vec<DiagComponent>,
    /// 最終 bbox (cap+pad 込み、None なら通常フィット)。
    pub bbox: Option<Rect>,
}

/// 余白カット検出の診断。検出と同じ前処理で全成分を列挙し、最終 bbox と併せて返す。
/// `detect_content_bbox` には影響しない読み取り専用の解析。
pub fn diagnose(img: &ColorImage, tol: u8) -> MarginFitDiag {
    let (w, h, lum) = downscale_luma(img);
    let margin = border_median_luma(&lum, w, h);
    let bmf = border_margin_fraction(&lum, w, h, margin, tol);
    let mask: Vec<bool> = lum.iter().map(|&v| v.abs_diff(margin) > tol).collect();
    let mut comps: Vec<DiagComponent> = all_components(&mask, w, h)
        .into_iter()
        .map(|(area, x0, y0, x1, y1)| DiagComponent {
            area,
            rect: Rect::from_min_max(
                pos2(x0 as f32 / w as f32, y0 as f32 / h as f32),
                pos2((x1 + 1) as f32 / w as f32, (y1 + 1) as f32 / h as f32),
            ),
            kept: area >= MIN_COMPONENT_AREA,
        })
        .collect();
    comps.sort_by(|a, b| b.area.cmp(&a.area));
    MarginFitDiag {
        downscaled: (w, h),
        margin_luma: margin,
        border_margin_frac: bmf,
        min_area: MIN_COMPONENT_AREA,
        max_trim_frac: MAX_TRIM_FRAC,
        tol,
        components: comps,
        bbox: detect_content_bbox(img, tol),
    }
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
        // 64x64 白地、中央 [10,54) に黒ブロック (余白 ~15.6% で MAX_TRIM_FRAC 20% 未満)。
        let mut img = white(64, 64);
        fill(&mut img, 10, 10, 54, 54, Color32::BLACK); // [10,54) x [10,54)
        let b = detect_content_bbox(&img, DEFAULT_TOLERANCE).expect("中身検出");
        // 0.156..0.844 付近 (セーフティパッド分の余裕を見て assert)。
        assert!((b.min.x - 0.156).abs() < 0.03, "left {}", b.min.x);
        assert!((b.min.y - 0.156).abs() < 0.03, "top {}", b.min.y);
        assert!((b.max.x - 0.844).abs() < 0.03, "right {}", b.max.x);
        assert!((b.max.y - 0.844).abs() < 0.03, "bottom {}", b.max.y);
    }

    #[test]
    fn isolated_dot_in_margin_is_ignored() {
        // 中央ブロック (余白 ~15.6% < 上限) + 隅に孤立した小さな点 (ゴミ)。点は捨てられる。
        let mut img = white(64, 64);
        fill(&mut img, 10, 10, 54, 54, Color32::BLACK); // 44x44 本体
        fill(&mut img, 2, 2, 5, 5, Color32::BLACK); // 3x3=9px の孤立点 (< MIN_COMPONENT_AREA)
        let b = detect_content_bbox(&img, DEFAULT_TOLERANCE).expect("検出");
        // 点 (x,y≈2..5 → 0.03) は無視され、左/上はブロック (10/64≒0.156) 付近まで詰む。
        assert!(b.min.x > 0.10, "孤立点を無視して左を詰める: {}", b.min.x);
        assert!(b.min.y > 0.10, "孤立点を無視して上を詰める: {}", b.min.y);
    }

    #[test]
    fn tiny_dirt_dropped_but_pagenumber_sized_kept() {
        // 本体 + 隅の小さなゴミ (面積 9 < MIN) + 下のページ番号相当の孤立マーク (面積 20 >= MIN)。
        // ページ番号の面積は実機ログ由来 (4095px ページの 5 分の 1 縮小で「4」=17/「5」=20)。
        // ゴミは捨てられ、ページ番号は残る (= ゴミは bbox を引っ張らないが番号は残り、切られない)。
        let mut img = white(80, 80);
        fill(&mut img, 30, 20, 60, 60, Color32::BLACK); // 本体 30x40
        fill(&mut img, 4, 72, 7, 75, Color32::BLACK); // 3x3=9px の隅ゴミ → drop (< 15)
        fill(&mut img, 12, 72, 17, 76, Color32::BLACK); // 5x4=20px のページ番号相当 → keep (>= 15)
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

    #[test]
    fn partial_bleed_cuts_only_margin_edges() {
        // 漫画の裁ち落とし相当: 絵が左右の縁まで届き (border_frac が 0.80 未満に下がる)、
        // 上下に余白。旧 BORDER_MARGIN_FRAC=0.80 だと丸ごと None だったが、緩和後は
        // 余白のある上下だけ詰め、中身のある左右は端のまま残す。
        let mut img = white(100, 100);
        fill(&mut img, 0, 10, 100, 90, Color32::BLACK); // 全幅 y[10,90) (左右ブリード)
        let b = detect_content_bbox(&img, DEFAULT_TOLERANCE).expect("部分ブリードでも検出される");
        assert!(b.min.y > 0.05, "上の余白を詰める: {}", b.min.y);
        assert!(b.max.y < 0.95, "下の余白を詰める: {}", b.max.y);
        assert!(
            b.min.x < 0.05,
            "左は端まで (中身がある=詰めない): {}",
            b.min.x
        );
        assert!(
            b.max.x > 0.95,
            "右は端まで (中身がある=詰めない): {}",
            b.max.x
        );
    }

    #[test]
    fn midtone_margin_color_is_not_cut() {
        // 縁の過半が中間調 (絵が左/上/下まで来て、右だけ白い本物の余白という構成を模す)。
        // median が絵の色 (mid-gray) になり、本物の白余白を中身扱いして絵を切る反転事故が
        // 起きうるので、中間調が余白色に出たら None (= 切らない / 全部映る優先) にする。
        let mut img = white(100, 100);
        fill(&mut img, 0, 0, 100, 100, Color32::from_gray(128)); // 全面 mid-gray (= 絵)
        fill(&mut img, 92, 0, 100, 100, Color32::WHITE); // 右に白い帯 (本物の余白)
        assert!(
            detect_content_bbox(&img, DEFAULT_TOLERANCE).is_none(),
            "中間調が余白色なら切らない (絵を切る反転を防ぐ)"
        );
    }

    #[test]
    fn black_art_bleed_with_white_margin_is_not_cut() {
        // ベタ黒の絵が左/上/下の縁まで来て、右だけ白い余白。median はベタ黒 (<=50) になり
        // 中間調ガードをすり抜けるが、右の縁が真っ白なので反転検出 (any_border_side_is_light)
        // で None にし、黒い絵を切らない。
        let mut img = white(100, 100);
        fill(&mut img, 0, 0, 92, 100, Color32::BLACK); // 左 92% ベタ黒 (絵)、右 8% 白余白
        assert!(
            detect_content_bbox(&img, DEFAULT_TOLERANCE).is_none(),
            "黒絵3辺ブリード+白余白は反転検出で切らない"
        );
    }

    #[test]
    fn diagnose_lists_components_by_area() {
        let mut img = white(64, 64);
        fill(&mut img, 10, 10, 54, 54, Color32::BLACK); // 本体 (大)
        fill(&mut img, 2, 2, 4, 4, Color32::BLACK); // 2x2=4 の小点 (< MIN)
        let d = diagnose(&img, DEFAULT_TOLERANCE);
        assert!(d.components.len() >= 2, "本体 + 小点で 2 成分以上");
        assert!(
            d.components[0].area >= d.components[1].area,
            "面積降順でソート"
        );
        assert!(d.components[0].kept, "本体は KEEP");
        assert!(
            !d.components.last().unwrap().kept,
            "最小成分 (小点) は drop"
        );
        assert!(d.bbox.is_some(), "bbox は検出される");
    }

    #[test]
    fn mostly_empty_page_trim_is_capped() {
        // ほぼ全面が余白色、左下隅にだけ小さな中身 (ロゴ相当)。トリム上限がないと隅だけに
        // 巨大ズームしてしまうが、上限 (MAX_TRIM_FRAC) で中央付近は必ず残る。
        let mut img = white(100, 100);
        fill(&mut img, 5, 80, 30, 95, Color32::BLACK); // 25x15 の左下ロゴ (>= MIN_COMPONENT_AREA)
        let b = detect_content_bbox(&img, DEFAULT_TOLERANCE).expect("検出");
        // 中身は左下なので右/上はほぼ全部余白だが、トリム上限で右は 0.80 まで・上は 0.20 まで。
        assert!(
            b.max.x >= 0.80 - 1e-3,
            "右トリムは上限で止まる: {}",
            b.max.x
        );
        assert!(
            b.min.y <= 0.20 + 1e-3,
            "上トリムは上限で止まる: {}",
            b.min.y
        );
        // 中身のある左/下は実位置まで詰む (上限内)。
        assert!(b.min.x < 0.20, "左は中身位置まで詰む: {}", b.min.x);
        assert!(b.max.y > 0.80, "下は中身位置まで詰む: {}", b.max.y);
    }
}
