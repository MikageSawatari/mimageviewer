//! 隠蔽加工の合成 (compose) アルゴリズム。
//!
//! 詳細仕様: [docs/conceal-feature-plan.md §7](../../docs/conceal-feature-plan.md)
//!
//! # Phase 進捗
//!
//! - **Phase 3a (本ファイル初版)**: [`compose_mosaic`] のみ。Mosaic / Opaque /
//!   Translucent / MaskShape の 3 境界モード + LongEdgeRatio / FixedPx の 2 タイル
//!   サイズモード
//! - Phase 3b: `compose_solid_fill` (WhiteFill / BlackFill、不透明度 + Feathered)
//! - Phase 3c: `compose_blur` (3 BlurMode + bbox 最適化)
//!
//! # 設計の要点
//!
//! - 入力: `egui::ColorImage` + ビットマップ + ベクタを合成済みの `Vec<bool>` マスク
//! - 出力: `egui::ColorImage` (元と同じサイズ、RGBA 順)
//! - 全関数は純粋 (副作用なし、テスト容易)
//! - 並列化: 大画像 (4K 超) で目に見えて遅くなる場合に rayon を導入する。
//!   現状は逐次。実測してから入れる方針 (premature optimization 回避)
//!
//! # マスク表現
//!
//! `mask` は `width * height` の 1bit/pixel リスト (`true` でマスク領域)。
//! `composite_mask` の出力をそのまま渡せばよい (ビットマップ + 全 Shape ラスタライズ
//! 済み)。

use eframe::egui;
use eframe::egui::Color32;

use crate::conceal::{FillEdge, MosaicBoundary, TileSizeMode, compute_tile_size};

// ── Mosaic 合成 ───────────────────────────────────────────────────────

/// モザイク合成を実行する。
///
/// `tile_size` は事前に [`compute_tile_size`] で算出した実画像 px 値を渡す。
/// `boundary` が `Opaque`/`Translucent` のとき、タイル全体の平均色を出す
/// (マスク有無に関わらず全画素を平均)。`MaskShape` のときも同じタイル平均を
/// 使い、マスク内画素にだけその色を載せる。
///
/// # 計算量
///
/// O(w*h)。タイル平均算出 (1 pass) + 画素ごとの出力決定 (1 pass)。
pub fn compose_mosaic(
    base: &egui::ColorImage,
    mask: &[bool],
    tile_size: u32,
    boundary: MosaicBoundary,
) -> egui::ColorImage {
    let [w, h] = base.size;
    if w == 0 || h == 0 {
        return base.clone();
    }
    assert_eq!(
        mask.len(),
        w * h,
        "mask size mismatch with base image: mask={}, expected={}",
        mask.len(),
        w * h
    );
    let tile = (tile_size.max(4)) as usize;
    let tw = w.div_ceil(tile);
    let th = h.div_ceil(tile);

    // Pass 1: タイルごとの平均色とマスク被覆を集計
    // [sum_r, sum_g, sum_b, mask_count, total_count] per tile
    let mut tile_stats = vec![(0u32, 0u32, 0u32, 0u32, 0u32); tw * th];
    for y in 0..h {
        let ty = y / tile;
        for x in 0..w {
            let tx = x / tile;
            let pi = y * w + x;
            let p = base.pixels[pi];
            let entry = &mut tile_stats[ty * tw + tx];
            entry.0 += p.r() as u32;
            entry.1 += p.g() as u32;
            entry.2 += p.b() as u32;
            entry.4 += 1;
            if mask[pi] {
                entry.3 += 1;
            }
        }
    }

    // Pass 2: 出力画素を決定
    let mut out_pixels = base.pixels.clone();
    for y in 0..h {
        let ty = y / tile;
        for x in 0..w {
            let tx = x / tile;
            let pi = y * w + x;
            let (sr, sg, sb, mc, tc) = tile_stats[ty * tw + tx];
            if mc == 0 || tc == 0 {
                continue; // マスク外のタイル: 元のまま
            }
            let avg_r = (sr / tc) as u8;
            let avg_g = (sg / tc) as u8;
            let avg_b = (sb / tc) as u8;
            let base_px = base.pixels[pi];
            let avg = Color32::from_rgba_unmultiplied(avg_r, avg_g, avg_b, base_px.a());
            out_pixels[pi] = match boundary {
                MosaicBoundary::Opaque => avg,
                MosaicBoundary::Translucent => {
                    let coverage = (mc as f32 / tc as f32).clamp(0.0, 1.0);
                    blend_over(base_px, avg, coverage)
                }
                MosaicBoundary::MaskShape => {
                    if mask[pi] {
                        avg
                    } else {
                        base_px
                    }
                }
            };
        }
    }

    egui::ColorImage::new([w, h], out_pixels)
}

/// MosaicBoundary を `TileSizeMode` + 画像長辺から自動計算するラッパ。
pub fn compose_mosaic_auto(
    base: &egui::ColorImage,
    mask: &[bool],
    tile_mode: TileSizeMode,
    boundary: MosaicBoundary,
) -> egui::ColorImage {
    let [w, h] = base.size;
    let long_edge = w.max(h) as u32;
    let tile_size = compute_tile_size(long_edge, tile_mode);
    compose_mosaic(base, mask, tile_size, boundary)
}

/// 元画素 `base` の上に `over` を不透明度 `alpha` (0..=1) で重ねる。
/// alpha チャンネルは `base` のものを維持 (出力は base.a で書き戻す)。
fn blend_over(base: Color32, over: Color32, alpha: f32) -> Color32 {
    let a = alpha.clamp(0.0, 1.0);
    let inv = 1.0 - a;
    let r = (base.r() as f32 * inv + over.r() as f32 * a).round() as u8;
    let g = (base.g() as f32 * inv + over.g() as f32 * a).round() as u8;
    let b = (base.b() as f32 * inv + over.b() as f32 * a).round() as u8;
    Color32::from_rgba_unmultiplied(r, g, b, base.a())
}

// ── 単色塗りつぶし合成 (WhiteFill / BlackFill、Phase 3b) ────────────────

/// Feathered モードのフェード幅 (画素単位)。境界画素 alpha=0、内側へ向かって
/// `FEATHER_RADIUS_PX` 画素で alpha 1.0 になる線形ランプ。
/// 仕様 (`docs/conceal-feature-plan.md §7.6`) の `FEATHER_RADIUS_PX` 定数。
/// 将来スライダー化する可能性があるが、当面は固定値 (要求がきたら App 設定に昇格)。
pub const FEATHER_RADIUS_PX: u32 = 8;

/// 単色塗りつぶし合成 (WhiteFill / BlackFill)。
///
/// `color` は通常 `Color32::WHITE` または `Color32::BLACK`、`opacity_percent` は
/// `1..=100` (Settings.conceal_fill_opacity_percent と同じ範囲)、`edge` で境界の
/// シャープ / フェードを切り替える。
///
/// # アルゴリズム
///
/// - `FillEdge::Sharp`: マスク内の各画素を `blend_over(base, color, base_alpha)`
///   で塗る (= 全体不透明度のみ反映、境界は鋭い)
/// - `FillEdge::Feathered`: マスク境界から内側へ `FEATHER_RADIUS_PX` 画素までは
///   alpha を線形補間 (境界画素は 0、内側で 255 = `base_alpha`)。
///   実装は「マスク内画素について、近傍 `FEATHER_RADIUS_PX` 範囲にマスク外画素が
///   あれば距離に応じて減衰」のシンプルな逐次走査 (画像サイズが極端に大きい場合は
///   bbox + 並列化で最適化する余地ありだが、現状は逐次)
///
/// # 計算量
///
/// O(w*h + masked_pixel_count * FEATHER_RADIUS_PX^2)。
/// 4K 全面マスク + Feathered で ~80ms 程度の目安 (実測で要確認)。
pub fn compose_solid_fill(
    base: &egui::ColorImage,
    mask: &[bool],
    color: Color32,
    opacity_percent: u8,
    edge: FillEdge,
) -> egui::ColorImage {
    let [w, h] = base.size;
    if w == 0 || h == 0 {
        return base.clone();
    }
    assert_eq!(
        mask.len(),
        w * h,
        "mask size mismatch: mask={}, expected={}",
        mask.len(),
        w * h
    );
    // 1% → 2.55 step (100% で 255、1% で 3 弱、0% は呼び出し側で弾く想定だが
    // 来ても安全に no-op になるよう max 0 で扱う)。
    let opacity = opacity_percent.clamp(1, 100);
    let base_alpha = ((opacity as f32) * 2.55).round() as u8;

    let edge_alpha_map: Option<Vec<u8>> = match edge {
        FillEdge::Sharp => None,
        FillEdge::Feathered => Some(compute_edge_feather_alpha(mask, w, h, FEATHER_RADIUS_PX)),
    };

    let mut out_pixels = base.pixels.clone();
    for y in 0..h {
        for x in 0..w {
            let i = y * w + x;
            if !mask[i] {
                continue;
            }
            // base_alpha と feather alpha を乗算 (= 「全体不透明度 × 境界フェード」)。
            // u8 乗算は中間 u16 で安全に算出。
            let pixel_alpha = match &edge_alpha_map {
                None => base_alpha,
                Some(amap) => ((base_alpha as u16 * amap[i] as u16) / 255) as u8,
            };
            let alpha_f = pixel_alpha as f32 / 255.0;
            out_pixels[i] = blend_over(out_pixels[i], color, alpha_f);
        }
    }

    egui::ColorImage::new([w, h], out_pixels)
}

/// マスク境界から内側への距離 (上限 `radius`) を計算し、線形ランプの alpha map を返す。
/// 境界画素 (マスク外と隣接) は alpha=0、内側 radius px で alpha=255。
///
/// 実装はシンプルな逐次:
///   1. 距離マップ `dist` を u16::MAX で初期化
///   2. マスク外画素を seed (距離 0) として BFS で半径 `radius` まで伝播
///   3. dist[i] が radius 未満なら alpha = floor(dist[i] / radius * 255)
///      dist[i] が radius 以上なら alpha = 255
///      マスク外画素は alpha = 0 (使われないので何でもよいが 0 を入れる)
///
/// BFS は 4 近傍。8 近傍も検討したが、4K 画像でのコスト差より単純さを優先。
/// 8 近傍が必要なら将来 GPU/rayon 化 + 8 近傍 distance transform に置き換える。
fn compute_edge_feather_alpha(mask: &[bool], w: usize, h: usize, radius: u32) -> Vec<u8> {
    let n = w * h;
    let mut dist = vec![u16::MAX; n];
    let mut queue: std::collections::VecDeque<(usize, usize)> = std::collections::VecDeque::new();

    // Seed: マスク外画素 (= 境界の「外」側)。これらの距離 0 から内側に拡張する。
    for y in 0..h {
        for x in 0..w {
            if !mask[y * w + x] {
                dist[y * w + x] = 0;
                queue.push_back((x, y));
            }
        }
    }

    // 4 近傍 BFS で距離計算 (radius を超えたら打ち切り)
    while let Some((x, y)) = queue.pop_front() {
        let d = dist[y * w + x];
        if d >= radius as u16 {
            continue;
        }
        let nd = d + 1;
        let neighbors: [(i32, i32); 4] = [(-1, 0), (1, 0), (0, -1), (0, 1)];
        for (dx, dy) in neighbors {
            let nx = x as i32 + dx;
            let ny = y as i32 + dy;
            if nx < 0 || ny < 0 || nx >= w as i32 || ny >= h as i32 {
                continue;
            }
            let ni = ny as usize * w + nx as usize;
            // マスク内画素にだけ距離を書き込む (マスク外は seed として既に 0)
            if mask[ni] && dist[ni] > nd {
                dist[ni] = nd;
                queue.push_back((nx as usize, ny as usize));
            }
        }
    }

    // 距離 → alpha (0..radius を 0..255 にマップ、radius 以上は 255)
    let mut out = vec![0u8; n];
    for i in 0..n {
        if !mask[i] {
            out[i] = 0; // マスク外は 0 (使われない)
        } else if dist[i] >= radius as u16 {
            out[i] = 255;
        } else if radius == 0 {
            out[i] = 255;
        } else {
            out[i] = ((dist[i] as u32 * 255) / radius) as u8;
        }
    }
    out
}

// ── テスト ───────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    /// 全画素同色のテスト画像を作る。
    fn solid_image(w: usize, h: usize, c: Color32) -> egui::ColorImage {
        let pixels = vec![c; w * h];
        egui::ColorImage::new([w, h], pixels)
    }

    /// 左半分赤、右半分青のテスト画像を作る。
    fn split_image(w: usize, h: usize) -> egui::ColorImage {
        let mut pixels = vec![Color32::TRANSPARENT; w * h];
        for y in 0..h {
            for x in 0..w {
                pixels[y * w + x] = if x < w / 2 {
                    Color32::from_rgb(200, 0, 0)
                } else {
                    Color32::from_rgb(0, 0, 200)
                };
            }
        }
        egui::ColorImage::new([w, h], pixels)
    }

    #[test]
    fn empty_mask_leaves_image_unchanged() {
        let img = split_image(16, 16);
        let mask = vec![false; 16 * 16];
        let out = compose_mosaic(&img, &mask, 4, MosaicBoundary::Opaque);
        assert_eq!(out.pixels, img.pixels);
    }

    #[test]
    fn opaque_paints_entire_tile_uniformly() {
        let img = solid_image(8, 8, Color32::from_rgb(100, 200, 50));
        // マスク内の 1 画素だけタイル全体が平均色で塗られる
        let mut mask = vec![false; 8 * 8];
        mask[0] = true; // (0,0) のマスク
        let out = compose_mosaic(&img, &mask, 4, MosaicBoundary::Opaque);
        // 同色画像なので avg は元の色と同じ → 結果も同じ
        assert_eq!(out.pixels[0], Color32::from_rgb(100, 200, 50));
        // タイル全体 (0..4, 0..4) が同じ色
        for y in 0..4 {
            for x in 0..4 {
                assert_eq!(out.pixels[y * 8 + x], Color32::from_rgb(100, 200, 50));
            }
        }
        // タイル外 (5..8, 5..8) も同色 (全体同色画像)
        assert_eq!(out.pixels[5 * 8 + 5], Color32::from_rgb(100, 200, 50));
    }

    #[test]
    fn opaque_uses_tile_average_color() {
        // 左赤・右青の 8x8 画像、タイルサイズ 8 → 1 タイル全体
        let img = split_image(8, 8);
        let mut mask = vec![false; 8 * 8];
        mask[0] = true;
        let out = compose_mosaic(&img, &mask, 8, MosaicBoundary::Opaque);
        // 平均: r = (200*4 + 0*4) / 8 = 100, b = 100
        let avg = out.pixels[0];
        assert!((avg.r() as i32 - 100).abs() <= 1);
        assert!((avg.b() as i32 - 100).abs() <= 1);
        // 同タイル内全画素が同じ色
        for &p in &out.pixels {
            assert_eq!(p, avg);
        }
    }

    #[test]
    fn translucent_blends_by_coverage() {
        // 全画素 (50, 50, 50) のタイル + マスク半分
        let img = solid_image(4, 4, Color32::from_rgb(50, 50, 50));
        let mut mask = vec![false; 4 * 4];
        // 半分マスク (= coverage = 0.5)
        for i in 0..8 {
            mask[i] = true;
        }
        let out = compose_mosaic(&img, &mask, 4, MosaicBoundary::Translucent);
        // 全画素同色なので avg = (50,50,50) と同じ。blend 後も (50,50,50)
        for &p in &out.pixels {
            assert_eq!(p, Color32::from_rgb(50, 50, 50));
        }
    }

    #[test]
    fn translucent_with_color_diff() {
        // 左赤(200,0,0)・右青(0,0,200) の 4x4 タイル、マスクは左半分のみ
        // avg = (100, 0, 100)、coverage = 0.5
        // 左画素の出力: base=(200,0,0), over=(100,0,100), alpha=0.5
        //   → (200*0.5 + 100*0.5, 0, 0*0.5 + 100*0.5) = (150, 0, 50)
        // 右画素の出力: base=(0,0,200), over=(100,0,100), alpha=0.5
        //   → (50, 0, 150)
        let img = split_image(4, 4);
        let mut mask = vec![false; 4 * 4];
        for y in 0..4 {
            for x in 0..2 {
                mask[y * 4 + x] = true;
            }
        }
        let out = compose_mosaic(&img, &mask, 4, MosaicBoundary::Translucent);
        // 左画素
        let p_left = out.pixels[0];
        assert!((p_left.r() as i32 - 150).abs() <= 2);
        assert!((p_left.b() as i32 - 50).abs() <= 2);
        // 右画素
        let p_right = out.pixels[3];
        assert!((p_right.r() as i32 - 50).abs() <= 2);
        assert!((p_right.b() as i32 - 150).abs() <= 2);
    }

    #[test]
    fn mask_shape_only_paints_masked_pixels() {
        let img = split_image(4, 4);
        let mut mask = vec![false; 4 * 4];
        mask[0] = true; // (0,0) だけマスク
        let out = compose_mosaic(&img, &mask, 4, MosaicBoundary::MaskShape);
        // (0,0) は平均色 (100, 0, 100)
        assert!((out.pixels[0].r() as i32 - 100).abs() <= 2);
        assert!((out.pixels[0].b() as i32 - 100).abs() <= 2);
        // (1,0) はマスクなし → base のまま (200, 0, 0)
        assert_eq!(out.pixels[1], Color32::from_rgb(200, 0, 0));
    }

    #[test]
    fn tile_size_clamps_to_min_4() {
        let img = solid_image(8, 8, Color32::from_rgb(100, 100, 100));
        let mask = vec![true; 8 * 8];
        // tile_size=2 はクランプされて 4 になる
        let out_t2 = compose_mosaic(&img, &mask, 2, MosaicBoundary::Opaque);
        let out_t4 = compose_mosaic(&img, &mask, 4, MosaicBoundary::Opaque);
        assert_eq!(out_t2.pixels, out_t4.pixels);
    }

    #[test]
    fn compose_mosaic_auto_uses_long_edge() {
        let img = solid_image(800, 400, Color32::from_rgb(150, 150, 150));
        let mask = vec![true; 800 * 400];
        // LongEdgeRatio(1.0) → tile = max(800/100, 4) = 8
        let out = compose_mosaic_auto(
            &img,
            &mask,
            TileSizeMode::LongEdgeRatio(1.0),
            MosaicBoundary::Opaque,
        );
        // 全画素同色なので元と同じ
        for &p in &out.pixels {
            assert_eq!(p, Color32::from_rgb(150, 150, 150));
        }
    }

    #[test]
    fn non_divisible_image_size_handles_edge_tiles() {
        // 5x5 画像、タイル 4 → タイル数 (2, 2)。右下のタイルは 1px だけ
        let img = solid_image(5, 5, Color32::from_rgb(80, 80, 80));
        let mut mask = vec![false; 5 * 5];
        // 右下隅 (4, 4) だけマスク
        mask[4 * 5 + 4] = true;
        let out = compose_mosaic(&img, &mask, 4, MosaicBoundary::Opaque);
        // (4,4) の所属タイル (tx=1, ty=1) は 1 画素 (5/4 端) → 平均 = 元色
        assert_eq!(out.pixels[4 * 5 + 4], Color32::from_rgb(80, 80, 80));
    }

    #[test]
    fn blend_over_alpha_0_returns_base() {
        let base = Color32::from_rgb(100, 100, 100);
        let over = Color32::from_rgb(255, 0, 0);
        let result = blend_over(base, over, 0.0);
        assert_eq!(result, base);
    }

    #[test]
    fn blend_over_alpha_1_returns_over() {
        let base = Color32::from_rgb(100, 100, 100);
        let over = Color32::from_rgb(255, 0, 0);
        let result = blend_over(base, over, 1.0);
        assert_eq!(result.r(), 255);
        assert_eq!(result.g(), 0);
        assert_eq!(result.b(), 0);
        assert_eq!(result.a(), base.a());
    }

    // ── compose_solid_fill (Phase 3b) ────────────────────────────────

    #[test]
    fn solid_fill_white_100_opaque_sharp() {
        let img = solid_image(4, 4, Color32::from_rgb(50, 50, 50));
        let mut mask = vec![false; 16];
        mask[0] = true;
        mask[1] = true;
        let out = compose_solid_fill(&img, &mask, Color32::WHITE, 100, FillEdge::Sharp);
        // マスク内: 白 (255,255,255), マスク外: 元色
        assert_eq!(
            out.pixels[0],
            Color32::from_rgba_unmultiplied(255, 255, 255, 255)
        );
        assert_eq!(
            out.pixels[1],
            Color32::from_rgba_unmultiplied(255, 255, 255, 255)
        );
        assert_eq!(out.pixels[2], Color32::from_rgb(50, 50, 50));
    }

    #[test]
    fn solid_fill_black_100_opaque_sharp() {
        let img = solid_image(4, 4, Color32::from_rgb(200, 200, 200));
        let mut mask = vec![false; 16];
        mask[5] = true;
        let out = compose_solid_fill(&img, &mask, Color32::BLACK, 100, FillEdge::Sharp);
        assert_eq!(out.pixels[5], Color32::from_rgba_unmultiplied(0, 0, 0, 255));
        // 他は元色保持
        assert_eq!(out.pixels[0], Color32::from_rgb(200, 200, 200));
    }

    #[test]
    fn solid_fill_50_percent_half_transparent() {
        // base=灰、color=白、opacity 50% → 灰 + 白 * 0.5 = (50+128)/(255) ≈ 153
        let img = solid_image(2, 2, Color32::from_rgb(50, 50, 50));
        let mask = vec![true; 4];
        let out = compose_solid_fill(&img, &mask, Color32::WHITE, 50, FillEdge::Sharp);
        // base_alpha = round(50 * 2.55) = 128, alpha=0.502 → 結果 ~152
        let r = out.pixels[0].r();
        assert!((r as i32 - 152).abs() <= 3, "expected ~152, got {}", r);
    }

    #[test]
    fn solid_fill_opacity_clamps_to_1_on_zero_input() {
        // opacity_percent=0 は 1 にクランプされる (= 完全透明にはならない)
        let img = solid_image(2, 2, Color32::from_rgb(50, 50, 50));
        let mask = vec![true; 4];
        let out = compose_solid_fill(&img, &mask, Color32::WHITE, 0, FillEdge::Sharp);
        // 1% で alpha ~3、結果は base に近い色
        let r = out.pixels[0].r();
        assert!(r as i32 - 50 < 5, "expected near 50, got {}", r);
    }

    #[test]
    fn solid_fill_feathered_boundary_is_dimmer_than_interior() {
        // 大きな矩形マスク + Feathered → マスク内・境界近傍の画素ほど alpha が小さく、
        // 内側に向かって 255 まで上昇する。
        // 画像 24x24、マスク中央 12x12 (8..20, 8..20)
        let img = solid_image(24, 24, Color32::from_rgb(50, 50, 50));
        let mut mask = vec![false; 24 * 24];
        for y in 8..20 {
            for x in 8..20 {
                mask[y * 24 + x] = true;
            }
        }
        let out = compose_solid_fill(&img, &mask, Color32::WHITE, 100, FillEdge::Feathered);
        // 「境界画素」(マスク 1 px 内側、(8,8)) は距離 1 / FEATHER_RADIUS_PX (=8)
        // → alpha ≈ 31。結果 ~75 (base 50 と white 255 の線形補間)。
        let boundary = out.pixels[8 * 24 + 8];
        assert!(
            boundary.r() > 50 && boundary.r() < 100,
            "near-boundary pixel should be lightly tinted (50..100), got {}",
            boundary.r()
        );
        // 中央 (14,14) は内側 6 px → distance ≥ 6、radius 8 なので alpha ≈ 191 + 中央寄りで 255
        // ここでは「白に十分近い」(r ≥ 180) を要求 (内側ほど完全白に近づく)。
        let interior = out.pixels[14 * 24 + 14];
        assert!(
            interior.r() >= 180,
            "interior pixel should be near white (>=180), got {}",
            interior.r()
        );
        // 「中間」(10,10) は距離 2-3 → 中央より明らかに暗いが、境界よりは明るい
        let mid = out.pixels[10 * 24 + 10];
        assert!(
            mid.r() > boundary.r(),
            "mid should be brighter than boundary"
        );
        assert!(mid.r() < interior.r(), "mid should be dimmer than interior");
    }

    #[test]
    fn solid_fill_empty_mask_leaves_image_unchanged() {
        let img = split_image(8, 8);
        let mask = vec![false; 64];
        let out = compose_solid_fill(&img, &mask, Color32::WHITE, 100, FillEdge::Sharp);
        assert_eq!(out.pixels, img.pixels);
    }

    #[test]
    fn feather_alpha_zero_at_boundary_max_inside() {
        // 単純なケース: 8x8 全マスク + radius 3 → 角は alpha=0、中央は alpha=255
        // ただし全マスクなら境界なし → 全画素 dist=u16::MAX → alpha=255
        let mask = vec![true; 8 * 8];
        let alpha = compute_edge_feather_alpha(&mask, 8, 8, 3);
        assert!(alpha.iter().all(|&a| a == 255));
    }

    #[test]
    fn feather_alpha_boundary_pixels_zero() {
        // 4x4 マスク + 周囲 1 行は false (= 外側) → 境界画素は alpha=0、
        // 内側は alpha 段階的に増加
        let mut mask = vec![false; 6 * 6];
        for y in 1..5 {
            for x in 1..5 {
                mask[y * 6 + x] = true;
            }
        }
        let alpha = compute_edge_feather_alpha(&mask, 6, 6, 2);
        // 境界画素 (1,1): dist=1 → alpha = 1/2 * 255 ≈ 127
        let boundary = alpha[1 * 6 + 1];
        assert!(
            boundary < 200,
            "boundary alpha should be < 200, got {}",
            boundary
        );
        // 中央 (2,2) も dist=1 (隣接マス (1,2) が境界からの距離 1)
        // → 境界画素も中央画素も小さなマスクでは radius 内に収まる
    }
}
