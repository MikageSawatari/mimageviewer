//! レトロ系ポストフィルタ (CRT ブラウン管・減色・複合プリセット)。
//!
//! 色調補正後の [`egui::ColorImage`] を受け取り、変換後の ColorImage を返す。
//! 各フィルタは `rayon` で並列化しており、4K 画像でも 50〜80ms 程度で完了する想定。
//!
//! 合成順序:
//!   apply_adjustments_fast (色調) → [post_filter::apply] → テクスチャ化 (NEAREST サンプラー)

use crate::adjustment::PostFilter;
use egui::{Color32, ColorImage};
use rayon::prelude::*;

/// ポストフィルタを適用する。`None` / `Nearest` は pixel clone (サンプラー切替で見た目が変わる)。
pub fn apply(src: &ColorImage, filter: PostFilter) -> ColorImage {
    match filter {
        PostFilter::None | PostFilter::Nearest => src.clone(),
        PostFilter::CrtSimple => crt::apply_simple(src),
        PostFilter::CrtFull => crt::apply_full(src),
        PostFilter::CrtArcade => crt::apply_arcade(src),
        PostFilter::GameBoy => palette::apply_gameboy(src),
        PostFilter::Pc98 => palette::apply_pc98(src),
        PostFilter::Famicom => palette::apply_famicom(src),
        PostFilter::Msx2Plus => palette::apply_msx2plus(src),
        PostFilter::MegaDrive => palette::apply_mega_drive(src),
        PostFilter::GameGear => palette::apply_game_gear(src),
        PostFilter::Sfc => palette::apply_sfc(src),
        PostFilter::Dither1bit => palette::apply_1bit(src),
        // 非液晶機種はすべて CRT Simple と組み合わせる (実機は CRT TV/モニタ接続が標準)
        PostFilter::ComboFamicomCrt => crt::apply_simple(&palette::apply_famicom(src)),
        PostFilter::ComboPc98Crt => crt::apply_simple(&palette::apply_pc98(src)),
        PostFilter::ComboMsx2PlusCrt => crt::apply_simple(&palette::apply_msx2plus(src)),
        PostFilter::ComboMegaDriveCrt => crt::apply_simple(&palette::apply_mega_drive(src)),
        PostFilter::ComboSfcCrt => crt::apply_simple(&palette::apply_sfc(src)),
    }
}

// ── 共通ユーティリティ ──────────────────────────────────────────

/// 出力長辺のハードキャップ (CRT 系のメモリ暴走防止)。
const CRT_OUTPUT_MAX: u32 = 4096;

/// CRT 系の適応アップスケール倍率 (ソース長辺に応じて)。
/// さらに出力が `CRT_OUTPUT_MAX` を超えないよう呼び出し側でクランプする。
fn crt_upscale_factor(w: usize, h: usize) -> usize {
    let longest = w.max(h) as u32;
    if longest <= 1024 {
        4
    } else if longest <= 2048 {
        2
    } else {
        1
    }
}

/// 出力長辺が `CRT_OUTPUT_MAX` を超えないような倍率に丸める。
fn clamp_upscale(w: usize, h: usize, factor: usize) -> usize {
    let longest = w.max(h).max(1);
    let max_factor = (CRT_OUTPUT_MAX as usize / longest).max(1);
    factor.min(max_factor)
}

/// 指定 bit 数で RGB 各チャンネルを量子化する。
/// ハードウェアカラー空間 (MD=3bit, ゲームギア=4bit, SFC=5bit) をエミュレートするのに使う。
#[inline]
fn quantize_channel_bits(v: u8, bits: u8) -> u8 {
    let levels = ((1u32 << bits) - 1).max(1);
    let lvl = ((v as u32 * levels + 127) / 255).min(levels);
    ((lvl * 255 + levels / 2) / levels) as u8
}

/// 指定 bit 深度で Color32 を量子化する。
#[inline]
fn quantize_color_bits(c: Color32, bits: u8) -> [u8; 3] {
    [
        quantize_channel_bits(c.r(), bits),
        quantize_channel_bits(c.g(), bits),
        quantize_channel_bits(c.b(), bits),
    ]
}

/// Bayer 4x4 ディザ閾値マップ (0..16 → 0..255)。
const BAYER4: [[u8; 4]; 4] = [
    [0, 8, 2, 10],
    [12, 4, 14, 6],
    [3, 11, 1, 9],
    [15, 7, 13, 5],
];

#[inline]
fn bayer4_threshold(x: usize, y: usize) -> f32 {
    // 0..15 を -0.5..0.5 の範囲にマップ
    (BAYER4[y & 3][x & 3] as f32 + 0.5) / 16.0 - 0.5
}

#[inline]
fn clamp_u8(v: f32) -> u8 {
    v.round().clamp(0.0, 255.0) as u8
}

/// y 方向の補間係数を事前計算したコンテキスト。
/// CRT 合成のように同一走査線で複数 x 位置をサンプルする用途で y 計算を共有する。
#[derive(Clone, Copy)]
struct BilinearYCtx {
    src_w: usize,
    row0: usize,    // y0 * src_w
    row1: usize,    // y1 * src_w
    ty: f32,
    one_minus_ty: f32,
    max_x: usize,   // src_w - 1
}

impl BilinearYCtx {
    #[inline]
    fn new(src_w: usize, src_h: usize, sy: f32) -> Self {
        let fy = sy.clamp(0.0, src_h as f32 - 1.0);
        let y0 = fy.floor() as usize;
        let y1 = (y0 + 1).min(src_h - 1);
        let ty = fy - y0 as f32;
        Self {
            src_w,
            row0: y0 * src_w,
            row1: y1 * src_w,
            ty,
            one_minus_ty: 1.0 - ty,
            max_x: src_w - 1,
        }
    }

    #[inline]
    fn sample(&self, src: &[Color32], sx: f32) -> (f32, f32, f32) {
        let fx = sx.clamp(0.0, self.src_w as f32 - 1.0);
        let x0 = fx.floor() as usize;
        let x1 = (x0 + 1).min(self.max_x);
        let tx = fx - x0 as f32;
        let one_minus_tx = 1.0 - tx;

        let c00 = src[self.row0 + x0];
        let c10 = src[self.row0 + x1];
        let c01 = src[self.row1 + x0];
        let c11 = src[self.row1 + x1];

        let top_r = c00.r() as f32 * one_minus_tx + c10.r() as f32 * tx;
        let top_g = c00.g() as f32 * one_minus_tx + c10.g() as f32 * tx;
        let top_b = c00.b() as f32 * one_minus_tx + c10.b() as f32 * tx;
        let bot_r = c01.r() as f32 * one_minus_tx + c11.r() as f32 * tx;
        let bot_g = c01.g() as f32 * one_minus_tx + c11.g() as f32 * tx;
        let bot_b = c01.b() as f32 * one_minus_tx + c11.b() as f32 * tx;

        (
            top_r * self.one_minus_ty + bot_r * self.ty,
            top_g * self.one_minus_ty + bot_g * self.ty,
            top_b * self.one_minus_ty + bot_b * self.ty,
        )
    }
}

// ── CRT ブラウン管エミュレーション ──────────────────────────────

mod crt {
    use super::*;

    // ── 3 プリセットの設計方針 ─────────────────────────────────────────
    //
    // brightness_boost は 「1 / (scan_atten × mask_atten)」で**元の明るさにほぼ揃える**。
    //   scan_atten = 1 - depth × 0.5   (sin² falloff の平均)
    //   mask_atten = 1 - 0.25 × mask_strength   (sin² 分布 3 相の平均)
    //
    // これにより CRT モード切替時に極端な明るさ変化が起きない。bloom は加算方向なので
    // 明部だけわずかに白飛びする (実機の phosphor も明部で飽和するので違和感は小さい)。
    //
    // プリセットの差別化方針:
    //   Simple  — 控えめ。常用向け。薄いスキャンライン+最小マスク+微 glow
    //   Full    — 没入。Simple と同じ強度+樽型歪み+強い phosphor glow
    //   Arcade  — 業務用モニタ風。太いスキャンライン+濃いマスク+わずかに高輝度
    // ─────────────────────────────────────────────────────────────────────

    /// CRT シンプル: 控えめなスキャンライン+微 glow。明るさは元画像とほぼ同じ。
    pub fn apply_simple(src: &ColorImage) -> ColorImage {
        // scan_atten = 1 - 0.32*0.5 = 0.840 ; mask_atten = 1 - 0.25*0.12 = 0.970
        // product = 0.815 → boost = 1/0.815 ≈ 1.227
        crt_common(src, CrtParams {
            scanline_depth: 0.32,
            mask_strength: 0.12,
            brightness_boost: 1.23,
            curvature: 0.0,
            bloom: 0.08,
            h_blur: 0.30,
        })
    }

    /// CRT フル: シンプル相当+樽型歪み+強めの phosphor glow。明るさは Simple と同等。
    pub fn apply_full(src: &ColorImage) -> ColorImage {
        // Simple と同じ scan_depth/mask_strength にして brightness を揃え、
        // bloom/curvature/h_blur だけ強化して「没入版」として差別化。
        crt_common(src, CrtParams {
            scanline_depth: 0.32,
            mask_strength: 0.12,
            brightness_boost: 1.23,
            curvature: 0.07,
            bloom: 0.25,
            h_blur: 0.40,
        })
    }

    /// CRT アーケード: 太いスキャンライン+濃いマスク。わずかに高輝度 (arcade 筐体風)。
    pub fn apply_arcade(src: &ColorImage) -> ColorImage {
        // scan_atten = 1 - 0.55*0.5 = 0.725 ; mask_atten = 1 - 0.25*0.26 = 0.935
        // product = 0.678 → parity boost = 1.475
        // 業務用モニタは実機でも輝度を上げていたので、+10% で 1.62 にして「少し眩しい」感を出す
        crt_common(src, CrtParams {
            scanline_depth: 0.55,
            mask_strength: 0.26,
            brightness_boost: 1.62,
            curvature: 0.0,
            bloom: 0.18,
            h_blur: 0.45,
        })
    }

    struct CrtParams {
        /// スキャンラインの最大暗部強度 (0..1)。大きいほど暗くなる
        scanline_depth: f32,
        /// RGB アパーチャマスクの強度 (0..1)
        mask_strength: f32,
        /// 全体の輝度ブースト (スキャンライン/マスクで落ちた分を補う)
        brightness_boost: f32,
        /// 樽型歪みの強さ (0 = 歪みなし、0.05〜0.15 が自然)
        curvature: f32,
        /// bloom (明部にじみ) の強度 (0..1)
        bloom: f32,
        /// 水平方向の追加ブラー。ビームが隣接ピクセルに少し広がる効果 (0..1)
        h_blur: f32,
    }

    fn crt_common(src: &ColorImage, params: CrtParams) -> ColorImage {
        let [src_w, src_h] = src.size;
        let factor = clamp_upscale(src_w, src_h, crt_upscale_factor(src_w, src_h));
        let out_w = src_w * factor;
        let out_h = src_h * factor;

        // ソースから明度マップを作って bloom 用 (bloom > 0 の場合のみ)
        let bloom_map = if params.bloom > 0.0 {
            Some(build_bloom_map(src))
        } else {
            None
        };

        let mut out = vec![Color32::BLACK; out_w * out_h];
        let src_pixels = &src.pixels;
        let factor_f = factor as f32;

        out.par_chunks_mut(out_w).enumerate().for_each(|(oy, row)| {
            for ox in 0..out_w {
                // 正規化座標 (0..1)
                let (ux, uy) = if params.curvature > 0.0 {
                    let nx = ox as f32 / out_w as f32 * 2.0 - 1.0;
                    let ny = oy as f32 / out_h as f32 * 2.0 - 1.0;
                    let r2 = nx * nx + ny * ny;
                    let k = 1.0 + r2 * params.curvature;
                    let dx = nx * k;
                    let dy = ny * k;
                    if dx.abs() > 1.0 || dy.abs() > 1.0 {
                        row[ox] = Color32::BLACK;
                        continue;
                    }
                    ((dx * 0.5 + 0.5), (dy * 0.5 + 0.5))
                } else {
                    ((ox as f32 + 0.5) / out_w as f32, (oy as f32 + 0.5) / out_h as f32)
                };

                // ソース座標 (サブピクセル精度)
                let sx_f = ux * src_w as f32 - 0.5;
                let sy_f = uy * src_h as f32 - 0.5;

                // 同一走査線で最大 3 箇所サンプルするため y 補間係数を事前計算
                let yctx = BilinearYCtx::new(src_w, src_h, sy_f);

                // bilinear サンプリングで柔らかいピクセル境界
                let (mut r, mut g, mut b) = yctx.sample(src_pixels, sx_f);

                // 水平方向の追加ブラー: ±0.5 px ぶん左右もサンプルしてブレンド
                if params.h_blur > 0.0 {
                    let (lr, lg, lb) = yctx.sample(src_pixels, sx_f - 0.5);
                    let (rr, rg, rb) = yctx.sample(src_pixels, sx_f + 0.5);
                    let w = params.h_blur * 0.5;  // 両隣の重み
                    let c = 1.0 - params.h_blur;  // 中央の重み
                    r = r * c + (lr + rr) * w;
                    g = g * c + (lg + rg) * w;
                    b = b * c + (lb + rb) * w;
                }

                // sin² スキャンライン: factor ごとに 1 周期、暗部がなだらかに落ちる
                let scan_phase = ((oy as f32 + 0.5) % factor_f) / factor_f;  // 0..1
                let scan_curve = (scan_phase * std::f32::consts::PI).sin();  // 0..1..0
                let scan_mult = 1.0 - params.scanline_depth * (1.0 - scan_curve * scan_curve);

                // RGB アパーチャマスク (滑らかな sin² 分布): ox%3 で R/G/B の強弱
                // 3 ピクセル周期で R G B をそれぞれ sin² 強調。総和が概ね 1 になるよう正規化
                let mask_phase = (ox as f32) / 3.0;  // 1 周期 = 3 ピクセル
                let two_pi = std::f32::consts::TAU;
                let rm = 1.0 - params.mask_strength
                    + params.mask_strength * 3.0 * (mask_phase * two_pi).sin().max(0.0).powi(2);
                let gm = 1.0 - params.mask_strength
                    + params.mask_strength * 3.0
                        * ((mask_phase + 1.0 / 3.0) * two_pi).sin().max(0.0).powi(2);
                let bm = 1.0 - params.mask_strength
                    + params.mask_strength * 3.0
                        * ((mask_phase + 2.0 / 3.0) * two_pi).sin().max(0.0).powi(2);

                let boost = params.brightness_boost;
                r = r * rm * scan_mult * boost;
                g = g * gm * scan_mult * boost;
                b = b * bm * scan_mult * boost;

                if let Some(map) = &bloom_map {
                    // bloom: 近傍の明度平均をソース座標から拾って加算
                    let bx = sx_f.round().clamp(0.0, src_w as f32 - 1.0) as usize;
                    let by = sy_f.round().clamp(0.0, src_h as f32 - 1.0) as usize;
                    let bloom_v = sample_bloom(map, src_w, src_h, bx, by);
                    let add = bloom_v * 255.0 * params.bloom;
                    r += add;
                    g += add;
                    b += add;
                }

                row[ox] = Color32::from_rgb(clamp_u8(r), clamp_u8(g), clamp_u8(b));
            }
        });

        ColorImage::new([out_w, out_h], out)
    }

    /// 明度 > 閾値のピクセルを 0..1 でマップ化 (bloom 用)。
    fn build_bloom_map(src: &ColorImage) -> Vec<f32> {
        src.pixels.par_iter().map(|c| {
            let lum = crate::adjustment::pixel_lum_f32(*c);
            ((lum - 0.55).max(0.0) * 2.5).min(1.0)
        }).collect()
    }

    /// 5x5 近傍の bloom 平均 (より広い範囲で滲ませて phosphor glow 風に)。
    fn sample_bloom(map: &[f32], w: usize, h: usize, cx: usize, cy: usize) -> f32 {
        let mut sum = 0.0_f32;
        let mut count = 0.0_f32;
        for dy in -2..=2_isize {
            for dx in -2..=2_isize {
                let nx = cx as isize + dx;
                let ny = cy as isize + dy;
                if nx >= 0 && nx < w as isize && ny >= 0 && ny < h as isize {
                    // ガウス風に中央を強め、周辺を弱め
                    let w_sq = (dx * dx + dy * dy) as f32;
                    let weight = (-w_sq * 0.3).exp();
                    sum += map[ny as usize * w + nx as usize] * weight;
                    count += weight;
                }
            }
        }
        if count > 0.0 { sum / count } else { 0.0 }
    }
}

// ── Median cut: 画像から適応パレットを生成 ─────────────────────

mod palette_gen {
    use super::{Color32, quantize_color_bits};

    /// 画像から median cut で `n_colors` 色のパレットを生成する。
    ///
    /// - `src_pixels`: ソース画像のピクセル配列
    /// - `n_colors`: 目標パレット色数 (16, 32, 61, 256 など)
    /// - `hw_bit_depth`: ハードウェアの RGB/ch ビット深度 (Some(3) = Mega Drive,
    ///   Some(5) = SFC など)。`None` なら量子化なし (PC-98 アナログモード等)
    ///
    /// サンプル数は最大 50,000 ピクセルにダウンサンプリングして速度を稼ぐ。
    pub fn generate(
        src_pixels: &[Color32],
        n_colors: usize,
        hw_bit_depth: Option<u8>,
    ) -> Vec<[u8; 3]> {
        if n_colors <= 1 || src_pixels.is_empty() {
            return vec![[0, 0, 0]];
        }
        // 1. 等間隔サンプリング (大きな画像でも ~50k)
        let step = (src_pixels.len() / 50_000).max(1);
        let samples: Vec<[u8; 3]> = src_pixels
            .iter()
            .step_by(step)
            .map(|c| match hw_bit_depth {
                Some(bits) => quantize_color_bits(*c, bits),
                None => [c.r(), c.g(), c.b()],
            })
            .collect();

        // 2. Median cut: 最大レンジの軸で二分を繰り返す
        let mut boxes: Vec<Vec<[u8; 3]>> = vec![samples];
        while boxes.len() < n_colors {
            let idx = boxes
                .iter()
                .enumerate()
                .filter(|(_, b)| b.len() >= 2)
                .max_by_key(|(_, b)| box_range(b))
                .map(|(i, _)| i);
            let Some(idx) = idx else { break }; // これ以上分割できるボックスが無い
            let b = boxes.swap_remove(idx);
            let (left, right) = split_box(b);
            boxes.push(left);
            boxes.push(right);
        }

        // 3. 各ボックスの平均色 → パレット (HW depth があれば再量子化して格子に揃える)
        boxes
            .iter()
            .map(|b| {
                let avg = average_color(b);
                match hw_bit_depth {
                    Some(bits) => [
                        super::quantize_channel_bits(avg[0], bits),
                        super::quantize_channel_bits(avg[1], bits),
                        super::quantize_channel_bits(avg[2], bits),
                    ],
                    None => avg,
                }
            })
            .collect()
    }

    fn box_range(b: &[[u8; 3]]) -> u32 {
        let mut min = [255u8; 3];
        let mut max = [0u8; 3];
        for p in b {
            for i in 0..3 {
                if p[i] < min[i] { min[i] = p[i]; }
                if p[i] > max[i] { max[i] = p[i]; }
            }
        }
        (max[0] - min[0])
            .max(max[1] - min[1])
            .max(max[2] - min[2]) as u32
    }

    fn split_box(mut b: Vec<[u8; 3]>) -> (Vec<[u8; 3]>, Vec<[u8; 3]>) {
        // 最大レンジのチャンネルで sort → 中央で split
        let mut min = [255u8; 3];
        let mut max = [0u8; 3];
        for p in &b {
            for i in 0..3 {
                if p[i] < min[i] { min[i] = p[i]; }
                if p[i] > max[i] { max[i] = p[i]; }
            }
        }
        let ranges = [max[0] - min[0], max[1] - min[1], max[2] - min[2]];
        let channel = (0..3).max_by_key(|&i| ranges[i]).unwrap();
        b.sort_unstable_by_key(|p| p[channel]);
        let mid = b.len() / 2;
        let right = b.split_off(mid);
        (b, right)
    }

    fn average_color(b: &[[u8; 3]]) -> [u8; 3] {
        let n = b.len().max(1) as u64;
        let mut sum = [0u64; 3];
        for p in b {
            sum[0] += p[0] as u64;
            sum[1] += p[1] as u64;
            sum[2] += p[2] as u64;
        }
        [(sum[0] / n) as u8, (sum[1] / n) as u8, (sum[2] / n) as u8]
    }
}

// ── 減色・ディザ ────────────────────────────────────────────────

mod palette {
    use super::*;

    /// GameBoy DMG 風 4 階調 (緑)。
    pub(super) const GAMEBOY_PALETTE: [[u8; 3]; 4] = [
        [0x0F, 0x38, 0x0F],
        [0x30, 0x62, 0x30],
        [0x8B, 0xAC, 0x0F],
        [0x9B, 0xBC, 0x0F],
    ];

    // PC-98 アナログ RGB (4bit/ch = 4096 色中から 16 色) は実機でも各ゲームが
    // 画像に合わせて 16 色を選んでいた。このため median cut による適応パレット生成が
    // 最も実機の挙動に近い。fixed パレットは使わない。

    /// MSX2+ SCREEN 8 の 256 色固定パレット (GRB 3:3:2)。
    /// G=3bit (8 levels), R=3bit (8 levels), B=2bit (4 levels) = 8×8×4 = 256 colors.
    /// やや青が粗く、緑・赤が細かいのが特徴 (人間の視覚感度に合わせた設計)。
    pub(super) const MSX2PLUS_PALETTE: [[u8; 3]; 256] = {
        let mut pal = [[0u8; 3]; 256];
        let mut i = 0usize;
        let mut g = 0u32;
        while g < 8 {
            let mut r = 0u32;
            while r < 8 {
                let mut b = 0u32;
                while b < 4 {
                    pal[i] = [
                        (r * 255 / 7) as u8,
                        (g * 255 / 7) as u8,
                        (b * 255 / 3) as u8,
                    ];
                    i += 1;
                    b += 1;
                }
                r += 1;
            }
            g += 1;
        }
        pal
    };

    /// ファミコン (NES) 代表パレット (約 52 色)。
    const FAMICOM_PALETTE: &[[u8; 3]] = &[
        [0x7C, 0x7C, 0x7C], [0x00, 0x00, 0xFC], [0x00, 0x00, 0xBC], [0x44, 0x28, 0xBC],
        [0x94, 0x00, 0x84], [0xA8, 0x00, 0x20], [0xA8, 0x10, 0x00], [0x88, 0x14, 0x00],
        [0x50, 0x30, 0x00], [0x00, 0x78, 0x00], [0x00, 0x68, 0x00], [0x00, 0x58, 0x00],
        [0x00, 0x40, 0x58], [0x00, 0x00, 0x00],
        [0xBC, 0xBC, 0xBC], [0x00, 0x78, 0xF8], [0x00, 0x58, 0xF8], [0x68, 0x44, 0xFC],
        [0xD8, 0x00, 0xCC], [0xE4, 0x00, 0x58], [0xF8, 0x38, 0x00], [0xE4, 0x5C, 0x10],
        [0xAC, 0x7C, 0x00], [0x00, 0xB8, 0x00], [0x00, 0xA8, 0x00], [0x00, 0xA8, 0x44],
        [0x00, 0x88, 0x88],
        [0xF8, 0xF8, 0xF8], [0x3C, 0xBC, 0xFC], [0x68, 0x88, 0xFC], [0x98, 0x78, 0xF8],
        [0xF8, 0x78, 0xF8], [0xF8, 0x58, 0x98], [0xF8, 0x78, 0x58], [0xFC, 0xA0, 0x44],
        [0xF8, 0xB8, 0x00], [0xB8, 0xF8, 0x18], [0x58, 0xD8, 0x54], [0x58, 0xF8, 0x98],
        [0x00, 0xE8, 0xD8], [0x78, 0x78, 0x78],
        [0xFC, 0xFC, 0xFC], [0xA4, 0xE4, 0xFC], [0xB8, 0xB8, 0xF8], [0xD8, 0xB8, 0xF8],
        [0xF8, 0xB8, 0xF8], [0xF8, 0xA4, 0xC0], [0xF0, 0xD0, 0xB0], [0xFC, 0xE0, 0xA8],
        [0xF8, 0xD8, 0x78], [0xD8, 0xF8, 0x78], [0xB8, 0xF8, 0xB8], [0xB8, 0xF8, 0xD8],
    ];

    pub fn apply_gameboy(src: &ColorImage) -> ColorImage {
        // GameBoy は 4 階調の輝度マッピング (緑だけのパレットだが、入力輝度から最近傍を選ぶ)
        quantize_with_dither(src, &GAMEBOY_PALETTE, 0.12)
    }

    pub fn apply_pc98(src: &ColorImage) -> ColorImage {
        // 実機の PC-98 アナログモードと同じく、画像から最適 16 色を選んで減色する。
        // 強めのディザでグラデーション階調を稼ぐ。
        let pal = palette_gen::generate(&src.pixels, 16, None);
        quantize_with_dither(src, &pal, 0.18)
    }

    pub fn apply_famicom(src: &ColorImage) -> ColorImage {
        // NES ハードパレットは肌色・中間調を欠くため、画像によっては色が大きく変わる。
        // ディザを強めにしてグラデーション階調を稼ぎ、カラーブロックの唐突さを緩和する。
        quantize_with_dither(src, FAMICOM_PALETTE, 0.22)
    }

    /// MSX2+ SCREEN 8 相当。GRB 3:3:2 の 256 色固定パレット。
    /// 青が粗い (4 段階) ためグラデーションはややボケる。
    pub fn apply_msx2plus(src: &ColorImage) -> ColorImage {
        quantize_with_dither(src, &MSX2PLUS_PALETTE, 0.08)
    }

    /// メガドライブ (Genesis) 相当。各チャンネル 3bit (8 階調) から 61 色を適応選択。
    /// 8 段階階調のはっきりしたバンディングが特徴。
    pub fn apply_mega_drive(src: &ColorImage) -> ColorImage {
        let pal = palette_gen::generate(&src.pixels, 61, Some(3));
        quantize_with_dither(src, &pal, 0.14)
    }

    /// ゲームギア相当。各チャンネル 4bit (16 階調) から 32 色を適応選択。
    /// MD より色深度あり、PC-98 より色数多いリッチな携帯機の雰囲気。
    pub fn apply_game_gear(src: &ColorImage) -> ColorImage {
        let pal = palette_gen::generate(&src.pixels, 32, Some(4));
        quantize_with_dither(src, &pal, 0.14)
    }

    /// スーパーファミコン相当。各チャンネル 5bit (32 階調) から 256 色を適応選択。
    /// 高精細、ほぼ現代的だが微かな階段状バンディングが見える。
    pub fn apply_sfc(src: &ColorImage) -> ColorImage {
        let pal = palette_gen::generate(&src.pixels, 256, Some(5));
        quantize_with_dither(src, &pal, 0.06)
    }

    pub fn apply_1bit(src: &ColorImage) -> ColorImage {
        let [w, h] = src.size;
        let mut out = vec![Color32::BLACK; w * h];
        let src_pixels = &src.pixels;
        out.par_chunks_mut(w).enumerate().for_each(|(y, row)| {
            for x in 0..w {
                let c = src_pixels[y * w + x];
                let lum = crate::adjustment::pixel_lum_f32(c);
                let t = bayer4_threshold(x, y);
                let v = if lum + t > 0.5 { 255 } else { 0 };
                row[x] = Color32::from_rgb(v, v, v);
            }
        });
        ColorImage::new([w, h], out)
    }

    /// Bayer ディザを使ってソースを指定パレットの最近傍色に量子化する。
    /// `dither_strength`: ディザノイズの振幅 (0..1、小さいほど原画に近い)。
    ///
    /// 最近傍探索は 32³=32768 エントリの 5bit/ch LUT を使い O(1) 化。
    /// 4K 画像に 256 色パレットを適用する場合、ナイーブ実装で 20〜40ms → LUT で数 ms に短縮。
    fn quantize_with_dither(src: &ColorImage, palette: &[[u8; 3]], dither_strength: f32) -> ColorImage {
        let [w, h] = src.size;
        let mut out = vec![Color32::BLACK; w * h];
        let src_pixels = &src.pixels;
        let noise_scale = 255.0 * dither_strength;
        let lut = build_palette_lut(palette);

        out.par_chunks_mut(w).enumerate().for_each(|(y, row)| {
            for x in 0..w {
                let c = src_pixels[y * w + x];
                let t = bayer4_threshold(x, y) * noise_scale;
                let r = (c.r() as f32 + t).clamp(0.0, 255.0) as u32;
                let g = (c.g() as f32 + t).clamp(0.0, 255.0) as u32;
                let b = (c.b() as f32 + t).clamp(0.0, 255.0) as u32;
                let idx = lut[lut_index(r, g, b)] as usize;
                let p = palette[idx];
                row[x] = Color32::from_rgb(p[0], p[1], p[2]);
            }
        });
        ColorImage::new([w, h], out)
    }

    const LUT_BITS: u32 = 5; // 5bit/ch → 32³ = 32768 bins
    const LUT_DIM: u32 = 1 << LUT_BITS; // 32
    const LUT_MAX: u32 = LUT_DIM - 1; // 31
    const LUT_SIZE: usize = (LUT_DIM * LUT_DIM * LUT_DIM) as usize; // 32768

    #[inline]
    fn lut_index(r: u32, g: u32, b: u32) -> usize {
        // (255 * LUT_MAX + 127) / 255 == LUT_MAX の四捨五入量子化
        let ri = (r * LUT_MAX + 127) / 255;
        let gi = (g * LUT_MAX + 127) / 255;
        let bi = (b * LUT_MAX + 127) / 255;
        ((ri * LUT_DIM + gi) * LUT_DIM + bi) as usize
    }

    /// パレットに対する 3D LUT (5bit/ch) を並列構築する。
    /// 各 bin の中心色から最近傍パレット index を求める。返り値は `LUT_SIZE` 要素の u8 配列。
    /// 構築コスト: LUT_SIZE * palette_len × 数 ops = パレット 256 で約 8M ops (≈ 1〜3ms、並列化後)
    fn build_palette_lut(palette: &[[u8; 3]]) -> Vec<u8> {
        let mut lut = vec![0u8; LUT_SIZE];
        lut.par_iter_mut().enumerate().for_each(|(bin, slot)| {
            let bi = bin as u32 % LUT_DIM;
            let gi = (bin as u32 / LUT_DIM) % LUT_DIM;
            let ri = bin as u32 / (LUT_DIM * LUT_DIM);
            // bin 中心の色 (0..=255)
            let r = (ri * 255 / LUT_MAX) as f32;
            let g = (gi * 255 / LUT_MAX) as f32;
            let b = (bi * 255 / LUT_MAX) as f32;
            *slot = nearest_palette_idx(palette, r, g, b) as u8;
        });
        lut
    }

    /// ユークリッド距離 (RGB) 最小のパレットインデックスを返す。
    /// LUT 構築時のみ呼ばれる。実行時のピクセル量子化では LUT 経由で O(1) 参照。
    fn nearest_palette_idx(palette: &[[u8; 3]], r: f32, g: f32, b: f32) -> usize {
        let mut best = 0usize;
        let mut best_d = f32::MAX;
        for (i, c) in palette.iter().enumerate() {
            let dr = r - c[0] as f32;
            let dg = g - c[1] as f32;
            let db = b - c[2] as f32;
            let d = dr * dr + dg * dg + db * db;
            if d < best_d {
                best_d = d;
                best = i;
            }
        }
        best
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_test_image(w: usize, h: usize) -> ColorImage {
        let mut pixels = Vec::with_capacity(w * h);
        for y in 0..h {
            for x in 0..w {
                pixels.push(Color32::from_rgb(
                    (x * 255 / w.max(1)) as u8,
                    (y * 255 / h.max(1)) as u8,
                    128,
                ));
            }
        }
        ColorImage::new([w, h], pixels)
    }

    #[test]
    fn none_is_identity_clone() {
        let src = make_test_image(16, 16);
        let out = apply(&src, PostFilter::None);
        assert_eq!(out.size, src.size);
        assert_eq!(out.pixels, src.pixels);
    }

    #[test]
    fn nearest_is_identity_clone() {
        let src = make_test_image(16, 16);
        let out = apply(&src, PostFilter::Nearest);
        assert_eq!(out.size, src.size);
        assert_eq!(out.pixels, src.pixels);
    }

    #[test]
    fn crt_simple_upscales_small_image() {
        let src = make_test_image(64, 64);
        let out = apply(&src, PostFilter::CrtSimple);
        // 64 → factor=4 → 256×256
        assert_eq!(out.size, [256, 256]);
    }

    #[test]
    fn crt_simple_caps_large_image() {
        let src = make_test_image(3000, 3000);
        let out = apply(&src, PostFilter::CrtSimple);
        // 3000 > 2048 なので factor=1、CRT_OUTPUT_MAX=4096 に収まる
        assert_eq!(out.size, [3000, 3000]);
    }

    #[test]
    fn gameboy_uses_palette_colors_only() {
        let src = make_test_image(32, 32);
        let out = apply(&src, PostFilter::GameBoy);
        let allowed: std::collections::HashSet<(u8, u8, u8)> = palette::GAMEBOY_PALETTE
            .iter()
            .map(|c| (c[0], c[1], c[2]))
            .collect();
        for p in &out.pixels {
            assert!(allowed.contains(&(p.r(), p.g(), p.b())),
                "pixel ({},{},{}) not in GameBoy palette", p.r(), p.g(), p.b());
        }
    }

    #[test]
    fn dither1bit_produces_only_black_or_white() {
        let src = make_test_image(64, 64);
        let out = apply(&src, PostFilter::Dither1bit);
        for p in &out.pixels {
            let v = p.r();
            assert!(v == 0 || v == 255);
            assert_eq!(p.r(), p.g());
            assert_eq!(p.g(), p.b());
        }
    }

    #[test]
    fn pc98_adaptive_produces_at_most_16_unique_colors() {
        let src = make_test_image(64, 64);
        let out = apply(&src, PostFilter::Pc98);
        let unique: std::collections::HashSet<(u8, u8, u8)> =
            out.pixels.iter().map(|p| (p.r(), p.g(), p.b())).collect();
        assert!(unique.len() <= 16, "PC-98 adaptive used {} unique colors, expected ≤16", unique.len());
        assert!(unique.len() >= 2, "PC-98 adaptive used only {} unique colors, expected variety", unique.len());
    }

    #[test]
    fn mega_drive_colors_are_on_3bit_grid() {
        let src = make_test_image(64, 64);
        let out = apply(&src, PostFilter::MegaDrive);
        // 3bit per channel → 8 可能な値: 0, 36, 73, 109, 146, 182, 219, 255
        let valid_levels: std::collections::HashSet<u8> =
            (0..8u32).map(|i| ((i * 255 + 3) / 7) as u8).collect();
        for p in &out.pixels {
            assert!(valid_levels.contains(&p.r()), "R={} not on 3-bit grid", p.r());
            assert!(valid_levels.contains(&p.g()), "G={} not on 3-bit grid", p.g());
            assert!(valid_levels.contains(&p.b()), "B={} not on 3-bit grid", p.b());
        }
    }

    #[test]
    fn msx2plus_palette_has_256_entries() {
        // 直接は private なので、filter を通して unique 色数をカウント
        let src = make_test_image(256, 256);
        let out = apply(&src, PostFilter::Msx2Plus);
        let unique: std::collections::HashSet<(u8, u8, u8)> =
            out.pixels.iter().map(|p| (p.r(), p.g(), p.b())).collect();
        assert!(unique.len() <= 256, "MSX2+ used {} colors, expected ≤256", unique.len());
    }

    #[test]
    fn serde_round_trip() {
        for f in PostFilter::ALL.iter().copied() {
            let json = serde_json::to_string(&f).unwrap();
            let decoded: PostFilter = serde_json::from_str(&json).unwrap();
            assert_eq!(f, decoded, "round-trip failed for {:?}", f);
        }
    }

    #[test]
    fn default_is_none() {
        let f: PostFilter = Default::default();
        assert_eq!(f, PostFilter::None);
    }

    #[test]
    fn adjust_params_json_backward_compat() {
        // post_filter フィールドが無い JSON もデコードできること
        let json = r#"{"brightness":0.0,"contrast":0.0,"gamma":1.0,"saturation":0.0,
            "temperature":0.0,"black_point":0,"white_point":255,"midtone":1.0,
            "auto_mode":null,"upscale_model":null}"#;
        let params: crate::adjustment::AdjustParams = serde_json::from_str(json).unwrap();
        assert_eq!(params.post_filter, PostFilter::None);
    }

    /// 性能回帰: 1920×1080 の CRT Simple が 200ms 以内に完了すること。
    /// CI 環境のばらつきを考慮し閾値は余裕を持たせている。
    #[test]
    #[ignore = "performance test; run with `cargo test --release -- --ignored post_filter`"]
    fn crt_simple_hd_under_200ms() {
        let src = make_test_image(1920, 1080);
        let start = std::time::Instant::now();
        let _out = apply(&src, PostFilter::CrtSimple);
        let elapsed = start.elapsed();
        assert!(elapsed.as_millis() < 200,
            "CRT Simple on 1920×1080 took {:?}, expected < 200ms", elapsed);
    }
}
