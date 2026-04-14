//! 画像タイプ分類（MobileNetV3 ベース）。
//!
//! deepghs/anime_classification の ONNX モデルを使い、
//! 画像を Illustration / Comic / ThreeD / RealLife に分類する。
//! 補助ヒューリスティクスでグレースケール判定も行う。

use super::runtime::AiRuntime;
use super::{AiError, ImageCategory, ModelKind};

/// 画像を分類して最適なカテゴリを返す。
///
/// 1. グレースケール判定 → 高確率で Comic
/// 2. MobileNetV3 で推論
/// 3. softmax → argmax
pub fn classify(
    runtime: &AiRuntime,
    image: &image::DynamicImage,
) -> Result<ImageCategory, AiError> {
    // ── Step 1: グレースケールヒューリスティクス ──
    if is_likely_grayscale(image) {
        return Ok(ImageCategory::Comic);
    }

    // ── Step 2: MobileNetV3 推論 ──
    let input_array = preprocess(image);
    let input_tensor = ort::value::Tensor::from_array(input_array)
        .map_err(|e| AiError::Ort(format!("Tensor creation: {e}")))?;

    let category = runtime.with_session(ModelKind::ClassifierMobileNet, |session| {
        let outputs = session
            .run(ort::inputs![input_tensor])
            .map_err(|e| AiError::Ort(format!("run: {e}")))?;

        // 出力テンソルからスコアを取得
        let (_shape, scores) = outputs[0]
            .try_extract_tensor::<f32>()
            .map_err(|e| AiError::Ort(format!("extract: {e}")))?;

        Ok(argmax_to_category(scores))
    })?;

    Ok(category)
}

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

/// 画像を MobileNetV3 入力用に前処理する。
///
/// - 384x384 にリサイズ（mobilenetv3_v1.5_dist の入力サイズ）
/// - [0, 1] に正規化
/// - ImageNet mean/std で標準化
/// - NCHW 形式 [1, 3, 384, 384]
fn preprocess(image: &image::DynamicImage) -> ndarray::Array4<f32> {
    const SIZE: u32 = 384;
    let resized = image.resize_exact(SIZE, SIZE, image::imageops::FilterType::Triangle);
    let rgb = resized.to_rgb8();

    // ImageNet の平均・標準偏差
    let mean = [0.485f32, 0.456, 0.406];
    let std = [0.229f32, 0.224, 0.225];

    let mut tensor = ndarray::Array4::<f32>::zeros((1, 3, SIZE as usize, SIZE as usize));

    for y in 0..SIZE {
        for x in 0..SIZE {
            let pixel = rgb.get_pixel(x, y);
            for c in 0..3 {
                let val = pixel.0[c] as f32 / 255.0;
                tensor[[0, c, y as usize, x as usize]] = (val - mean[c]) / std[c];
            }
        }
    }

    tensor
}

/// softmax スコアから ImageCategory に変換する。
///
/// mobilenetv3_v1.5_dist の出力クラス順序（meta.json による）:
/// 0: "3d", 1: "bangumi", 2: "comic", 3: "illustration", 4: "not_painting"
///
/// bangumi は実質的に illustration に近いため Illustration として扱う。
/// not_painting は写真として扱う。
fn argmax_to_category(scores: &[f32]) -> ImageCategory {
    if scores.len() < 4 {
        return ImageCategory::RealLife;
    }

    let mut max_idx = 0;
    let mut max_val = scores[0];
    for (i, &s) in scores.iter().enumerate().skip(1) {
        if s > max_val {
            max_val = s;
            max_idx = i;
        }
    }

    match max_idx {
        0 => ImageCategory::ThreeD,          // 3d
        1 => ImageCategory::Illustration,     // bangumi → illustration 扱い
        2 => ImageCategory::Comic,            // comic
        3 => ImageCategory::Illustration,     // illustration
        4 => ImageCategory::RealLife,         // not_painting → 写真扱い
        _ => ImageCategory::RealLife,
    }
}

/// 分類器なしで使えるヒューリスティクス分類（フォールバック用）。
///
/// モデルが未ダウンロードの場合にこちらを使う。
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
