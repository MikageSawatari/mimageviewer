//! BiRefNet 被写体分離モデルの検証 probe (開発専用、配布物ではない)。
//!
//! 編集用追加パックに同梱する被写体分離モデル (BiRefNet fp16) を、
//! DirectML EP で実際にロード・推論し、以下を確定する:
//!   - 入力/出力テンソルの名前・dtype・shape (fp16 か fp32 か、1024² か)
//!   - cold / warm の推論時間 (RTX 4090 実機)
//!   - 出力マットの妥当性 (sigmoid 適用要否、値域)
//!
//! 使い方:
//!   cargo run --release --bin probe_birefnet -- <model.onnx> <image> [input_size=1024]
//!
//! 出力: <image stem>_matte_sigmoid.png / _matte_raw.png を入力画像と同じディレクトリに保存。

use std::path::PathBuf;

use mimageviewer::ai::ModelKind;
use mimageviewer::ai::runtime::AiRuntime;

fn sigmoid(x: f32) -> f32 {
    1.0 / (1.0 + (-x).exp())
}

fn main() {
    if let Err(e) = run() {
        eprintln!("ERROR: {e}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), String> {
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 3 {
        eprintln!("usage: probe_birefnet <model.onnx> <image> [input_size=1024]");
        std::process::exit(2);
    }
    let model_path = PathBuf::from(&args[1]);
    let image_path = PathBuf::from(&args[2]);
    let size: usize = args.get(3).and_then(|s| s.parse().ok()).unwrap_or(1024);
    println!("model     : {}", model_path.display());
    println!("image     : {}", image_path.display());
    println!("input size: {size}x{size}");

    // --- テスト画像読み込み + 前処理 (1024² resize, ImageNet 正規化, NCHW) ---
    let img = image::open(&image_path)
        .map_err(|e| format!("open image: {e}"))?
        .to_rgb8();
    let (iw, ih) = img.dimensions();
    println!("source image: {iw}x{ih}");
    let resized = image::imageops::resize(
        &img,
        size as u32,
        size as u32,
        image::imageops::FilterType::Triangle,
    );
    let mean = [0.485_f32, 0.456, 0.406];
    let std = [0.229_f32, 0.224, 0.225];
    let mut input = ndarray::Array4::<f32>::zeros((1, 3, size, size));
    for y in 0..size {
        for x in 0..size {
            let p = resized.get_pixel(x as u32, y as u32).0;
            for c in 0..3 {
                input[[0, c, y, x]] = (p[c] as f32 / 255.0 - mean[c]) / std[c];
            }
        }
    }

    // --- DirectML ランタイムでモデルロード ---
    println!("creating DirectML AiRuntime...");
    let rt = AiRuntime::new().map_err(|e| format!("AiRuntime::new: {e}"))?;
    let t_load = std::time::Instant::now();
    rt.load_model(ModelKind::SubjectMatte, &model_path)
        .map_err(|e| format!("load_model (DirectML): {e}"))?;
    println!(
        "model loaded (DirectML) in {:.0} ms",
        t_load.elapsed().as_secs_f64() * 1000.0
    );

    // --- 2 回推論 (cold=shader compile 込み / warm) ---
    let mut last: Option<(Vec<i64>, Vec<f32>)> = None;
    for pass in 0..2 {
        let input_tensor = ort::value::Tensor::from_array(input.clone())
            .map_err(|e| format!("Tensor::from_array (pass {pass}): {e}"))?;
        let result = rt
            .with_session(ModelKind::SubjectMatte, |session| {
                if pass == 0 {
                    println!("--- session io ---");
                    for i in session.inputs() {
                        println!("  INPUT  {i:?}");
                    }
                    for o in session.outputs() {
                        println!("  OUTPUT {o:?}");
                    }
                    println!("------------------");
                }
                let t = std::time::Instant::now();
                let outputs = session
                    .run(ort::inputs![input_tensor])
                    .map_err(|e| mimageviewer::ai::AiError::Ort(format!("run: {e}")))?;
                let run_ms = t.elapsed().as_secs_f64() * 1000.0;
                let (shape, raw) = outputs[0]
                    .try_extract_tensor::<f32>()
                    .map_err(|e| mimageviewer::ai::AiError::Ort(format!("extract: {e}")))?;
                Ok((
                    shape.iter().copied().collect::<Vec<i64>>(),
                    raw.to_vec(),
                    run_ms,
                ))
            })
            .map_err(|e| e.to_string())?;
        let (shape, raw, run_ms) = result;
        println!("pass {pass}: output shape {shape:?}, run {run_ms:.0} ms");
        last = Some((shape, raw));
    }

    // --- マット保存 (warm pass) ---
    let (shape, raw) = last.ok_or("no inference result")?;
    let n = shape.len();
    if n < 2 {
        return Err(format!("unexpected output rank: {shape:?}"));
    }
    let ow = shape[n - 1] as usize;
    let oh = shape[n - 2] as usize;
    let off = raw.len().saturating_sub(ow * oh);
    let vals = &raw[off..off + ow * oh];
    let (mn, mx) = vals
        .iter()
        .fold((f32::INFINITY, f32::NEG_INFINITY), |(a, b), &v| {
            (a.min(v), b.max(v))
        });
    println!("raw output range: [{mn:.4}, {mx:.4}]  (output {ow}x{oh})");

    let save = |name: &str, f: &dyn Fn(f32) -> f32| -> Result<(), String> {
        let mut buf = image::GrayImage::new(ow as u32, oh as u32);
        for y in 0..oh {
            for x in 0..ow {
                let v = f(vals[y * ow + x]).clamp(0.0, 1.0);
                buf.put_pixel(x as u32, y as u32, image::Luma([(v * 255.0) as u8]));
            }
        }
        let stem = image_path
            .file_stem()
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_else(|| "out".to_string());
        let out = image_path.with_file_name(format!("{stem}_{name}.png"));
        buf.save(&out).map_err(|e| format!("save {name}: {e}"))?;
        println!("saved {}", out.display());
        Ok(())
    };
    save("matte_sigmoid", &sigmoid)?;
    save("matte_raw", &|v| v)?;
    println!("done. sigmoid 版 / raw 版を目視で比較して、正しい後処理を判定してください。");
    Ok(())
}
