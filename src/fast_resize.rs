//! SIMD 高速リサイズ用ユーティリティ (`fast_image_resize` ラッパー)。
//!
//! 背景: `image::imageops::resize` はスカラー実装 (AVX2 / SSE4.1 なし) で、
//! 7K-9K クラスの画像だと Triangle/Lanczos3 どちらでも秒オーダーかかる。
//! `fast_image_resize` は同じ convolution 系フィルタを SIMD で実装していて、
//! 実測 5-10 倍速い。`clamp_dynamic_for_gpu` とサムネイル生成で使う。
//!
//! 対応ピクセル型は実プロジェクトで出てくる RGBA8 / RGB8 の 2 種類のみ。
//! その他のバリアント (Luma / 16bit / F32) が来たら RGBA8 に変換してから処理する。
//! `DynamicImage::to_rgba8` は常にコピーするので fallback 経路は遅いが、通常は
//! 画像デコーダが RGBA8 か RGB8 で返してくるのでここには入らない。

use fast_image_resize::{FilterType, ResizeAlg, ResizeOptions, Resizer};
use image::{DynamicImage, RgbImage, RgbaImage};

/// リサイズ品質。Triangle 相当 (`Bilinear`) と Lanczos3 の 2 択。
///
/// - `Bilinear`: 2-tap 線形補間。GPU 上限クランプなど「縮小前提 & 速度優先」用。
///   image crate の `FilterType::Triangle` と同じフィルタ。
/// - `Lanczos3`: 6-tap sinc 系。サムネイル生成で使う「品質重視」用。
#[derive(Clone, Copy, Debug)]
pub enum Quality {
    Bilinear,
    Lanczos3,
}

impl From<Quality> for FilterType {
    fn from(q: Quality) -> FilterType {
        match q {
            Quality::Bilinear => FilterType::Bilinear,
            Quality::Lanczos3 => FilterType::Lanczos3,
        }
    }
}

/// RGBA8 画像を指定サイズに正確にリサイズする。
pub fn resize_rgba8_exact(src: &RgbaImage, new_w: u32, new_h: u32, quality: Quality) -> RgbaImage {
    let mut dst = RgbaImage::new(new_w.max(1), new_h.max(1));
    let mut resizer = Resizer::new();
    let opts = ResizeOptions::new().resize_alg(ResizeAlg::Convolution(quality.into()));
    // src/dst とも `image` feature 経由で `IntoImageView`/`IntoImageViewMut`。
    // U8x4 はアルファプレマルしない (Convolution は独立 4 ch として扱う)。
    resizer
        .resize(src, &mut dst, &opts)
        .expect("fast_image_resize: rgba8 resize must succeed for matching pixel types");
    dst
}

/// RGB8 画像を指定サイズに正確にリサイズする。
pub fn resize_rgb8_exact(src: &RgbImage, new_w: u32, new_h: u32, quality: Quality) -> RgbImage {
    let mut dst = RgbImage::new(new_w.max(1), new_h.max(1));
    let mut resizer = Resizer::new();
    let opts = ResizeOptions::new().resize_alg(ResizeAlg::Convolution(quality.into()));
    resizer
        .resize(src, &mut dst, &opts)
        .expect("fast_image_resize: rgb8 resize must succeed for matching pixel types");
    dst
}

/// DynamicImage を指定サイズに正確にリサイズする。
/// Rgba8 / Rgb8 はそのままのピクセル型で処理。それ以外は Rgba8 に変換する。
pub fn resize_dynamic_exact(
    src: &DynamicImage,
    new_w: u32,
    new_h: u32,
    quality: Quality,
) -> DynamicImage {
    match src {
        DynamicImage::ImageRgba8(buf) => {
            DynamicImage::ImageRgba8(resize_rgba8_exact(buf, new_w, new_h, quality))
        }
        DynamicImage::ImageRgb8(buf) => {
            DynamicImage::ImageRgb8(resize_rgb8_exact(buf, new_w, new_h, quality))
        }
        _ => {
            // 16bit / F32 / Luma 系: Rgba8 に変換して処理する (rare path)。
            let rgba = src.to_rgba8();
            DynamicImage::ImageRgba8(resize_rgba8_exact(&rgba, new_w, new_h, quality))
        }
    }
}

/// DynamicImage を (max_w, max_h) の矩形にアスペクト比保持で収める。
/// `image::DynamicImage::resize(w, h, Filter)` と同じセマンティクスの置き換え。
/// 既に収まっていればクローンして返す (追加のリサイズ処理なし)。
pub fn resize_dynamic_fit(
    src: &DynamicImage,
    max_w: u32,
    max_h: u32,
    quality: Quality,
) -> DynamicImage {
    let (w, h) = (src.width(), src.height());
    if w <= max_w && h <= max_h {
        return src.clone();
    }
    let ratio_w = max_w as f64 / w as f64;
    let ratio_h = max_h as f64 / h as f64;
    let scale = ratio_w.min(ratio_h);
    let new_w = ((w as f64 * scale).round() as u32).max(1);
    let new_h = ((h as f64 * scale).round() as u32).max(1);
    resize_dynamic_exact(src, new_w, new_h, quality)
}

/// 先行 dims 表示用の軽量なファイルヘッダ解析。
/// デコードはせず、PNG/JPEG/GIF/WebP/BMP のヘッダから幅×高さだけ取る。
/// 失敗したら None (呼び出し側はフルデコード完了まで dims を出さない)。
pub fn probe_dims(path: &std::path::Path) -> Option<[usize; 2]> {
    let reader = image::ImageReader::open(path).ok()?.with_guessed_format().ok()?;
    let (w, h) = reader.into_dimensions().ok()?;
    Some([w as usize, h as usize])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bilinear_rgba8_exact_produces_correct_dims() {
        let src = RgbaImage::from_pixel(256, 128, image::Rgba([10, 20, 30, 255]));
        let out = resize_rgba8_exact(&src, 64, 32, Quality::Bilinear);
        assert_eq!(out.dimensions(), (64, 32));
        // 均一色入力は均一色出力になる (Bilinear / Lanczos3 とも)
        for p in out.pixels() {
            assert_eq!(p.0[0], 10);
            assert_eq!(p.0[3], 255);
        }
    }

    #[test]
    fn lanczos_rgb8_exact_produces_correct_dims() {
        let src = RgbImage::from_pixel(500, 300, image::Rgb([128, 64, 200]));
        let out = resize_rgb8_exact(&src, 100, 60, Quality::Lanczos3);
        assert_eq!(out.dimensions(), (100, 60));
    }

    #[test]
    fn dynamic_fit_preserves_aspect() {
        let src = DynamicImage::ImageRgba8(RgbaImage::from_pixel(
            1000,
            500,
            image::Rgba([0, 0, 0, 255]),
        ));
        // 200×200 の box に収めると長辺 200、短辺 100 になる
        let out = resize_dynamic_fit(&src, 200, 200, Quality::Bilinear);
        assert_eq!((out.width(), out.height()), (200, 100));
    }

    #[test]
    fn dynamic_fit_noop_when_within_box() {
        let src = DynamicImage::ImageRgba8(RgbaImage::from_pixel(
            100,
            100,
            image::Rgba([5, 5, 5, 255]),
        ));
        let out = resize_dynamic_fit(&src, 200, 200, Quality::Bilinear);
        assert_eq!((out.width(), out.height()), (100, 100));
    }

    #[test]
    fn dynamic_exact_huge_portrait() {
        // clamp_dynamic_for_gpu の典型入力: 7168×9216 → 6372×8192
        // テスト実行を軽くするため 7168→6372 の比率だけ小スケールで検証。
        let src = DynamicImage::ImageRgba8(RgbaImage::from_pixel(
            716,
            921,
            image::Rgba([200, 100, 50, 255]),
        ));
        let out = resize_dynamic_exact(&src, 637, 819, Quality::Bilinear);
        assert_eq!((out.width(), out.height()), (637, 819));
    }
}
