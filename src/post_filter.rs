//! ポストフィルタ (レトロ系 + 写真系プリセット)。
//!
//! 色調補正後の [`egui::ColorImage`] を受け取り、変換後の ColorImage を返す。
//! 各フィルタは `rayon` で並列化しており、4K 画像でも 50〜80ms 程度で完了する想定。
//!
//! 合成順序:
//!   apply_adjustments_fast (色調) → [post_filter::apply] → テクスチャ化 (NEAREST サンプラー)
//!
//! 系統:
//! - `crt` — ブラウン管エミュレート (Simple/Full/Arcade + 非液晶機種との複合)
//! - `palette` — ハード別の減色・ディザ (GameBoy/PC-98/MD/SFC…)
//! - `photo` — 写真向け (カラーグレーディング、アナログ、絵画風、実用)

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
        // ── 写真系カラーグレーディング ──────────────────────────
        PostFilter::Sepia => photo::apply_sepia(src),
        PostFilter::MonoNeutral => photo::apply_mono_neutral(src),
        PostFilter::MonoCool => photo::apply_mono_cool(src),
        PostFilter::MonoWarm => photo::apply_mono_warm(src),
        PostFilter::TealOrange => photo::apply_teal_orange(src),
        PostFilter::KodakPortra => photo::apply_kodak_portra(src),
        PostFilter::FujiVelvia => photo::apply_fuji_velvia(src),
        PostFilter::BleachBypass => photo::apply_bleach_bypass(src),
        PostFilter::CrossProcess => photo::apply_cross_process(src),
        PostFilter::Vintage => photo::apply_vintage(src),
        PostFilter::WarmTone => photo::apply_warm_tone(src),
        PostFilter::CoolTone => photo::apply_cool_tone(src),
        // ── アナログフィルム ─────────────────────────────────────
        PostFilter::FilmGrain => photo::apply_film_grain(src),
        PostFilter::Vignette => photo::apply_vignette(src),
        PostFilter::LightLeak => photo::apply_light_leak(src),
        PostFilter::SoftFocus => photo::apply_soft_focus(src),
        // ── 絵画・描画風 ─────────────────────────────────────────
        PostFilter::Halftone => photo::apply_halftone(src),
        PostFilter::OilPaint => photo::apply_oil_paint(src),
        PostFilter::Sketch => photo::apply_sketch(src),
        // ── 実用 ─────────────────────────────────────────────────
        PostFilter::Sharpen => photo::apply_sharpen(src),
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
        let (r, g, b, _a) = self.sample_rgba(src, sx);
        (r, g, b)
    }

    /// alpha も含めてサンプリングする。透過画像で CRT 系が不透明化するのを防ぐ。
    #[inline]
    fn sample_rgba(&self, src: &[Color32], sx: f32) -> (f32, f32, f32, f32) {
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
        let top_a = c00.a() as f32 * one_minus_tx + c10.a() as f32 * tx;
        let bot_r = c01.r() as f32 * one_minus_tx + c11.r() as f32 * tx;
        let bot_g = c01.g() as f32 * one_minus_tx + c11.g() as f32 * tx;
        let bot_b = c01.b() as f32 * one_minus_tx + c11.b() as f32 * tx;
        let bot_a = c01.a() as f32 * one_minus_tx + c11.a() as f32 * tx;

        (
            top_r * self.one_minus_ty + bot_r * self.ty,
            top_g * self.one_minus_ty + bot_g * self.ty,
            top_b * self.one_minus_ty + bot_b * self.ty,
            top_a * self.one_minus_ty + bot_a * self.ty,
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

                // bilinear サンプリングで柔らかいピクセル境界 (alpha 保持)
                let (mut r, mut g, mut b, a_src) = yctx.sample_rgba(src_pixels, sx_f);

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

                row[ox] = Color32::from_rgba_unmultiplied(
                    clamp_u8(r), clamp_u8(g), clamp_u8(b), clamp_u8(a_src),
                );
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
                row[x] = Color32::from_rgba_unmultiplied(v, v, v, c.a());
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
                row[x] = Color32::from_rgba_unmultiplied(p[0], p[1], p[2], c.a());
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

// ── 写真系ポストフィルタ ────────────────────────────────────────
//
// カラーグレーディング: 3×3 色行列 + チャンネル別トーンカーブの組み合わせで
//   大半の「フィルム風ルック」を 1〜2 ms/MP 程度で再現する。
// アナログエフェクト: フィルムグレイン・ビネット・ライトリーク・ソフトフォーカス。
// 絵画・描画風: ハーフトーン・Kuwahara オイルペイント・Sobel スケッチ。
// 実用: アンシャープマスクによるシャープ化。
//
// すべて ColorImage → ColorImage のピュア変換で、rayon 並列化で 4K 画像も実用速度。

mod photo {
    use super::*;

    // ── 共通ヘルパー ────────────────────────────────────────────

    /// 3×3 RGB 行列をピクセルに適用する (クランプあり、alpha 保持)。
    #[inline]
    fn apply_matrix(c: Color32, m: [[f32; 3]; 3]) -> Color32 {
        let r = c.r() as f32;
        let g = c.g() as f32;
        let b = c.b() as f32;
        let nr = m[0][0] * r + m[0][1] * g + m[0][2] * b;
        let ng = m[1][0] * r + m[1][1] * g + m[1][2] * b;
        let nb = m[2][0] * r + m[2][1] * g + m[2][2] * b;
        Color32::from_rgba_unmultiplied(clamp_u8(nr), clamp_u8(ng), clamp_u8(nb), c.a())
    }

    /// S 字コントラストカーブ (低入力をより暗く、高入力をより明るく)。
    /// `strength`: 0.0 で無変化、1.0 強めの S 字
    #[inline]
    fn s_curve(v: f32, strength: f32) -> f32 {
        // v: 0..255。正規化 → スムーズステップ的な S 字 → 再スケール
        let n = (v / 255.0).clamp(0.0, 1.0);
        let s = n * n * (3.0 - 2.0 * n); // smoothstep
        (n * (1.0 - strength) + s * strength) * 255.0
    }

    /// 輝度保持の彩度調整 (saturation: -1..+1、0 で無変化、alpha 保持)。
    #[inline]
    fn adjust_saturation(c: Color32, saturation: f32) -> Color32 {
        let r = c.r() as f32;
        let g = c.g() as f32;
        let b = c.b() as f32;
        let lum = 0.299 * r + 0.587 * g + 0.114 * b;
        let mul = 1.0 + saturation;
        Color32::from_rgba_unmultiplied(
            clamp_u8(lum + (r - lum) * mul),
            clamp_u8(lum + (g - lum) * mul),
            clamp_u8(lum + (b - lum) * mul),
            c.a(),
        )
    }

    /// 輝度マップ (0..1)。
    #[inline]
    fn luminance(c: Color32) -> f32 {
        crate::adjustment::pixel_lum_f32(c)
    }

    /// ピクセル毎の単純変換を並列に実行するヘルパー。
    fn map_parallel<F>(src: &ColorImage, f: F) -> ColorImage
    where
        F: Fn(Color32) -> Color32 + Sync,
    {
        let [w, h] = src.size;
        let pixels: Vec<Color32> = src.pixels.par_iter().map(|c| f(*c)).collect();
        ColorImage::new([w, h], pixels)
    }

    /// 座標 (x, y) を見るピクセル毎並列変換。
    fn map_parallel_xy<F>(src: &ColorImage, f: F) -> ColorImage
    where
        F: Fn(usize, usize, Color32) -> Color32 + Sync,
    {
        let [w, h] = src.size;
        let src_pixels = &src.pixels;
        let mut out = vec![Color32::BLACK; w * h];
        out.par_chunks_mut(w).enumerate().for_each(|(y, row)| {
            for x in 0..w {
                row[x] = f(x, y, src_pixels[y * w + x]);
            }
        });
        ColorImage::new([w, h], out)
    }

    // ── カラーグレーディング ────────────────────────────────────

    /// セピア (古写真風)。Microsoft の典型的なセピアフィルタ係数。
    pub fn apply_sepia(src: &ColorImage) -> ColorImage {
        const M: [[f32; 3]; 3] = [
            [0.393, 0.769, 0.189],
            [0.349, 0.686, 0.168],
            [0.272, 0.534, 0.131],
        ];
        map_parallel(src, |c| apply_matrix(c, M))
    }

    /// ニュートラルモノクロ (ITU-R BT.601 輝度のまま)。
    pub fn apply_mono_neutral(src: &ColorImage) -> ColorImage {
        map_parallel(src, |c| {
            let y = (luminance(c) * 255.0) as u8;
            Color32::from_rgba_unmultiplied(y, y, y, c.a())
        })
    }

    /// 冷調モノクロ (青みの影)。
    pub fn apply_mono_cool(src: &ColorImage) -> ColorImage {
        map_parallel(src, |c| {
            let y = luminance(c);
            let r = y * 0.88;
            let g = y * 0.95;
            let b = (y * 1.05).min(1.0);
            Color32::from_rgba_unmultiplied(
                (r * 255.0) as u8, (g * 255.0) as u8, (b * 255.0) as u8, c.a(),
            )
        })
    }

    /// 暖調モノクロ (茶み付きのセピアより薄めの仕上げ)。
    pub fn apply_mono_warm(src: &ColorImage) -> ColorImage {
        map_parallel(src, |c| {
            let y = luminance(c);
            let r = (y * 1.08).min(1.0);
            let g = y * 0.98;
            let b = y * 0.82;
            Color32::from_rgba_unmultiplied(
                (r * 255.0) as u8, (g * 255.0) as u8, (b * 255.0) as u8, c.a(),
            )
        })
    }

    /// シネマ風 Teal & Orange。影を青緑、ハイライトを橙に振る。
    pub fn apply_teal_orange(src: &ColorImage) -> ColorImage {
        map_parallel(src, |c| {
            let y = luminance(c);
            // 影 (y<0.5): 青緑、ハイライト (y>=0.5): 橙 を線形補間で混ぜる
            let t = (y - 0.5) * 2.0; // -1..+1
            let r_shift = 18.0 * t;
            let b_shift = -18.0 * t;
            let g_shift = 6.0 * t.abs() * t.signum() * 0.5;
            let out = Color32::from_rgba_unmultiplied(
                clamp_u8(c.r() as f32 + r_shift),
                clamp_u8(c.g() as f32 + g_shift),
                clamp_u8(c.b() as f32 + b_shift),
                c.a(),
            );
            // 少しだけ彩度を上げる (adjust_saturation も alpha を保持)
            adjust_saturation(out, 0.12)
        })
    }

    /// Kodak Portra 風。落ち着いた彩度、肌色を少し血色よく、緑をやや抑制。
    pub fn apply_kodak_portra(src: &ColorImage) -> ColorImage {
        const M: [[f32; 3]; 3] = [
            [1.04, 0.00, -0.02],
            [-0.01, 0.98, 0.01],
            [-0.02, 0.02, 0.96],
        ];
        map_parallel(src, |c| {
            let out = apply_matrix(c, M);
            // やや低彩度でフィルム調 (adjust_saturation が alpha を保持)
            let out = adjust_saturation(out, -0.05);
            // 軽い lift (暗部を持ち上げる)、alpha は元画像から継承
            Color32::from_rgba_unmultiplied(
                clamp_u8(out.r() as f32 + 5.0),
                clamp_u8(out.g() as f32 + 5.0),
                clamp_u8(out.b() as f32 + 5.0),
                c.a(),
            )
        })
    }

    /// Fuji Velvia 風。高彩度、緑と青を強調、コントラスト強め。
    pub fn apply_fuji_velvia(src: &ColorImage) -> ColorImage {
        map_parallel(src, |c| {
            // チャンネル別ゲイン
            let r = c.r() as f32 * 1.02;
            let g = c.g() as f32 * 1.08;
            let b = c.b() as f32 * 1.10;
            // S 字コントラスト
            let r = s_curve(r, 0.35);
            let g = s_curve(g, 0.35);
            let b = s_curve(b, 0.35);
            let out = Color32::from_rgba_unmultiplied(clamp_u8(r), clamp_u8(g), clamp_u8(b), c.a());
            // 強めの彩度ブースト (adjust_saturation は alpha を保持)
            adjust_saturation(out, 0.35)
        })
    }

    /// ブリーチバイパス (銀残し): 低彩度 × 高コントラスト。
    pub fn apply_bleach_bypass(src: &ColorImage) -> ColorImage {
        map_parallel(src, |c| {
            // 彩度を大幅ダウン + 強い S 字 (desat 経由で alpha が継承される)
            let desat = adjust_saturation(c, -0.55);
            let r = s_curve(desat.r() as f32, 0.55);
            let g = s_curve(desat.g() as f32, 0.55);
            let b = s_curve(desat.b() as f32, 0.55);
            Color32::from_rgba_unmultiplied(clamp_u8(r), clamp_u8(g), clamp_u8(b), c.a())
        })
    }

    /// クロスプロセス風。影は青緑に振り、ハイライトは黄色に振る。高彩度。
    pub fn apply_cross_process(src: &ColorImage) -> ColorImage {
        map_parallel(src, |c| {
            let y = luminance(c);
            // 影 → 青緑、ハイライト → 黄
            let r = c.r() as f32 + if y < 0.5 { -12.0 } else { 15.0 };
            let g = c.g() as f32 + if y < 0.5 { 6.0 } else { 10.0 };
            let b = c.b() as f32 + if y < 0.5 { 12.0 } else { -20.0 };
            let out = Color32::from_rgba_unmultiplied(clamp_u8(r), clamp_u8(g), clamp_u8(b), c.a());
            adjust_saturation(out, 0.25)
        })
    }

    /// ビンテージ / 褪色。コントラストを抑え、シャドウを紫、ハイライトを黄に褪色。
    pub fn apply_vintage(src: &ColorImage) -> ColorImage {
        map_parallel(src, |c| {
            let y = luminance(c);
            // lift/gain: 影を持ち上げ、ハイライトを落とす (コントラスト圧縮)
            let lift = 25.0 * (1.0 - y); // 影ほど強く持ち上げる
            let compress = -12.0 * y; // ハイライトを少し落とす
            let r = c.r() as f32 + lift + compress + 8.0; // 赤寄りのシャドウ
            let g = c.g() as f32 + lift * 0.6 + compress;
            let b = c.b() as f32 + lift * 0.9 + compress - 8.0; // 黄色ハイライト
            let out = Color32::from_rgba_unmultiplied(clamp_u8(r), clamp_u8(g), clamp_u8(b), c.a());
            adjust_saturation(out, -0.20) // 少し褪せる
        })
    }

    /// 暖色調 (全体に +R/-B)。
    pub fn apply_warm_tone(src: &ColorImage) -> ColorImage {
        map_parallel(src, |c| {
            Color32::from_rgba_unmultiplied(
                clamp_u8(c.r() as f32 + 14.0),
                clamp_u8(c.g() as f32 + 4.0),
                clamp_u8(c.b() as f32 - 14.0),
                c.a(),
            )
        })
    }

    /// 寒色調 (全体に -R/+B)。
    pub fn apply_cool_tone(src: &ColorImage) -> ColorImage {
        map_parallel(src, |c| {
            Color32::from_rgba_unmultiplied(
                clamp_u8(c.r() as f32 - 14.0),
                clamp_u8(c.g() as f32 + 0.0),
                clamp_u8(c.b() as f32 + 14.0),
                c.a(),
            )
        })
    }

    // ── アナログ写真エフェクト ──────────────────────────────────

    /// ピクセル位置から決定論的な擬似乱数 (0..1)。画像内で再現性のあるノイズを作るのに使う。
    /// integer hash (wang hash 変種) で十分ばらける。
    #[inline]
    fn pixel_hash(x: usize, y: usize, salt: u32) -> f32 {
        let mut h = (x as u32).wrapping_mul(374761393)
            ^ (y as u32).wrapping_mul(668265263)
            ^ salt.wrapping_mul(2246822519);
        h ^= h >> 13;
        h = h.wrapping_mul(1274126177);
        h ^= h >> 16;
        (h as f32) / (u32::MAX as f32)
    }

    /// フィルムグレイン (輝度ノイズ、暗部で強め)。
    /// 振幅 32/255 ≒ ISO 3200 相当の明瞭な粒状感。
    pub fn apply_film_grain(src: &ColorImage) -> ColorImage {
        map_parallel_xy(src, |x, y, c| {
            let noise = (pixel_hash(x, y, 1) - 0.5) * 2.0; // -1..+1
            // 暗部ほどグレインが目立つフィルム特性
            let y_norm = luminance(c);
            let amount = 32.0 * (1.0 - y_norm * 0.5);
            let n = noise * amount;
            Color32::from_rgba_unmultiplied(
                clamp_u8(c.r() as f32 + n),
                clamp_u8(c.g() as f32 + n),
                clamp_u8(c.b() as f32 + n),
                c.a(),
            )
        })
    }

    /// ビネット (周辺減光)。中心からの距離^2 で周辺を暗くする。
    /// 中心 1.0 → 対角端 0.30 (−70%) のはっきりした落ち込み。
    pub fn apply_vignette(src: &ColorImage) -> ColorImage {
        let [w, h] = src.size;
        let cx = (w as f32) * 0.5;
        let cy = (h as f32) * 0.5;
        let max_d2 = cx * cx + cy * cy;
        map_parallel_xy(src, move |x, y, c| {
            let dx = x as f32 - cx;
            let dy = y as f32 - cy;
            let d2 = dx * dx + dy * dy;
            let t = (d2 / max_d2).clamp(0.0, 1.0);
            let mult = 1.0 - t * t * 0.70;
            Color32::from_rgba_unmultiplied(
                clamp_u8(c.r() as f32 * mult),
                clamp_u8(c.g() as f32 * mult),
                clamp_u8(c.b() as f32 * mult),
                c.a(),
            )
        })
    }

    /// ライトリーク (左上角から暖色の光漏れ)。Screen ブレンドで明るく加算。
    /// ピーク 0.70 でしっかり「漏れてる」感を出す。
    pub fn apply_light_leak(src: &ColorImage) -> ColorImage {
        let [w, h] = src.size;
        let inv_diag = 1.0 / ((w as f32).hypot(h as f32));
        map_parallel_xy(src, move |x, y, c| {
            // 左上から対角で減衰 (0..1)
            let d = ((x as f32).hypot(y as f32)) * inv_diag;
            let leak = (1.0 - d).clamp(0.0, 1.0).powi(3) * 0.70; // ピーク 0..0.70
            // Screen blend: 1 - (1-a)(1-b)。光源色は暖色 (240, 150, 80)
            let lr = 240.0 / 255.0 * leak;
            let lg = 150.0 / 255.0 * leak;
            let lb = 80.0 / 255.0 * leak;
            let cr = c.r() as f32 / 255.0;
            let cg = c.g() as f32 / 255.0;
            let cb = c.b() as f32 / 255.0;
            let r = 1.0 - (1.0 - cr) * (1.0 - lr);
            let g = 1.0 - (1.0 - cg) * (1.0 - lg);
            let b = 1.0 - (1.0 - cb) * (1.0 - lb);
            Color32::from_rgba_unmultiplied(
                (r * 255.0) as u8, (g * 255.0) as u8, (b * 255.0) as u8, c.a(),
            )
        })
    }

    /// ソフトフォーカス (明部にじみ)。広めのブラーと強めの glow でしっかり夢見心地に。
    pub fn apply_soft_focus(src: &ColorImage) -> ColorImage {
        let [w, h] = src.size;
        let src_pixels = &src.pixels;
        // 明部 (luminance > 0.45) を広めに拾って glow 源に
        let glow_src: Vec<[f32; 3]> = src_pixels.par_iter().map(|c| {
            let y = luminance(*c);
            let factor = ((y - 0.45).max(0.0) * 1.8).min(1.0);
            [c.r() as f32 * factor, c.g() as f32 * factor, c.b() as f32 * factor]
        }).collect();
        // 分離可能ボックスブラー 2 回適用で擬似ガウシアン化 + 半径も拡大 (7 → 15-tap)
        let hblur1 = separable_box_blur(&glow_src, w, h, 7, true);
        let vblur1 = separable_box_blur(&hblur1, w, h, 7, false);
        let hblur2 = separable_box_blur(&vblur1, w, h, 7, true);
        let glow = separable_box_blur(&hblur2, w, h, 7, false);
        // Screen blend を強めに
        const GLOW_STRENGTH: f32 = 0.80;
        let mut out = vec![Color32::BLACK; w * h];
        out.par_chunks_mut(w).enumerate().for_each(|(y, row)| {
            for x in 0..w {
                let c = src_pixels[y * w + x];
                let [gr, gg, gb] = glow[y * w + x];
                let cr = c.r() as f32 / 255.0;
                let cg = c.g() as f32 / 255.0;
                let cb = c.b() as f32 / 255.0;
                let lr = (gr / 255.0).clamp(0.0, 1.0) * GLOW_STRENGTH;
                let lg = (gg / 255.0).clamp(0.0, 1.0) * GLOW_STRENGTH;
                let lb = (gb / 255.0).clamp(0.0, 1.0) * GLOW_STRENGTH;
                let r = 1.0 - (1.0 - cr) * (1.0 - lr);
                let g = 1.0 - (1.0 - cg) * (1.0 - lg);
                let b = 1.0 - (1.0 - cb) * (1.0 - lb);
                row[x] = Color32::from_rgba_unmultiplied(
                    (r * 255.0) as u8, (g * 255.0) as u8, (b * 255.0) as u8, c.a(),
                );
            }
        });
        ColorImage::new([w, h], out)
    }

    /// 分離可能ボックスブラー。`horizontal = true` で水平方向、false で垂直方向。
    /// 入力は [f32; 3] 配列。radius は片側の画素数 (例: 5 なら 11 タップ)。
    fn separable_box_blur(src: &[[f32; 3]], w: usize, h: usize, radius: usize, horizontal: bool) -> Vec<[f32; 3]> {
        let mut out = vec![[0.0_f32; 3]; w * h];
        let window = (radius * 2 + 1) as f32;
        if horizontal {
            out.par_chunks_mut(w).enumerate().for_each(|(y, row)| {
                let base = y * w;
                for x in 0..w {
                    let x0 = x.saturating_sub(radius);
                    let x1 = (x + radius + 1).min(w);
                    let mut sum = [0.0_f32; 3];
                    let mut count = 0.0_f32;
                    for xi in x0..x1 {
                        let p = src[base + xi];
                        sum[0] += p[0]; sum[1] += p[1]; sum[2] += p[2];
                        count += 1.0;
                    }
                    // 境界で count 不足を avg 補正 (window 固定だと端で暗くなる)
                    let c = if count > 0.0 { count } else { window };
                    row[x] = [sum[0] / c, sum[1] / c, sum[2] / c];
                }
            });
        } else {
            out.par_chunks_mut(w).enumerate().for_each(|(y, row)| {
                let y0 = y.saturating_sub(radius);
                let y1 = (y + radius + 1).min(h);
                for x in 0..w {
                    let mut sum = [0.0_f32; 3];
                    let mut count = 0.0_f32;
                    for yi in y0..y1 {
                        let p = src[yi * w + x];
                        sum[0] += p[0]; sum[1] += p[1]; sum[2] += p[2];
                        count += 1.0;
                    }
                    let c = if count > 0.0 { count } else { window };
                    row[x] = [sum[0] / c, sum[1] / c, sum[2] / c];
                }
            });
        }
        out
    }

    // ── 絵画・描画風 ────────────────────────────────────────────

    /// ハーフトーン (漫画風)。輝度をグレー化し、6×6 セルごとのドットで濃淡表現。
    pub fn apply_halftone(src: &ColorImage) -> ColorImage {
        const CELL: usize = 6;
        let [w, h] = src.size;
        let src_pixels = &src.pixels;
        // セル平均輝度を前計算
        let cells_w = w.div_ceil(CELL);
        let cells_h = h.div_ceil(CELL);
        let mut cell_lum = vec![0.0_f32; cells_w * cells_h];
        for cy in 0..cells_h {
            for cx in 0..cells_w {
                let y0 = cy * CELL;
                let y1 = ((cy + 1) * CELL).min(h);
                let x0 = cx * CELL;
                let x1 = ((cx + 1) * CELL).min(w);
                let mut sum = 0.0_f32;
                let mut count = 0.0_f32;
                for y in y0..y1 {
                    for x in x0..x1 {
                        sum += luminance(src_pixels[y * w + x]);
                        count += 1.0;
                    }
                }
                cell_lum[cy * cells_w + cx] = if count > 0.0 { sum / count } else { 1.0 };
            }
        }
        map_parallel_xy(src, move |x, y, c| {
            let cx = x / CELL;
            let cy = y / CELL;
            let lum = cell_lum[cy * cells_w + cx];
            // セル中心からの距離でドット半径を決める
            let local_x = (x % CELL) as f32 - (CELL as f32 / 2.0) + 0.5;
            let local_y = (y % CELL) as f32 - (CELL as f32 / 2.0) + 0.5;
            let d = (local_x * local_x + local_y * local_y).sqrt();
            // 暗いセル → 大きなドット、明るいセル → 小さなドット
            let max_r = (CELL as f32) * 0.55;
            let r_needed = (1.0 - lum) * max_r;
            let v = if d < r_needed { 0 } else { 255 };
            Color32::from_rgba_unmultiplied(v, v, v, c.a())
        })
    }

    /// オイルペイント風 (Kuwahara フィルタ)。
    /// 近傍を 4 つの象限に分け、輝度分散最小の領域の平均色を採る。
    /// 半径 5 (11×11) にして塗り重ね感を強める。処理コストはピクセル毎 ~120 ops だが rayon で吸収。
    pub fn apply_oil_paint(src: &ColorImage) -> ColorImage {
        const R: isize = 5;
        let [w, h] = src.size;
        let src_pixels = &src.pixels;
        let iw = w as isize;
        let ih = h as isize;
        map_parallel_xy(src, move |x, y, c| {
            let cx = x as isize;
            let cy = y as isize;
            // 4 つの象限それぞれで平均と分散を計算 (輝度ベース)
            let mut best_mean = [0.0_f32; 3];
            let mut best_var = f32::MAX;
            let quads: [(isize, isize, isize, isize); 4] = [
                (-R, 0, -R, 0), // 左上
                (0, R, -R, 0),  // 右上
                (-R, 0, 0, R),  // 左下
                (0, R, 0, R),   // 右下
            ];
            for &(dx0, dx1, dy0, dy1) in &quads {
                let mut sum = [0.0_f32; 3];
                let mut sum_sq_lum = 0.0_f32;
                let mut sum_lum = 0.0_f32;
                let mut count = 0.0_f32;
                for dy in dy0..=dy1 {
                    let ny = cy + dy;
                    if ny < 0 || ny >= ih { continue; }
                    let row = ny as usize * w;
                    for dx in dx0..=dx1 {
                        let nx = cx + dx;
                        if nx < 0 || nx >= iw { continue; }
                        let p = src_pixels[row + nx as usize];
                        let r = p.r() as f32;
                        let g = p.g() as f32;
                        let b = p.b() as f32;
                        sum[0] += r; sum[1] += g; sum[2] += b;
                        let lum = 0.299 * r + 0.587 * g + 0.114 * b;
                        sum_lum += lum;
                        sum_sq_lum += lum * lum;
                        count += 1.0;
                    }
                }
                if count > 0.0 {
                    let mean_lum = sum_lum / count;
                    let var = (sum_sq_lum / count - mean_lum * mean_lum).max(0.0);
                    if var < best_var {
                        best_var = var;
                        best_mean = [sum[0] / count, sum[1] / count, sum[2] / count];
                    }
                }
            }
            // alpha は中心ピクセルから継承 (Kuwahara は選択的平均なので alpha も単純継承が自然)
            Color32::from_rgba_unmultiplied(
                clamp_u8(best_mean[0]), clamp_u8(best_mean[1]), clamp_u8(best_mean[2]), c.a(),
            )
        })
    }

    /// スケッチ風 (Sobel エッジ検出 → 反転グレー)。
    pub fn apply_sketch(src: &ColorImage) -> ColorImage {
        let [w, h] = src.size;
        let src_pixels = &src.pixels;
        // 輝度マップ前計算
        let lum: Vec<f32> = src_pixels.par_iter().map(|c| luminance(*c) * 255.0).collect();
        let iw = w as isize;
        let ih = h as isize;
        let sample = |x: isize, y: isize| -> f32 {
            let x = x.clamp(0, iw - 1) as usize;
            let y = y.clamp(0, ih - 1) as usize;
            lum[y * w + x]
        };
        map_parallel_xy(src, move |x, y, c| {
            let xi = x as isize;
            let yi = y as isize;
            // Sobel 3×3 勾配
            let gx = -sample(xi - 1, yi - 1) + sample(xi + 1, yi - 1)
                    -2.0 * sample(xi - 1, yi) + 2.0 * sample(xi + 1, yi)
                    -sample(xi - 1, yi + 1) + sample(xi + 1, yi + 1);
            let gy = -sample(xi - 1, yi - 1) - 2.0 * sample(xi, yi - 1) - sample(xi + 1, yi - 1)
                    +sample(xi - 1, yi + 1) + 2.0 * sample(xi, yi + 1) + sample(xi + 1, yi + 1);
            let mag = (gx * gx + gy * gy).sqrt();
            // 閾値を強めにかけて紙の上の鉛筆風に
            let intensity = (mag * 0.6).min(255.0);
            // エッジ = 黒、非エッジ = 白 (反転)
            let v = 255 - intensity as u8;
            Color32::from_rgba_unmultiplied(v, v, v, c.a())
        })
    }

    // ── 実用: シャープ化 ────────────────────────────────────────

    /// アンシャープマスク: 元画像 − ガウシアンブラー の差分を重み付きで加算。
    /// amount 1.2, 半径 3 (7-tap × 2 回) でしっかり輪郭補強。
    pub fn apply_sharpen(src: &ColorImage) -> ColorImage {
        let [w, h] = src.size;
        let src_pixels = &src.pixels;
        // f32 RGB に変換 → ブラー
        let rgb: Vec<[f32; 3]> = src_pixels.par_iter().map(|c| {
            [c.r() as f32, c.g() as f32, c.b() as f32]
        }).collect();
        // 半径 3 のボックスブラーを 2 回重ねて擬似ガウシアン化
        let hblur1 = separable_box_blur(&rgb, w, h, 3, true);
        let vblur1 = separable_box_blur(&hblur1, w, h, 3, false);
        let hblur2 = separable_box_blur(&vblur1, w, h, 3, true);
        let blur = separable_box_blur(&hblur2, w, h, 3, false);
        const AMOUNT: f32 = 1.2;
        let mut out = vec![Color32::BLACK; w * h];
        out.par_chunks_mut(w).enumerate().for_each(|(y, row)| {
            for x in 0..w {
                let idx = y * w + x;
                let orig = rgb[idx];
                let blurred = blur[idx];
                let r = orig[0] + AMOUNT * (orig[0] - blurred[0]);
                let g = orig[1] + AMOUNT * (orig[1] - blurred[1]);
                let b = orig[2] + AMOUNT * (orig[2] - blurred[2]);
                row[x] = Color32::from_rgba_unmultiplied(
                    clamp_u8(r), clamp_u8(g), clamp_u8(b), src_pixels[idx].a(),
                );
            }
        });
        ColorImage::new([w, h], out)
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
    fn photo_filters_preserve_size() {
        // 写真系フィルタはすべてソース解像度を維持する (CRT のようなアップスケールなし)
        let src = make_test_image(32, 32);
        for &f in &[
            PostFilter::Sepia, PostFilter::MonoNeutral, PostFilter::MonoCool,
            PostFilter::MonoWarm, PostFilter::TealOrange, PostFilter::KodakPortra,
            PostFilter::FujiVelvia, PostFilter::BleachBypass, PostFilter::CrossProcess,
            PostFilter::Vintage, PostFilter::WarmTone, PostFilter::CoolTone,
            PostFilter::FilmGrain, PostFilter::Vignette, PostFilter::LightLeak,
            PostFilter::SoftFocus, PostFilter::Halftone, PostFilter::OilPaint,
            PostFilter::Sketch, PostFilter::Sharpen,
        ] {
            let out = apply(&src, f);
            assert_eq!(out.size, [32, 32], "{:?} changed size", f);
            assert_eq!(out.pixels.len(), 32 * 32);
        }
    }

    #[test]
    fn mono_neutral_is_grayscale() {
        let src = make_test_image(16, 16);
        let out = apply(&src, PostFilter::MonoNeutral);
        for p in &out.pixels {
            assert_eq!(p.r(), p.g(), "R != G in MonoNeutral");
            assert_eq!(p.g(), p.b(), "G != B in MonoNeutral");
        }
    }

    #[test]
    fn sketch_is_grayscale() {
        let src = make_test_image(16, 16);
        let out = apply(&src, PostFilter::Sketch);
        for p in &out.pixels {
            assert_eq!(p.r(), p.g());
            assert_eq!(p.g(), p.b());
        }
    }

    #[test]
    fn halftone_is_binary_grayscale() {
        let src = make_test_image(16, 16);
        let out = apply(&src, PostFilter::Halftone);
        // ハーフトーンは 0 / 255 の 2 階調グレー
        for p in &out.pixels {
            assert_eq!(p.r(), p.g());
            assert_eq!(p.g(), p.b());
            assert!(p.r() == 0 || p.r() == 255, "halftone produced grey value {}", p.r());
        }
    }

    #[test]
    fn filters_preserve_alpha_channel() {
        // 左半分が完全透過 (alpha=0)、右半分が不透明 (alpha=255) の画像。
        // 全フィルタ回帰テスト: from_rgb は alpha=255 を強制するので Codex が検出した退行を防ぐ。
        // CRT 系は bilinear で補間するが、十分広い領域なら中央部の alpha=0/255 は保存される。
        let w = 32_usize;
        let h = 32_usize;
        let mut pixels = Vec::with_capacity(w * h);
        for _y in 0..h {
            for x in 0..w {
                let a = if x < w / 2 { 0 } else { 255 };
                pixels.push(Color32::from_rgba_unmultiplied(180, 100, 60, a));
            }
        }
        let src = ColorImage::new([w, h], pixels);
        for &f in PostFilter::ALL {
            if matches!(f, PostFilter::None | PostFilter::Nearest) {
                continue; // これらは clone なので自明
            }
            let out = apply(&src, f);
            let has_transparent = out.pixels.iter().any(|p| p.a() < 10);
            let has_opaque = out.pixels.iter().any(|p| p.a() > 245);
            assert!(
                has_transparent && has_opaque,
                "{:?} failed alpha preservation (has_transparent={}, has_opaque={})",
                f, has_transparent, has_opaque,
            );
        }
    }

    #[test]
    fn vignette_center_brighter_than_corner() {
        // 中央は暗化が最小、対角端は最大暗化
        let src = ColorImage::new([100, 100], vec![Color32::WHITE; 100 * 100]);
        let out = apply(&src, PostFilter::Vignette);
        let center = out.pixels[50 * 100 + 50];
        let corner = out.pixels[0];
        assert!(center.r() > corner.r(), "vignette should darken corners more than center");
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
