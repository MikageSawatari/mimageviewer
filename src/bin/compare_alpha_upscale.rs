//! AI アップスケールにおけるアルファ処理方式の比較ツール。
//!
//! 透明度を持つ画像に対し、以下 2 方式 × 3 背景パターンで結果を生成する。
//!
//! 方式:
//!   - alpha:     現方式 = AI で RGB をアップスケール + Lanczos3 でアルファをリサイズ
//!                → 再結合した RGBA をあとから背景に合成
//!   - composite: 入力 RGBA を先に背景へ合成 → RGB → AI でアップスケール (アルファ無し)
//!
//! 背景: white / black / checker (16x16, 224/176 グレー)
//!
//! 出力ファイル名: `<stem>_<bg>_<method>.png`
//!
//! ```
//! cargo run --release --bin compare_alpha_upscale
//! cargo run --release --bin compare_alpha_upscale -- testimage/transparent --out tmp/compare
//! cargo run --release --bin compare_alpha_upscale -- --model realesr_general_v3
//! ```

use std::path::PathBuf;
use std::sync::atomic::AtomicBool;
use std::sync::Arc;

use image::{DynamicImage, RgbImage, RgbaImage};
use mimageviewer::ai::{model_manager, runtime::AiRuntime, upscale, ModelKind};

const DEFAULT_SRC: &str = "testimage/transparent";
const DEFAULT_OUT: &str = "testimage/transparent_compare";
const DEFAULT_MODEL: ModelKind = ModelKind::UpscaleRealEsrganX4Plus;

struct Args {
    src_dir: PathBuf,
    out_dir: PathBuf,
    model: ModelKind,
}

fn parse_model(s: &str) -> Option<ModelKind> {
    match s {
        "realesrgan_x4plus" => Some(ModelKind::UpscaleRealEsrganX4Plus),
        "realesrgan_anime6b" => Some(ModelKind::UpscaleRealEsrganAnime6B),
        "realesr_general_v3" => Some(ModelKind::UpscaleRealEsrGeneralV3),
        "realcugan_4x" => Some(ModelKind::UpscaleRealCugan4x),
        "nmkd_siax_4x" => Some(ModelKind::UpscaleNmkdSiax4x),
        _ => None,
    }
}

fn parse_args() -> Args {
    let raw: Vec<String> = std::env::args().skip(1).collect();
    let mut src_dir = None;
    let mut out_dir = None;
    let mut model = DEFAULT_MODEL;
    let mut i = 0;
    while i < raw.len() {
        match raw[i].as_str() {
            "--out" => {
                i += 1;
                out_dir = Some(PathBuf::from(&raw[i]));
            }
            "--model" => {
                i += 1;
                model = parse_model(&raw[i]).expect("unknown model name");
            }
            other if !other.starts_with("--") && src_dir.is_none() => {
                src_dir = Some(PathBuf::from(other));
            }
            other => panic!("unknown arg: {other}"),
        }
        i += 1;
    }
    Args {
        src_dir: src_dir.unwrap_or_else(|| PathBuf::from(DEFAULT_SRC)),
        out_dir: out_dir.unwrap_or_else(|| PathBuf::from(DEFAULT_OUT)),
        model,
    }
}

fn main() {
    let args = parse_args();
    std::fs::create_dir_all(&args.out_dir).expect("create out dir");

    println!("Source: {}", args.src_dir.display());
    println!("Output: {}", args.out_dir.display());
    println!("Model:  {:?}", args.model);

    model_manager::ensure_models_extracted();
    let mm = model_manager::ModelManager::new();
    let model_path = mm.model_path(args.model).expect("model path not found");
    let runtime = AiRuntime::new().expect("AiRuntime::new");
    runtime.load_model(args.model, &model_path).expect("load_model");

    let mut entries: Vec<PathBuf> = std::fs::read_dir(&args.src_dir)
        .expect("read_dir")
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.is_file())
        .filter(|p| {
            let ext = p.extension().and_then(|s| s.to_str()).unwrap_or("").to_lowercase();
            matches!(ext.as_str(), "png" | "webp" | "tiff" | "tif")
        })
        .collect();
    entries.sort();

    let cancel = Arc::new(AtomicBool::new(false));
    let bgs = ["white", "black", "checker"];

    for path in &entries {
        let stem = path.file_stem().unwrap().to_string_lossy().to_string();
        println!("\n=== {} ===", stem);

        let img = match image::open(path) {
            Ok(i) => i,
            Err(e) => {
                eprintln!("  load failed: {e}");
                continue;
            }
        };
        println!("  size: {}x{}, color: {:?}", img.width(), img.height(), img.color());

        // Method A: 現方式 (RGBA upscale を 1 回だけ実行 → 各背景に合成)
        let upscaled_a = upscale::upscale(&runtime, args.model, &img, &cancel)
            .expect("upscale A failed");
        let out_w = upscaled_a.size[0] as u32;
        let out_h = upscaled_a.size[1] as u32;

        for bg in bgs {
            // A: 既存方式: AI 出力 RGBA を背景に合成
            let bg_out = make_bg(bg, out_w, out_h);
            let composited = composite_color_image_over(&upscaled_a, &bg_out);
            let out_path = args.out_dir.join(format!("{stem}_{bg}_alpha.png"));
            composited.save(&out_path).expect("save A");
            println!("  saved {}", out_path.display());

            // B: 入力を背景に合成してから upscale
            let bg_in = make_bg(bg, img.width(), img.height());
            let composited_in = composite_rgba_over(&img.to_rgba8(), &bg_in);
            let composited_dyn = DynamicImage::ImageRgb8(composited_in);
            let upscaled_b = upscale::upscale(&runtime, args.model, &composited_dyn, &cancel)
                .expect("upscale B failed");
            let rgb_out = color_image_to_rgb(&upscaled_b);
            let out_path = args.out_dir.join(format!("{stem}_{bg}_composite.png"));
            rgb_out.save(&out_path).expect("save B");
            println!("  saved {}", out_path.display());
        }
    }

    println!("\nDone. {} images processed.", entries.len());
}

fn make_bg(kind: &str, w: u32, h: u32) -> RgbImage {
    match kind {
        "white" => RgbImage::from_pixel(w, h, image::Rgb([255, 255, 255])),
        "black" => RgbImage::from_pixel(w, h, image::Rgb([0, 0, 0])),
        "checker" => {
            let mut img = RgbImage::new(w, h);
            for y in 0..h {
                for x in 0..w {
                    let cell = ((x / 8) + (y / 8)) % 2;
                    let v: u8 = if cell == 0 { 224 } else { 176 };
                    img.put_pixel(x, y, image::Rgb([v, v, v]));
                }
            }
            img
        }
        _ => panic!("unknown bg: {kind}"),
    }
}

/// ColorImage (premultiplied RGBA) を RGB 背景の上に合成して RgbImage を返す。
fn composite_color_image_over(fg: &egui::ColorImage, bg: &RgbImage) -> RgbImage {
    let w = bg.width();
    let h = bg.height();
    assert_eq!(fg.size, [w as usize, h as usize]);
    let mut out = RgbImage::new(w, h);
    for y in 0..h {
        for x in 0..w {
            let bg_px = bg.get_pixel(x, y);
            let fg_px = fg.pixels[(y as usize) * (w as usize) + (x as usize)];
            let a = fg_px.a() as f32 / 255.0;
            let inv = 1.0 - a;
            // fg は premultiplied なので加算合成
            let r = (fg_px.r() as f32 + bg_px.0[0] as f32 * inv).clamp(0.0, 255.0) as u8;
            let g = (fg_px.g() as f32 + bg_px.0[1] as f32 * inv).clamp(0.0, 255.0) as u8;
            let b = (fg_px.b() as f32 + bg_px.0[2] as f32 * inv).clamp(0.0, 255.0) as u8;
            out.put_pixel(x, y, image::Rgb([r, g, b]));
        }
    }
    out
}

/// 非 premultiplied な RGBA を RGB 背景の上に合成する (B 方式の入力作成用)。
fn composite_rgba_over(fg: &RgbaImage, bg: &RgbImage) -> RgbImage {
    let w = bg.width();
    let h = bg.height();
    assert_eq!(fg.dimensions(), bg.dimensions());
    let mut out = RgbImage::new(w, h);
    for y in 0..h {
        for x in 0..w {
            let bg_px = bg.get_pixel(x, y);
            let fg_px = fg.get_pixel(x, y);
            let a = fg_px.0[3] as f32 / 255.0;
            let inv = 1.0 - a;
            let r = (fg_px.0[0] as f32 * a + bg_px.0[0] as f32 * inv).clamp(0.0, 255.0) as u8;
            let g = (fg_px.0[1] as f32 * a + bg_px.0[1] as f32 * inv).clamp(0.0, 255.0) as u8;
            let b = (fg_px.0[2] as f32 * a + bg_px.0[2] as f32 * inv).clamp(0.0, 255.0) as u8;
            out.put_pixel(x, y, image::Rgb([r, g, b]));
        }
    }
    out
}

fn color_image_to_rgb(ci: &egui::ColorImage) -> RgbImage {
    let w = ci.size[0] as u32;
    let h = ci.size[1] as u32;
    let mut out = RgbImage::new(w, h);
    for y in 0..h {
        for x in 0..w {
            let c = ci.pixels[(y as usize) * (w as usize) + (x as usize)];
            out.put_pixel(x, y, image::Rgb([c.r(), c.g(), c.b()]));
        }
    }
    out
}
