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
    resize_dynamic_fit_with_source_aspect(src, max_w, max_h, (src.width(), src.height()), quality)
}

const THUMBNAIL_ASPECT_ERROR_TARGET: f64 = 0.0005;

/// Choose the largest near-cap integer raster whose aspect error is at most 0.05%.
///
/// Thumbnail dimensions are integers, so fixing the long edge and rounding the short
/// edge can leave a visible aspect error. Consider candidates derived from both axes;
/// among candidates meeting the error target, retain the largest long edge. If an
/// unusually small source cannot meet the target, use the globally closest candidate.
/// The returned dimensions never exceed either the source raster or the requested box.
pub(crate) fn aspect_accurate_fit_dimensions(
    raster_size: (u32, u32),
    max_size: (u32, u32),
    source_aspect_size: (u32, u32),
) -> (u32, u32) {
    let (raster_w, raster_h) = (raster_size.0.max(1), raster_size.1.max(1));
    let bound_w = raster_w.min(max_size.0.max(1));
    let bound_h = raster_h.min(max_size.1.max(1));
    let (aspect_w, aspect_h) = (
        source_aspect_size.0.max(1) as f64,
        source_aspect_size.1.max(1) as f64,
    );
    let source_ratio = aspect_w / aspect_h;

    #[derive(Clone, Copy)]
    struct Candidate {
        width: u32,
        height: u32,
        error: f64,
    }

    impl Candidate {
        fn long_edge(self) -> u32 {
            self.width.max(self.height)
        }

        fn area(self) -> u64 {
            self.width as u64 * self.height as u64
        }
    }

    fn better_acceptable(candidate: Candidate, current: Candidate) -> bool {
        candidate.long_edge() > current.long_edge()
            || (candidate.long_edge() == current.long_edge()
                && (candidate.error < current.error
                    || (candidate.error == current.error && candidate.area() > current.area())))
    }

    fn better_fallback(candidate: Candidate, current: Candidate) -> bool {
        candidate.error < current.error
            || (candidate.error == current.error
                && (candidate.long_edge() > current.long_edge()
                    || (candidate.long_edge() == current.long_edge()
                        && candidate.area() > current.area())))
    }

    let mut acceptable: Option<Candidate> = None;
    let mut fallback: Option<Candidate> = None;
    let mut consider = |width: u32, height: u32| {
        if width == 0 || height == 0 || width > bound_w || height > bound_h {
            return;
        }
        let ratio = width as f64 / height as f64;
        let candidate = Candidate {
            width,
            height,
            error: ((ratio / source_ratio) - 1.0).abs(),
        };
        if candidate.error <= THUMBNAIL_ASPECT_ERROR_TARGET
            && acceptable.is_none_or(|current| better_acceptable(candidate, current))
        {
            acceptable = Some(candidate);
        }
        if fallback.is_none_or(|current| better_fallback(candidate, current)) {
            fallback = Some(candidate);
        }
    };

    // Fix each possible height and test the two adjacent integer widths.
    for height in 1..=bound_h {
        let ideal_width = source_ratio * height as f64;
        consider(ideal_width.floor().max(1.0) as u32, height);
        consider(ideal_width.ceil().max(1.0) as u32, height);
    }
    // Do the symmetric search so a width-limited candidate is not lost.
    for width in 1..=bound_w {
        let ideal_height = width as f64 / source_ratio;
        consider(width, ideal_height.floor().max(1.0) as u32);
        consider(width, ideal_height.ceil().max(1.0) as u32);
    }

    let selected = acceptable.or(fallback).unwrap_or(Candidate {
        width: bound_w,
        height: bound_h,
        error: f64::INFINITY,
    });
    (selected.width, selected.height)
}

/// Fit a decoded raster using a separately supplied canonical source aspect.
///
/// PDF page boxes and DCT-scaled JPEG buffers can have a more accurate aspect than
/// the already rounded raster passed in here. No upscaling is performed.
pub fn resize_dynamic_fit_with_source_aspect(
    src: &DynamicImage,
    max_w: u32,
    max_h: u32,
    source_aspect_size: (u32, u32),
    quality: Quality,
) -> DynamicImage {
    let (w, h) = (src.width(), src.height());
    let (new_w, new_h) = aspect_accurate_fit_dimensions((w, h), (max_w, max_h), source_aspect_size);
    if w == new_w && h == new_h {
        return src.clone();
    }
    resize_dynamic_exact(src, new_w, new_h, quality)
}

/// 先行 dims 表示用の軽量なファイルヘッダ解析。
/// デコードはせず、PNG/JPEG/GIF/WebP/BMP のヘッダから幅×高さだけ取る。
/// 失敗したら None (呼び出し側はフルデコード完了まで dims を出さない)。
pub fn probe_dims(path: &std::path::Path) -> Option<[usize; 2]> {
    let reader = image::ImageReader::open(path)
        .ok()?
        .with_guessed_format()
        .ok()?;
    let (w, h) = reader.into_dimensions().ok()?;
    Some([w as usize, h as usize])
}

/// path を再 open できない検証済み relative page / archive entry 用。
pub fn probe_dims_from_bytes(bytes: &[u8]) -> Option<[usize; 2]> {
    let reader = image::ImageReader::new(std::io::Cursor::new(bytes))
        .with_guessed_format()
        .ok()?;
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
        let src =
            DynamicImage::ImageRgba8(RgbaImage::from_pixel(100, 100, image::Rgba([5, 5, 5, 255])));
        let out = resize_dynamic_fit(&src, 200, 200, Quality::Bilinear);
        assert_eq!((out.width(), out.height()), (100, 100));
    }

    #[test]
    fn thumbnail_dimensions_meet_aspect_target_without_exceeding_the_long_edge() {
        for (source_w, source_h) in [(1643, 2375), (1024, 1536), (896, 1120)] {
            let (width, height) = aspect_accurate_fit_dimensions(
                (source_w, source_h),
                (512, 512),
                (source_w, source_h),
            );
            let source_ratio = source_w as f64 / source_h as f64;
            let output_ratio = width as f64 / height as f64;
            let relative_error = ((output_ratio / source_ratio) - 1.0).abs();

            assert!(width <= 512 && height <= 512);
            assert!(
                width.max(height) >= 507,
                "the aspect improvement must retain at least 99% of the long edge"
            );
            assert!(
                relative_error <= THUMBNAIL_ASPECT_ERROR_TARGET,
                "{source_w}x{source_h} -> {width}x{height}: {relative_error:.6}"
            );
        }
    }

    #[test]
    fn thumbnail_dimensions_use_canonical_aspect_for_an_already_rounded_pdf_raster() {
        let (width, height) = aspect_accurate_fit_dimensions((327, 473), (512, 512), (1643, 2375));
        let source_ratio = 1643.0 / 2375.0;
        let output_ratio = width as f64 / height as f64;

        assert!(width <= 327 && height <= 473, "must not upscale");
        assert!(
            width.max(height) >= 468,
            "must retain at least 99% of the raster"
        );
        assert!(((output_ratio / source_ratio) - 1.0).abs() <= 0.0005);
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
