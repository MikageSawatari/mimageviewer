//! AI アップスケール機能の統合テスト。
//!
//! 全モデルがサンプル画像で正常に動作することを確認する。
//!
//! 実行方法:
//!   cargo test --test test_ai_upscale -- --nocapture
//!
//! 環境変数 TEST_IMAGE_PATH でテスト用画像を指定できる:
//!   TEST_IMAGE_PATH="D:\path\to\image.jpg" cargo test --test test_ai_upscale -- --nocapture

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

/// テスト用画像のパスを返す。
/// 環境変数 TEST_IMAGE_PATH があればそれを使い、なければデフォルトのテスト画像を生成。
fn get_test_image() -> image::DynamicImage {
    if let Ok(path) = std::env::var("TEST_IMAGE_PATH") {
        let p = PathBuf::from(&path);
        if p.exists() {
            println!("Using test image: {}", p.display());
            return image::open(&p).expect("Failed to open test image");
        }
        println!("TEST_IMAGE_PATH not found: {}, using generated image", path);
    }

    // 100x75 のグラデーション画像を生成
    println!("Using generated 100x75 gradient test image");
    let mut img = image::RgbImage::new(100, 75);
    for y in 0..75u32 {
        for x in 0..100u32 {
            let r = (x * 255 / 100) as u8;
            let g = (y * 255 / 75) as u8;
            let b = 128;
            img.put_pixel(x, y, image::Rgb([r, g, b]));
        }
    }
    image::DynamicImage::ImageRgb8(img)
}

/// モデルファイルのディレクトリを探す。
fn find_models_dir() -> PathBuf {
    // カレントディレクトリの models/ を探す
    let cwd_models = std::env::current_dir().unwrap().join("models");
    if cwd_models.exists() {
        return cwd_models;
    }

    // %APPDATA%/mimageviewer/models/ を探す
    if let Ok(appdata) = std::env::var("APPDATA") {
        let appdata_models = PathBuf::from(appdata).join("mimageviewer").join("models");
        if appdata_models.exists() {
            return appdata_models;
        }
    }

    panic!("models/ directory not found. Place ONNX models in ./models/ or %APPDATA%/mimageviewer/models/");
}

/// ONNX Runtime + DirectML で各モデルをロードし、タイル1枚で推論できるか確認。
#[test]
fn test_runtime_and_model_loading() {
    let models_dir = find_models_dir();
    println!("Models directory: {}", models_dir.display());

    let runtime = mimageviewer::ai::runtime::AiRuntime::new()
        .expect("Failed to create AiRuntime");

    // 全モデルのロードテスト
    let models = [
        (mimageviewer::ai::ModelKind::ClassifierMobileNet, "anime_classifier_mobilenetv3.onnx"),
        (mimageviewer::ai::ModelKind::UpscaleRealEsrganX4Plus, "realesrgan_x4plus.onnx"),
        (mimageviewer::ai::ModelKind::UpscaleRealEsrganAnime6B, "realesrgan_x4plus_anime_6b.onnx"),
        (mimageviewer::ai::ModelKind::UpscaleRealEsrGeneralV3, "realesr_general_x4v3.onnx"),
        (mimageviewer::ai::ModelKind::UpscaleRealCugan4x, "realcugan_4x_conservative.onnx"),
        (mimageviewer::ai::ModelKind::DenoiseRealplksr, "dejpg_realplksr_otf.onnx"),
        (mimageviewer::ai::ModelKind::InpaintMiGan, "migan.onnx"),
    ];

    for (kind, filename) in &models {
        let path = models_dir.join(filename);
        if !path.exists() {
            println!("SKIP: {} not found", filename);
            continue;
        }
        print!("Loading {:?} ({})... ", kind, filename);
        match runtime.load_model(*kind, &path) {
            Ok(()) => println!("OK"),
            Err(e) => {
                println!("FAIL: {}", e);
                panic!("Model load failed for {:?}: {}", kind, e);
            }
        }
    }
}

/// 分類器テスト: 画像を入力して分類結果が得られるか。
#[test]
fn test_classifier() {
    let models_dir = find_models_dir();
    let classifier_path = models_dir.join("anime_classifier_mobilenetv3.onnx");
    if !classifier_path.exists() {
        println!("SKIP: classifier model not found");
        return;
    }

    let runtime = mimageviewer::ai::runtime::AiRuntime::new().unwrap();
    runtime.load_model(
        mimageviewer::ai::ModelKind::ClassifierMobileNet,
        &classifier_path,
    ).unwrap();

    let img = get_test_image();
    println!("Image size: {}x{}", img.width(), img.height());

    // AI 分類
    let category = mimageviewer::ai::classify::classify(&runtime, &img);
    match category {
        Ok(cat) => println!("Classification result: {:?} ({})", cat, cat.display_label()),
        Err(e) => panic!("Classification failed: {}", e),
    }

    // ヒューリスティクス分類
    let heuristic = mimageviewer::ai::classify::classify_heuristic(&img);
    println!("Heuristic result: {:?} ({})", heuristic, heuristic.display_label());
}

/// 各アップスケールモデルで小さいタイルを推論してみるテスト。
#[test]
fn test_upscale_all_models() {
    let models_dir = find_models_dir();
    let img = get_test_image();

    // 小さくリサイズ（テスト用に高速化）
    let small = img.resize_exact(64, 64, image::imageops::FilterType::Triangle);
    println!("Test image: {}x{}", small.width(), small.height());

    let runtime = mimageviewer::ai::runtime::AiRuntime::new().unwrap();
    let cancel = Arc::new(AtomicBool::new(false));

    let upscale_models = [
        (mimageviewer::ai::ModelKind::UpscaleRealEsrganX4Plus, "realesrgan_x4plus.onnx", 4),
        (mimageviewer::ai::ModelKind::UpscaleRealEsrganAnime6B, "realesrgan_x4plus_anime_6b.onnx", 4),
        (mimageviewer::ai::ModelKind::UpscaleRealEsrGeneralV3, "realesr_general_x4v3.onnx", 4),
        (mimageviewer::ai::ModelKind::UpscaleRealCugan4x, "realcugan_4x_conservative.onnx", 4),
    ];

    for (kind, filename, expected_scale) in &upscale_models {
        let path = models_dir.join(filename);
        if !path.exists() {
            println!("SKIP: {} not found", filename);
            continue;
        }

        println!("\n--- Testing {:?} ({}) ---", kind, filename);
        runtime.load_model(*kind, &path).expect("Model load failed");

        let t0 = std::time::Instant::now();
        let result = mimageviewer::ai::upscale::upscale(
            &runtime,
            *kind,
            &small,
            &cancel,
        );
        let elapsed = t0.elapsed();

        match result {
            Ok(upscaled) => {
                let [w, h] = upscaled.size;
                println!(
                    "OK: {}x{} → {}x{} in {:.2}s",
                    small.width(), small.height(),
                    w, h,
                    elapsed.as_secs_f64()
                );
                assert_eq!(w, small.width() as usize * expected_scale);
                assert_eq!(h, small.height() as usize * expected_scale);
            }
            Err(e) => {
                println!("FAIL: {} ({:.2}s)", e, elapsed.as_secs_f64());
                panic!("Upscale failed for {:?}: {}", kind, e);
            }
        }
    }
}

/// フルサイズ画像でのアップスケールテスト（マルチタイル）。
/// TEST_IMAGE_PATH の画像をそのままのサイズでアップスケールする。
/// 出力画像を target/test_upscale_output.png に保存して目視確認可能。
#[test]
fn test_upscale_full_size() {
    let models_dir = find_models_dir();
    let img = get_test_image();

    // フルサイズのまま（64x64にリサイズしない）
    println!("Full-size test image: {}x{}", img.width(), img.height());

    if img.width() <= 64 && img.height() <= 64 {
        println!("SKIP: Image too small for multi-tile test. Set TEST_IMAGE_PATH to a larger image.");
        return;
    }

    let runtime = mimageviewer::ai::runtime::AiRuntime::new().unwrap();
    let cancel = Arc::new(AtomicBool::new(false));

    // anime_6b で試す（最も軽量な 4x モデル）
    let model_path = models_dir.join("realesrgan_x4plus_anime_6b.onnx");
    if !model_path.exists() {
        println!("SKIP: anime_6b model not found");
        return;
    }

    // Real-CUGAN でテスト（漫画向け）。なければ anime_6b にフォールバック。
    let (kind, model_path) = {
        let cugan_path = models_dir.join("realcugan_4x_conservative.onnx");
        if cugan_path.exists() {
            (mimageviewer::ai::ModelKind::UpscaleRealCugan4x, cugan_path)
        } else {
            let anime_path = models_dir.join("realesrgan_x4plus_anime_6b.onnx");
            if !anime_path.exists() {
                println!("SKIP: no suitable model found");
                return;
            }
            (mimageviewer::ai::ModelKind::UpscaleRealEsrganAnime6B, anime_path)
        }
    };
    runtime.load_model(kind, &model_path).unwrap();

    let t0 = std::time::Instant::now();
    let result = mimageviewer::ai::upscale::upscale(&runtime, kind, &img, &cancel);
    let elapsed = t0.elapsed();

    match result {
        Ok(upscaled) => {
            let [w, h] = upscaled.size;
            println!(
                "Full-size OK: {}x{} → {}x{} in {:.2}s",
                img.width(), img.height(), w, h, elapsed.as_secs_f64()
            );
            assert_eq!(w, img.width() as usize * 4);
            assert_eq!(h, img.height() as usize * 4);

            // 出力画像をファイルに保存して目視確認
            let out_path = std::path::PathBuf::from("target/test_upscale_output.png");
            let mut rgb_buf = image::RgbImage::new(w as u32, h as u32);
            for y in 0..h {
                for x in 0..w {
                    let c = upscaled.pixels[y * w + x];
                    rgb_buf.put_pixel(x as u32, y as u32, image::Rgb([c.r(), c.g(), c.b()]));
                }
            }
            rgb_buf.save(&out_path).unwrap();
            println!("Output saved to: {}", out_path.display());
        }
        Err(e) => panic!("Full-size upscale failed: {}", e),
    }
}

