//! 画像タイプ分類（ヒューリスティック）。
//!
//! 自動アップスケールモデル選択用に、画像を Illustration / Comic / RealLife に分類する。
//! モノクロ漫画を優先的に拾い、それ以外は彩度からイラスト / 写真を大まかに推定する。

use super::ImageCategory;

/// 画像がグレースケール（モノクロ漫画）かどうかを判定する。
///
/// RGB チャンネル間の差が小さいピクセルが 95% 以上なら grayscale とみなす。
pub fn is_likely_grayscale(image: &image::DynamicImage) -> bool {
    let rgb = image.to_rgb8();
    let total = rgb.width() as usize * rgb.height() as usize;
    if total == 0 {
        return false;
    }

    // サンプリング: 大きい画像は間引いて高速化
    let step = ((total as f64).sqrt() / 100.0).max(1.0) as usize;
    let mut gray_count = 0usize;
    let mut sampled = 0usize;

    for (i, pixel) in rgb.pixels().enumerate() {
        if i % step != 0 {
            continue;
        }
        sampled += 1;
        let [r, g, b] = pixel.0;
        let max = r.max(g).max(b);
        let min = r.min(g).min(b);
        if max - min <= 15 {
            gray_count += 1;
        }
    }

    if sampled == 0 {
        return false;
    }

    let ratio = gray_count as f64 / sampled as f64;
    ratio > 0.95
}

/// 分類器なしで使えるヒューリスティクス分類。
///
/// 通常の自動アップスケールモデル選択ではこちらを使う。
pub fn classify_heuristic(image: &image::DynamicImage) -> ImageCategory {
    if is_likely_grayscale(image) {
        return ImageCategory::Comic;
    }

    // 彩度を調べてイラスト vs 写真を推定
    let rgb = image.to_rgb8();
    let total = rgb.width() as usize * rgb.height() as usize;
    let step = ((total as f64).sqrt() / 80.0).max(1.0) as usize;
    let mut high_sat_count = 0usize;
    let mut sampled = 0usize;

    for (i, pixel) in rgb.pixels().enumerate() {
        if i % step != 0 {
            continue;
        }
        sampled += 1;
        let [r, g, b] = pixel.0;
        let max_c = r.max(g).max(b) as f32;
        let min_c = r.min(g).min(b) as f32;
        if max_c > 0.0 {
            let sat = (max_c - min_c) / max_c;
            if sat > 0.4 {
                high_sat_count += 1;
            }
        }
    }

    if sampled == 0 {
        return ImageCategory::RealLife;
    }

    let high_sat_ratio = high_sat_count as f64 / sampled as f64;

    // 高彩度ピクセルが多い → イラスト系
    if high_sat_ratio > 0.3 {
        ImageCategory::Illustration
    } else {
        ImageCategory::RealLife
    }
}
