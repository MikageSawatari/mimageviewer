//! タイルブレンドの品質検証テスト。
//!
//! スクリーントーン（ドットパターン）のテスト画像を生成し、
//! アップスケール結果のタイル境界付近で濃淡差がないか数値的に検証する。
//!
//! 実行方法:
//!   cargo test --test test_tile_blend -- --nocapture

use std::sync::atomic::{AtomicBool};
use std::sync::Arc;
use std::path::PathBuf;

fn find_models_dir() -> PathBuf {
    let cwd_models = std::env::current_dir().unwrap().join("models");
    if cwd_models.exists() { return cwd_models; }
    if let Ok(appdata) = std::env::var("APPDATA") {
        let p = PathBuf::from(appdata).join("mimageviewer").join("models");
        if p.exists() { return p; }
    }
    panic!("models/ directory not found");
}

/// 均一なスクリーントーン（ドットパターン）画像を生成する。
/// ドット間隔 period px, ドット半径 radius px, 背景白・ドット黒。
fn generate_screentone(width: u32, height: u32, period: u32, radius: f32) -> image::DynamicImage {
    let mut img = image::RgbImage::new(width, height);
    // 白で埋める
    for p in img.pixels_mut() {
        *p = image::Rgb([255, 255, 255]);
    }
    // ドットパターンを描画
    let r2 = radius * radius;
    for cy in (0..height).step_by(period as usize) {
        for cx in (0..width).step_by(period as usize) {
            let center_x = cx as f32 + period as f32 * 0.5;
            let center_y = cy as f32 + period as f32 * 0.5;
            let min_x = (center_x - radius).max(0.0) as u32;
            let max_x = ((center_x + radius) as u32 + 1).min(width);
            let min_y = (center_y - radius).max(0.0) as u32;
            let max_y = ((center_y + radius) as u32 + 1).min(height);
            for y in min_y..max_y {
                for x in min_x..max_x {
                    let dx = x as f32 - center_x;
                    let dy = y as f32 - center_y;
                    if dx * dx + dy * dy <= r2 {
                        img.put_pixel(x, y, image::Rgb([0, 0, 0]));
                    }
                }
            }
        }
    }
    image::DynamicImage::ImageRgb8(img)
}

/// 出力画像のタイル境界付近の平均輝度と中央部の平均輝度を比較する。
/// 差が大きいほどタイル境界が目立つ。
fn analyze_tile_boundaries(
    img: &egui::ColorImage,
    tile_size: u32,
    scale: u32,
) -> (f64, f64, f64) {
    let [w, h] = img.size;
    let scaled_tile = (tile_size * scale) as usize;

    let mut boundary_sum = 0.0_f64;
    let mut boundary_count = 0usize;
    let mut center_sum = 0.0_f64;
    let mut center_count = 0usize;

    let band = 8usize; // 境界付近 ±8 ピクセルを検査

    for y in 0..h {
        for x in 0..w {
            let c = img.pixels[y * w + x];
            let lum = c.r() as f64 * 0.299 + c.g() as f64 * 0.587 + c.b() as f64 * 0.114;

            // タイル境界からの距離
            let dist_x = if scaled_tile > 0 {
                let pos_in_tile = x % scaled_tile;
                pos_in_tile.min(scaled_tile - pos_in_tile)
            } else { 999 };
            let dist_y = if scaled_tile > 0 {
                let pos_in_tile = y % scaled_tile;
                pos_in_tile.min(scaled_tile - pos_in_tile)
            } else { 999 };
            let dist = dist_x.min(dist_y);

            if dist <= band {
                boundary_sum += lum;
                boundary_count += 1;
            } else if dist >= scaled_tile / 4 {
                center_sum += lum;
                center_count += 1;
            }
        }
    }

    let boundary_avg = if boundary_count > 0 { boundary_sum / boundary_count as f64 } else { 0.0 };
    let center_avg = if center_count > 0 { center_sum / center_count as f64 } else { 0.0 };
    let diff = (boundary_avg - center_avg).abs();

    (boundary_avg, center_avg, diff)
}

/// 実際の漫画画像でタイル境界の品質を検証する。
/// TEST_IMAGE_PATH 環境変数で漫画画像を指定。
#[test]
fn test_manga_tile_blend() {
    let img_path = match std::env::var("TEST_IMAGE_PATH") {
        Ok(p) if std::path::PathBuf::from(&p).exists() => p,
        _ => {
            println!("SKIP: TEST_IMAGE_PATH not set or not found");
            return;
        }
    };

    let models_dir = find_models_dir();
    let cugan_path = models_dir.join("realcugan_4x_conservative.onnx");
    if !cugan_path.exists() {
        println!("SKIP: realcugan model not found");
        return;
    }

    let runtime = mimageviewer::ai::runtime::AiRuntime::new().unwrap();
    runtime.load_model(
        mimageviewer::ai::ModelKind::UpscaleRealCugan4x,
        &cugan_path,
    ).unwrap();

    let img = image::open(&img_path).expect("Failed to open test image");
    println!("Manga image: {}x{} from {}", img.width(), img.height(), img_path);

    let cancel = Arc::new(AtomicBool::new(false));
    let t0 = std::time::Instant::now();
    let result = mimageviewer::ai::upscale::upscale(
        &runtime,
        mimageviewer::ai::ModelKind::UpscaleRealCugan4x,
        &img,
        &cancel,
    ).expect("Upscale failed");
    let elapsed = t0.elapsed();

    let [w, h] = result.size;
    println!("Output: {}x{} in {:.2}s", w, h, elapsed.as_secs_f64());

    // 出力保存
    let mut rgb_buf = image::RgbImage::new(w as u32, h as u32);
    for y in 0..h {
        for x in 0..w {
            let c = result.pixels[y * w + x];
            rgb_buf.put_pixel(x as u32, y as u32, image::Rgb([c.r(), c.g(), c.b()]));
        }
    }
    rgb_buf.save("target/test_manga_blend_output.png").unwrap();
    println!("Output saved to: target/test_manga_blend_output.png");

    // タイル境界の分析
    let tile_step = 192 - 32; // = 160 入力ピクセル
    let (boundary_avg, center_avg, diff) = analyze_tile_boundaries(&result, tile_step as u32, 4);
    println!("Boundary luminance: {:.2}", boundary_avg);
    println!("Center luminance:   {:.2}", center_avg);
    println!("Difference:         {:.2}", diff);
}

/// 複数パターンのスクリーントーンでブレンド品質を一括検証する。
/// 各パターンの出力画像を target/ に保存して目視確認も可能。
#[test]
fn test_screentone_patterns() {
    let models_dir = find_models_dir();
    let cugan_path = models_dir.join("realcugan_4x_conservative.onnx");
    if !cugan_path.exists() {
        println!("SKIP: realcugan model not found");
        return;
    }

    let runtime = mimageviewer::ai::runtime::AiRuntime::new().unwrap();
    runtime.load_model(
        mimageviewer::ai::ModelKind::UpscaleRealCugan4x,
        &cugan_path,
    ).unwrap();

    let cancel = Arc::new(AtomicBool::new(false));

    // 複数のスクリーントーンパターン: (名前, 画像サイズ, ドット間隔, ドット半径, 背景グレー)
    let patterns: Vec<(&str, u32, u32, f32, u8)> = vec![
        ("fine_8px_r2",     400, 8,  2.0, 255),   // 細かい (8px間隔, r=2)
        ("medium_12px_r3",  400, 12, 3.0, 255),   // 中程度 (12px間隔, r=3)
        ("coarse_16px_r4",  400, 16, 4.0, 255),   // 粗い (16px間隔, r=4)
        ("large_24px_r6",   400, 24, 6.0, 255),   // 大きい (24px間隔, r=6)
        ("gray_bg_12px",    400, 12, 3.0, 220),   // グレー背景
        ("dense_6px_r1",    400, 6,  1.5, 255),   // 非常に細かい
        ("wide_500px",      500, 14, 3.5, 255),   // 横長 (タイル境界が多い)
    ];

    println!("{:<20} {:>6} {:>8} {:>8} {:>8}", "Pattern", "Time", "Bnd.Avg", "Ctr.Avg", "Diff");
    println!("{}", "-".repeat(60));

    for (name, size, period, radius, bg) in &patterns {
        let img = generate_screentone_with_bg(*size, *size, *period, *radius, *bg);

        let in_path = format!("target/tone_{name}_input.png");
        img.save(&in_path).unwrap();

        let t0 = std::time::Instant::now();
        let result = mimageviewer::ai::upscale::upscale(
            &runtime,
            mimageviewer::ai::ModelKind::UpscaleRealCugan4x,
            &img,
            &cancel,
        ).expect("Upscale failed");
        let elapsed = t0.elapsed();

        let [w, h] = result.size;
        let out_path = format!("target/tone_{name}_output.png");
        save_color_image(&result, &out_path);

        let tile_step = 192 - 32;
        let (boundary_avg, center_avg, diff) = analyze_tile_boundaries(&result, tile_step as u32, 4);

        let quality = if diff < 2.0 { "Excellent" }
            else if diff < 5.0 { "Good" }
            else if diff < 10.0 { "Acceptable" }
            else { "VISIBLE" };

        println!("{:<20} {:>5.1}s {:>8.1} {:>8.1} {:>7.1} {}", name, elapsed.as_secs_f64(),
            boundary_avg, center_avg, diff, quality);
    }
}

/// 背景色指定可能なスクリーントーン生成。
fn generate_screentone_with_bg(width: u32, height: u32, period: u32, radius: f32, bg: u8) -> image::DynamicImage {
    let mut img = image::RgbImage::new(width, height);
    for p in img.pixels_mut() {
        *p = image::Rgb([bg, bg, bg]);
    }
    let r2 = radius * radius;
    for cy in (0..height).step_by(period as usize) {
        for cx in (0..width).step_by(period as usize) {
            let center_x = cx as f32 + period as f32 * 0.5;
            let center_y = cy as f32 + period as f32 * 0.5;
            let min_x = (center_x - radius).max(0.0) as u32;
            let max_x = ((center_x + radius) as u32 + 1).min(width);
            let min_y = (center_y - radius).max(0.0) as u32;
            let max_y = ((center_y + radius) as u32 + 1).min(height);
            for y in min_y..max_y {
                for x in min_x..max_x {
                    let dx = x as f32 - center_x;
                    let dy = y as f32 - center_y;
                    if dx * dx + dy * dy <= r2 {
                        img.put_pixel(x, y, image::Rgb([0, 0, 0]));
                    }
                }
            }
        }
    }
    image::DynamicImage::ImageRgb8(img)
}

fn save_color_image(img: &egui::ColorImage, path: &str) {
    let [w, h] = img.size;
    let mut rgb = image::RgbImage::new(w as u32, h as u32);
    for y in 0..h {
        for x in 0..w {
            let c = img.pixels[y * w + x];
            rgb.put_pixel(x as u32, y as u32, image::Rgb([c.r(), c.g(), c.b()]));
        }
    }
    rgb.save(path).unwrap();
}

/// Real-CUGAN でスクリーントーン画像をアップスケールし、
/// タイル境界の品質を検証する。
#[test]
fn test_screentone_tile_blend() {
    let models_dir = find_models_dir();
    let cugan_path = models_dir.join("realcugan_4x_conservative.onnx");
    if !cugan_path.exists() {
        println!("SKIP: realcugan model not found");
        return;
    }

    let runtime = mimageviewer::ai::runtime::AiRuntime::new().unwrap();
    runtime.load_model(
        mimageviewer::ai::ModelKind::UpscaleRealCugan4x,
        &cugan_path,
    ).unwrap();

    // テスト用スクリーントーン画像: 400x400, ドット間隔 8px, 半径 2.5px
    let screentone = generate_screentone(400, 400, 8, 2.5);
    println!("Screentone test image: {}x{}", screentone.width(), screentone.height());

    // 入力画像も保存
    screentone.save("target/test_screentone_input.png").unwrap();
    println!("Input saved to: target/test_screentone_input.png");

    let cancel = Arc::new(AtomicBool::new(false));
    let t0 = std::time::Instant::now();
    let result = mimageviewer::ai::upscale::upscale(
        &runtime,
        mimageviewer::ai::ModelKind::UpscaleRealCugan4x,
        &screentone,
        &cancel,
    ).expect("Upscale failed");
    let elapsed = t0.elapsed();

    let [w, h] = result.size;
    println!("Output: {}x{} in {:.2}s", w, h, elapsed.as_secs_f64());

    // 出力画像を保存
    let mut rgb_buf = image::RgbImage::new(w as u32, h as u32);
    for y in 0..h {
        for x in 0..w {
            let c = result.pixels[y * w + x];
            rgb_buf.put_pixel(x as u32, y as u32, image::Rgb([c.r(), c.g(), c.b()]));
        }
    }
    rgb_buf.save("target/test_screentone_output.png").unwrap();
    println!("Output saved to: target/test_screentone_output.png");

    // タイル境界の品質分析
    // tile_size=192, overlap=32 なので実効ステップは 160、出力では 640px 間隔
    let tile_step = 192 - 32; // = 160 入力ピクセル
    let (boundary_avg, center_avg, diff) = analyze_tile_boundaries(&result, tile_step as u32, 4);
    println!("Boundary luminance: {:.2}", boundary_avg);
    println!("Center luminance:   {:.2}", center_avg);
    println!("Difference:         {:.2}", diff);
    println!("  (< 2.0 is excellent, < 5.0 is acceptable, > 10.0 is visible)");

    if diff > 10.0 {
        println!("WARNING: Tile boundary seam is likely visible (diff={:.2})", diff);
    }
}
