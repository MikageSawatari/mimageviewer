//! 見開き欠損補完ベンチマーク。
//!
//! 実際の PDF ページを使い、複数の inpainting モデルの品質と速度を比較する。
//!
//! Usage:
//!   cargo run --release --bin bench_inpaint -- <pdf_path> <left_page> <right_page> [gap_width] [trim]
//!
//! Example:
//!   cargo run --release --bin bench_inpaint -- "D:\home\scan\illust\book.pdf" 77 78 40 5
//!
//! 出力: bench_inpaint_*.png (各モデルの補完結果 + 比較画像)

use std::path::{Path, PathBuf};
use std::time::Instant;

fn models_dir() -> PathBuf {
    mimageviewer::data_dir::get().join("models")
}

/// モデル定義
struct ModelDef {
    name: &'static str,
    filename: &'static str,
    /// 入力サイズ (0 = 動的)
    input_size: u32,
    /// image 入力名
    image_input: &'static str,
    /// mask 入力名 ("" = combined_input)
    mask_input: &'static str,
    /// 出力値域 (true = [0,255], false = [0,1])
    output_0_255: bool,
    /// CPU 専用 (DirectML 非互換)
    cpu_only: bool,
    /// 入力が [1, 4, H, W] (RGB+mask 結合) かどうか
    combined_input: bool,
}

const MODELS: &[ModelDef] = &[
    // ── LaMa 系 ──
    ModelDef {
        name: "LaMa FP32 512 (Carve)",
        filename: "lama_fp32.onnx",

        input_size: 512,
        image_input: "image",
        mask_input: "mask",
        output_0_255: true,
        cpu_only: false,
        combined_input: false,
    },
    ModelDef {
        name: "LaMa FP16 512 (converted)",
        filename: "lama_fp16.onnx",

        input_size: 512,
        image_input: "image",
        mask_input: "mask",
        output_0_255: true,
        cpu_only: false,
        combined_input: false,
    },
    ModelDef {
        name: "LaMa Dynamic FP32",
        filename: "lama_dynamic_fp32.onnx",

        input_size: 0,
        image_input: "image",
        mask_input: "mask",
        output_0_255: true,
        cpu_only: false,
        combined_input: false,
    },
    ModelDef {
        name: "LaMa Dynamic FP16",
        filename: "lama_dynamic_fp16.onnx",

        input_size: 0,
        image_input: "image",
        mask_input: "mask",
        output_0_255: true,
        cpu_only: false,
        combined_input: false,
    },
    // ── 代替モデル ──
    ModelDef {
        name: "MI-GAN (lxfater)",
        filename: "migan.onnx",

        input_size: 512,
        image_input: "input",
        mask_input: "",
        output_0_255: false,
        cpu_only: false,
        combined_input: true,
    },
    ModelDef {
        name: "DeepFillv2",
        filename: "deepfillv2.onnx",

        input_size: 512,
        image_input: "input",
        mask_input: "",
        output_0_255: false,
        cpu_only: false,
        combined_input: true, // 5ch 入力だが run_benchmark 内で特別処理
    },
];

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.len() < 3 {
        eprintln!("Usage: bench_inpaint <pdf_path> <left_page> <right_page> [gap_width] [trim]");
        std::process::exit(1);
    }

    let pdf_path = PathBuf::from(&args[0]);
    let left_page: u32 = args[1].parse().expect("left_page must be a number");
    let right_page: u32 = args[2].parse().expect("right_page must be a number");
    let gap_width: u32 = args.get(3).and_then(|s| s.parse().ok()).unwrap_or(40);
    let trim: u32 = args.get(4).and_then(|s| s.parse().ok()).unwrap_or(5);

    println!("PDF: {}", pdf_path.display());
    println!("Pages: {} (left), {} (right)", left_page, right_page);
    println!("Gap: {}px, Trim: {}px", gap_width, trim);

    // ── PDF ページのレンダリング ──
    println!("\nRendering PDF pages...");
    let left_img = render_pdf_page(&pdf_path, left_page);
    let right_img = render_pdf_page(&pdf_path, right_page);
    println!("  Left:  {}x{}", left_img.width(), left_img.height());
    println!("  Right: {}x{}", right_img.width(), right_img.height());

    // 入力画像を保存
    left_img.save("bench_inpaint_left.png").expect("save left");
    right_img.save("bench_inpaint_right.png").expect("save right");

    // ── モデルの確認・ダウンロード ──
    let dir = models_dir();
    std::fs::create_dir_all(&dir).ok();

    for model in MODELS {
        let path = dir.join(model.filename);
        if !path.exists() {
            panic!(
                "Model not found: {}\nExpected at: {}\nRun the app once to extract embedded models, or place the file manually.",
                model.name,
                path.display()
            );
        }
    }

    // ── 各モデルでベンチマーク ──
    for model in MODELS {
        println!("\n============================================================");
        println!("Model: {}", model.name);
        println!("============================================================");

        let path = dir.join(model.filename);
        if !path.exists() {
            println!("  SKIP: model file not found");
            continue;
        }

        // CPU セッション
        run_benchmark(model, &path, &left_img, &right_img, gap_width, trim, false);

        // DirectML セッション（cpu_only でなければ）
        if !model.cpu_only {
            run_benchmark(model, &path, &left_img, &right_img, gap_width, trim, true);
        }
    }

    println!("\nDone. Check bench_inpaint_*.png files.");
}

fn run_benchmark(
    model: &ModelDef,
    model_path: &Path,
    left_img: &image::DynamicImage,
    right_img: &image::DynamicImage,
    gap_width: u32,
    trim: u32,
    use_dml: bool,
) {
    let device = if use_dml { "DirectML" } else { "CPU" };
    println!("\n  [{device}] Loading model...");

    let t_load = Instant::now();
    let mut session = match create_session(model_path, use_dml) {
        Ok(s) => s,
        Err(e) => {
            println!("  [{device}] FAILED to load: {e}");
            return;
        }
    };
    let load_ms = t_load.elapsed().as_secs_f64() * 1000.0;
    println!("  [{device}] Loaded in {load_ms:.0}ms");

    // モデル入出力情報
    println!("  Inputs:");
    for input in session.inputs().iter() {
        println!("    {:?}", input);
    }
    println!("  Outputs:");
    for output in session.outputs().iter() {
        println!("    {:?}", output);
    }

    // 入力サイズ決定 (0 = 動的、ストリップサイズをそのまま使用)
    let is_dynamic = model.input_size == 0;

    // ストリップ構築
    let lw = left_img.width();
    let rw = right_img.width();
    let lh = left_img.height();
    let rh = right_img.height();
    let combined_h = lh.max(rh);

    let left_trim = trim.min(lw / 4);
    let right_trim = trim.min(rw / 4);
    let ctx = 256u32.min(lw.saturating_sub(left_trim)).min(rw.saturating_sub(right_trim));
    let strip_w = ctx + left_trim + gap_width + right_trim + ctx;

    // 動的モデル: ストリップを 64 の倍数にパディング（FFC 層の 8x ダウンサンプリング + FFT 偶数要件）
    let (input_w, input_h) = if is_dynamic {
        let pad64 = |v: u32| (v + 63) & !63;
        (pad64(strip_w), pad64(combined_h))
    } else {
        (model.input_size, model.input_size)
    };
    println!("  Strip: {}x{} → {}x{} ({})", strip_w, combined_h, input_w, input_h,
        if is_dynamic { "dynamic" } else { "fixed" });

    // ストリップ RGB [0,1]
    let left_rgba = left_img.to_rgba8();
    let right_rgba = right_img.to_rgba8();

    let mut strip = vec![0.0f32; (strip_w * combined_h * 3) as usize];

    // 左コンテキスト + 左トリム
    let left_total = ctx + left_trim;
    let left_src_x0 = lw - left_total;
    for y in 0..lh.min(combined_h) {
        for x in 0..left_total {
            let p = left_rgba.get_pixel(left_src_x0 + x, y);
            let base = ((y * strip_w + x) * 3) as usize;
            strip[base] = p[0] as f32 / 255.0;
            strip[base + 1] = p[1] as f32 / 255.0;
            strip[base + 2] = p[2] as f32 / 255.0;
        }
    }

    // 右トリム + 右コンテキスト
    let right_total = right_trim + ctx;
    let right_dst_x0 = left_total + gap_width;
    for y in 0..rh.min(combined_h) {
        for x in 0..right_total {
            let p = right_rgba.get_pixel(x, y);
            let base = ((y * strip_w + right_dst_x0 + x) * 3) as usize;
            strip[base] = p[0] as f32 / 255.0;
            strip[base + 1] = p[1] as f32 / 255.0;
            strip[base + 2] = p[2] as f32 / 255.0;
        }
    }

    // リサイズ (固定サイズ) またはパディング (動的サイズ)
    let lama_rgb = resize_rgb_bilinear(&strip, strip_w, combined_h, input_w, input_h);

    // マスク
    let scale_x = input_w as f32 / strip_w as f32;
    let mask_x0 = (ctx as f32 * scale_x).round() as u32;
    let mask_x1 = (((ctx + left_trim + gap_width + right_trim) as f32) * scale_x)
        .round()
        .min(input_w as f32) as u32;

    let iw = input_w as usize;
    let ih = input_h as usize;

    // 推論
    println!("  [{device}] Running inference ({}x{}, mask x=[{}..{}])...",
        input_w, input_h, mask_x0, mask_x1);

    let t_infer = Instant::now();
    let result = if model.filename.contains("deepfillv2") {
        // DeepFillv2 形式: [1, 5, H, W]
        // ch0-2: image [-1,1] * (1-mask) (マスク領域は 0)
        // ch3: ones (全 1.0)
        // ch4: mask (1=inpaint, 0=known)
        let mut combined = ndarray::Array4::<f32>::zeros((1, 5, ih, iw));
        for y in 0..ih {
            for x in 0..iw {
                let base = (y * iw + x) * 3;
                let is_masked = x >= mask_x0 as usize && x < mask_x1 as usize;
                let m = if is_masked { 1.0f32 } else { 0.0f32 }; // 1=inpaint
                let r = lama_rgb[base] * 2.0 - 1.0;
                let g = lama_rgb[base + 1] * 2.0 - 1.0;
                let b = lama_rgb[base + 2] * 2.0 - 1.0;
                combined[[0, 0, y, x]] = r * (1.0 - m);
                combined[[0, 1, y, x]] = g * (1.0 - m);
                combined[[0, 2, y, x]] = b * (1.0 - m);
                combined[[0, 3, y, x]] = 1.0; // ones
                combined[[0, 4, y, x]] = m;   // mask
            }
        }
        let tensor = ort::value::Tensor::from_array(combined).expect("combined tensor");
        session.run(ort::inputs![model.image_input => tensor])
    } else if model.combined_input {
        // MI-GAN 形式: [1, 4, H, W]
        // ch0: mask - 0.5 (0=inpaint→-0.5, 1=known→0.5)
        // ch1-3: image [-1,1] * mask
        let mut combined = ndarray::Array4::<f32>::zeros((1, 4, ih, iw));
        for y in 0..ih {
            for x in 0..iw {
                let base = (y * iw + x) * 3;
                let is_masked = x >= mask_x0 as usize && x < mask_x1 as usize;
                let m = if is_masked { 0.0f32 } else { 1.0f32 }; // 0=inpaint, 1=known
                combined[[0, 0, y, x]] = m - 0.5;
                let r = lama_rgb[base] * 2.0 - 1.0;
                let g = lama_rgb[base + 1] * 2.0 - 1.0;
                let b = lama_rgb[base + 2] * 2.0 - 1.0;
                combined[[0, 1, y, x]] = r * m;
                combined[[0, 2, y, x]] = g * m;
                combined[[0, 3, y, x]] = b * m;
            }
        }
        let tensor = ort::value::Tensor::from_array(combined).expect("combined tensor");
        session.run(ort::inputs![model.image_input => tensor])
    } else {
        // LaMa 形式: image [1, 3, H, W] + mask [1, 1, H, W]
        let mut image_nchw = ndarray::Array4::<f32>::zeros((1, 3, ih, iw));
        for y in 0..ih {
            for x in 0..iw {
                let base = (y * iw + x) * 3;
                image_nchw[[0, 0, y, x]] = lama_rgb[base];
                image_nchw[[0, 1, y, x]] = lama_rgb[base + 1];
                image_nchw[[0, 2, y, x]] = lama_rgb[base + 2];
            }
        }
        let mut mask_nchw = ndarray::Array4::<f32>::zeros((1, 1, ih, iw));
        for y in 0..ih {
            for x in mask_x0 as usize..mask_x1 as usize {
                mask_nchw[[0, 0, y, x]] = 1.0;
            }
        }
        let image_tensor = ort::value::Tensor::from_array(image_nchw).expect("image tensor");
        let mask_tensor = ort::value::Tensor::from_array(mask_nchw).expect("mask tensor");
        session.run(ort::inputs![model.image_input => image_tensor, model.mask_input => mask_tensor])
    };
    let infer_ms = t_infer.elapsed().as_secs_f64() * 1000.0;

    match result {
        Ok(outputs) => {
            println!("  [{device}] Inference: {infer_ms:.1}ms");

            match outputs[0].try_extract_tensor::<f32>() {
                Ok((_shape, raw)) => {
                    // 出力値を [0,1] に変換するクロージャ
                    let normalize_output = |v: f32| -> f32 {
                        if model.output_0_255 {
                            v / 255.0 // [0,255] → [0,1]
                        } else if model.combined_input || model.filename.contains("deepfillv2") {
                            v * 0.5 + 0.5 // [-1,1] → [0,1] (MI-GAN / DeepFillv2)
                        } else {
                            v // already [0,1]
                        }
                    };

                    // gap 部分を取り出し
                    let gap_w = (mask_x1 - mask_x0) as usize;
                    let mut gap_rgb = vec![0.0f32; gap_w * ih * 3];
                    for y in 0..ih {
                        for x in 0..gap_w {
                            let src_x = mask_x0 as usize + x;
                            let dst = (y * gap_w + x) * 3;
                            gap_rgb[dst] = normalize_output(raw.get(0 * ih * iw + y * iw + src_x).copied().unwrap_or(0.0));
                            gap_rgb[dst + 1] = normalize_output(raw.get(1 * ih * iw + y * iw + src_x).copied().unwrap_or(0.0));
                            gap_rgb[dst + 2] = normalize_output(raw.get(2 * ih * iw + y * iw + src_x).copied().unwrap_or(0.0));
                        }
                    }

                    // 元解像度にリサイズ
                    let inpaint_w = left_trim + gap_width + right_trim;
                    let inpaint_full = resize_rgb_bilinear(
                        &gap_rgb, gap_w as u32, input_h, inpaint_w, combined_h,
                    );

                    // 合成画像を生成
                    let total_w = (lw - left_trim) + inpaint_w + (rw - right_trim);
                    let mut out_img = image::RgbImage::new(total_w, combined_h);
                    for y in 0..combined_h {
                        let mut ox = 0u32;
                        // 左ページ
                        for x in 0..(lw - left_trim) {
                            if y < lh {
                                let p = left_rgba.get_pixel(x, y);
                                out_img.put_pixel(ox, y, image::Rgb([p[0], p[1], p[2]]));
                            }
                            ox += 1;
                        }
                        // 補完部分
                        for x in 0..inpaint_w {
                            let base = ((y * inpaint_w + x) * 3) as usize;
                            if base + 2 < inpaint_full.len() {
                                let r = (inpaint_full[base] * 255.0).clamp(0.0, 255.0) as u8;
                                let g = (inpaint_full[base + 1] * 255.0).clamp(0.0, 255.0) as u8;
                                let b = (inpaint_full[base + 2] * 255.0).clamp(0.0, 255.0) as u8;
                                out_img.put_pixel(ox, y, image::Rgb([r, g, b]));
                            }
                            ox += 1;
                        }
                        // 右ページ
                        for x in right_trim..rw {
                            if y < rh {
                                let p = right_rgba.get_pixel(x, y);
                                out_img.put_pixel(ox, y, image::Rgb([p[0], p[1], p[2]]));
                            }
                            ox += 1;
                        }
                    }

                    let suffix = model.filename.replace(".onnx", "");
                    let filename = format!("bench_inpaint_{suffix}_{device}.png");
                    out_img.save(&filename).expect("save output");
                    println!("  [{device}] Saved: {filename} ({}x{})", total_w, combined_h);
                }
                Err(e) => println!("  [{device}] Extract failed: {e}"),
            }
        }
        Err(e) => {
            println!("  [{device}] FAILED: {e}");
        }
    }
}

fn create_session(path: &Path, use_dml: bool) -> Result<ort::session::Session, String> {
    let mut builder = ort::session::Session::builder()
        .map_err(|e| format!("builder: {e}"))?;

    if use_dml {
        // DirectML 必須設定: メモリパターン無効 + 逐次実行
        builder = builder
            .with_memory_pattern(false)
            .map_err(|e| format!("mem_pattern: {e}"))?
            .with_parallel_execution(false)
            .map_err(|e| format!("parallel: {e}"))?
            .with_optimization_level(ort::session::builder::GraphOptimizationLevel::Level1)
            .map_err(|e| format!("opt: {e}"))?
            .with_intra_threads(1)
            .map_err(|e| format!("threads: {e}"))?;

        builder = match builder.with_execution_providers([ort::ep::DirectML::default().build()]) {
            Ok(b) => b,
            Err(e) => {
                println!("    DirectML EP failed: {e}, falling back to CPU");
                e.recover()
            }
        };
    } else {
        builder = builder
            .with_optimization_level(ort::session::builder::GraphOptimizationLevel::Level3)
            .map_err(|e| format!("opt: {e}"))?
            .with_intra_threads(4)
            .map_err(|e| format!("threads: {e}"))?;
    }

    builder.commit_from_file(path).map_err(|e| format!("load: {e}"))
}

fn render_pdf_page(pdf_path: &Path, page_num: u32) -> image::DynamicImage {
    // まずメインアプリで事前レンダリングされた PNG を試す
    let png_path = PathBuf::from(format!("bench_page_{}.png", page_num));
    if png_path.exists() {
        return image::open(&png_path).expect("Failed to open pre-rendered PNG");
    }

    // PDFium ワーカープールは使えないので、直接 PDFium を初期化して描画
    let dll_path = mimageviewer::data_dir::get().join("pdfium.dll");
    if !dll_path.exists() {
        eprintln!("pdfium.dll not found at {}", dll_path.display());
        eprintln!("Alternatively, pre-render pages as bench_page_<N>.png");
        std::process::exit(1);
    }

    let dll_dir = dll_path.parent().unwrap();
    let bindings = pdfium_render::prelude::Pdfium::bind_to_library(
        pdfium_render::prelude::Pdfium::pdfium_platform_library_name_at_path(
            dll_dir.to_str().unwrap(),
        ),
    ).expect("PDFium bind failed");
    let pdfium = pdfium_render::prelude::Pdfium::new(bindings);

    let doc = pdfium.load_pdf_from_file(pdf_path, None)
        .expect("Failed to open PDF");
    let page = doc.pages().get(page_num as u16).expect("Page not found");

    let config = pdfium_render::prelude::PdfRenderConfig::new()
        .set_target_width(2000)
        .set_maximum_height(8000);
    let bitmap = page.render_with_config(&config).expect("Render failed");
    let img = bitmap.as_image();

    // 後で再利用できるように保存
    img.save(&png_path).expect("save page png");
    println!("  Saved pre-rendered page: {}", png_path.display());
    img
}

fn resize_rgb_bilinear(src: &[f32], src_w: u32, src_h: u32, dst_w: u32, dst_h: u32) -> Vec<f32> {
    let mut dst = vec![0.0f32; (dst_w * dst_h * 3) as usize];
    let x_ratio = src_w as f32 / dst_w as f32;
    let y_ratio = src_h as f32 / dst_h as f32;
    for dy in 0..dst_h {
        let sy = dy as f32 * y_ratio;
        let y0 = (sy as u32).min(src_h.saturating_sub(1));
        let y1 = (y0 + 1).min(src_h.saturating_sub(1));
        let fy = sy - y0 as f32;
        for dx in 0..dst_w {
            let sx = dx as f32 * x_ratio;
            let x0 = (sx as u32).min(src_w.saturating_sub(1));
            let x1 = (x0 + 1).min(src_w.saturating_sub(1));
            let fx = sx - x0 as f32;
            for c in 0..3u32 {
                let v00 = src[((y0 * src_w + x0) * 3 + c) as usize];
                let v10 = src[((y0 * src_w + x1) * 3 + c) as usize];
                let v01 = src[((y1 * src_w + x0) * 3 + c) as usize];
                let v11 = src[((y1 * src_w + x1) * 3 + c) as usize];
                dst[((dy * dst_w + dx) * 3 + c) as usize] =
                    v00 * (1.0 - fx) * (1.0 - fy) + v10 * fx * (1.0 - fy)
                    + v01 * (1.0 - fx) * fy + v11 * fx * fy;
            }
        }
    }
    dst
}
