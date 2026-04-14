//! LaMa Inpainting テストプログラム。
//!
//! Usage: cargo run --release --bin test_inpaint
//!
//! %APPDATA%/mimageviewer/models/lama_fp32.onnx を読み込み、
//! CPU モードで 512x512 推論をテスト。結果を PNG で保存。

use std::path::PathBuf;

fn model_path() -> PathBuf {
    let appdata = std::env::var("APPDATA").expect("APPDATA not set");
    PathBuf::from(appdata)
        .join("mimageviewer")
        .join("models")
        .join("lama_fp32.onnx")
}

fn main() {
    let path = model_path();
    println!("Model path: {}", path.display());
    if !path.exists() {
        eprintln!("ERROR: Model file not found.");
        std::process::exit(1);
    }

    // CPU セッション（DirectML は LaMa と非互換）
    println!("Loading model (CPU)...");
    let t0 = std::time::Instant::now();
    let mut session = ort::session::Session::builder()
        .expect("builder")
        .with_optimization_level(ort::session::builder::GraphOptimizationLevel::Level3)
        .expect("opt")
        .with_intra_threads(4)
        .expect("threads")
        .commit_from_file(&path)
        .expect("load model");
    println!("Loaded in {:.2}s", t0.elapsed().as_secs_f64());

    // モデル入出力情報
    println!("\nInputs:");
    for input in session.inputs().iter() {
        println!("  {:?}", input);
    }
    println!("Outputs:");
    for output in session.outputs().iter() {
        println!("  {:?}", output);
    }

    // ── テスト画像: 左半分=カラフル、中央=黒(gap)、右半分=カラフル ──
    let s = 512usize;
    let gap_x0 = 200;
    let gap_x1 = 312;

    // === テスト 1: [0,1] 正規化 ===
    println!("\n=== Test 1: image [0,1], mask [0,1] ===");
    test_with_range(&mut session, s, gap_x0, gap_x1, 1.0, 1.0, "test1_01");

    // === テスト 2: [0,255] 入力 ===
    println!("\n=== Test 2: image [0,255], mask [0,255] ===");
    test_with_range(&mut session, s, gap_x0, gap_x1, 255.0, 255.0, "test2_255");

    // === テスト 3: image [0,255], mask [0,1] ===
    println!("\n=== Test 3: image [0,255], mask [0,1] ===");
    test_with_range(&mut session, s, gap_x0, gap_x1, 255.0, 1.0, "test3_img255_mask01");

    println!("\nDone. Check test*.png files.");
}

fn test_with_range(
    session: &mut ort::session::Session,
    s: usize,
    gap_x0: usize,
    gap_x1: usize,
    img_scale: f32,
    mask_scale: f32,
    prefix: &str,
) {
    let mut image_nchw = ndarray::Array4::<f32>::zeros((1, 3, s, s));
    let mut mask_nchw = ndarray::Array4::<f32>::zeros((1, 1, s, s));

    for y in 0..s {
        for x in 0..s {
            if x >= gap_x0 && x < gap_x1 {
                // gap: 黒
                mask_nchw[[0, 0, y, x]] = mask_scale;
            } else {
                // コンテンツ: グラデーション
                let r = (x as f32 / s as f32) * img_scale;
                let g = (y as f32 / s as f32) * img_scale;
                let b = 0.5 * img_scale;
                image_nchw[[0, 0, y, x]] = r;
                image_nchw[[0, 1, y, x]] = g;
                image_nchw[[0, 2, y, x]] = b;
            }
        }
    }

    // 入力画像を保存
    save_nchw_as_png(&image_nchw, s, img_scale, &format!("{prefix}_input.png"));

    let image_tensor = ort::value::Tensor::from_array(image_nchw).expect("image tensor");
    let mask_tensor = ort::value::Tensor::from_array(mask_nchw).expect("mask tensor");

    let t0 = std::time::Instant::now();
    match session.run(ort::inputs!["image" => image_tensor, "mask" => mask_tensor]) {
        Ok(outputs) => {
            let elapsed = t0.elapsed();
            match outputs[0].try_extract_tensor::<f32>() {
                Ok((_shape, data)) => {
                    println!("  OK ({:.1}ms)", elapsed.as_secs_f64() * 1000.0);

                    // 出力値の統計
                    let len = data.len();
                    let min = data.iter().copied().fold(f32::INFINITY, f32::min);
                    let max = data.iter().copied().fold(f32::NEG_INFINITY, f32::max);
                    let mean: f32 = data.iter().sum::<f32>() / len as f32;

                    // gap 領域の値
                    let mut gap_vals = Vec::new();
                    for y in (s / 2 - 2)..(s / 2 + 2) {
                        for x in gap_x0..gap_x1.min(gap_x0 + 5) {
                            // R channel: offset 0*s*s
                            let r = data[0 * s * s + y * s + x];
                            let g = data[1 * s * s + y * s + x];
                            let b = data[2 * s * s + y * s + x];
                            gap_vals.push((r, g, b));
                        }
                    }

                    // 非 gap 領域の値
                    let mut nongap_vals = Vec::new();
                    for y in (s / 2 - 2)..(s / 2 + 2) {
                        for x in 10..15 {
                            let r = data[0 * s * s + y * s + x];
                            let g = data[1 * s * s + y * s + x];
                            let b = data[2 * s * s + y * s + x];
                            nongap_vals.push((r, g, b));
                        }
                    }

                    println!("  Output stats: min={min:.4}, max={max:.4}, mean={mean:.4}, len={len}");
                    println!("  Gap samples (center): {:?}", &gap_vals[..gap_vals.len().min(5)]);
                    println!("  Non-gap samples (x=10): {:?}", &nongap_vals[..nongap_vals.len().min(5)]);

                    // 出力画像を保存
                    save_output_as_png(&data, s, min, max, &format!("{prefix}_output.png"));
                }
                Err(e) => println!("  Extract failed: {e}"),
            }
        }
        Err(e) => println!("  FAILED: {e}"),
    }
}

fn save_nchw_as_png(data: &ndarray::Array4<f32>, s: usize, scale: f32, filename: &str) {
    let mut img = image::RgbImage::new(s as u32, s as u32);
    for y in 0..s {
        for x in 0..s {
            let r = (data[[0, 0, y, x]] / scale * 255.0).clamp(0.0, 255.0) as u8;
            let g = (data[[0, 1, y, x]] / scale * 255.0).clamp(0.0, 255.0) as u8;
            let b = (data[[0, 2, y, x]] / scale * 255.0).clamp(0.0, 255.0) as u8;
            img.put_pixel(x as u32, y as u32, image::Rgb([r, g, b]));
        }
    }
    img.save(filename).expect("save input png");
    println!("  Saved: {filename}");
}

fn save_output_as_png(data: &[f32], s: usize, min: f32, max: f32, filename: &str) {
    let mut img = image::RgbImage::new(s as u32, s as u32);
    let range = (max - min).max(1e-6);
    for y in 0..s {
        for x in 0..s {
            // 出力値の範囲に応じて正規化
            let r = ((data[0 * s * s + y * s + x] - min) / range * 255.0).clamp(0.0, 255.0) as u8;
            let g = ((data[1 * s * s + y * s + x] - min) / range * 255.0).clamp(0.0, 255.0) as u8;
            let b = ((data[2 * s * s + y * s + x] - min) / range * 255.0).clamp(0.0, 255.0) as u8;
            img.put_pixel(x as u32, y as u32, image::Rgb([r, g, b]));
        }
    }
    img.save(filename).expect("save output png");
    println!("  Saved: {filename} (normalized from [{min:.2}, {max:.2}])");
}
