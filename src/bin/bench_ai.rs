//! AI アップスケール/デノイズのタイル単位性能ベンチマーク。
//!
//! 「GPU 使用率 30% 弱」が CPU 前処理待ちか GPU 推論律速かを切り分けるための計測ツール。
//! `upscale::upscale_with_timings()` を呼び、各タイルの extract / infer / blend 時間を
//! 計測して平均・合計・パーセント内訳を表示する。
//!
//! デフォルトテスト画像:
//!   - testimage/うちのこ/ComfyUI_2_0003.png (大きめ)
//!   - testimage/Sonic Melty _ TuneCore Japan_files/itc316342.png (小さめ)
//!
//! デフォルトモデル:
//!   - realesrgan_anime6b (うちのこに分類器がおそらく選ぶモデル)
//!   - denoise_realplksr (ノイズ除去、別タイルサイズ 256)
//!
//! ```
//! cargo run --release --bin bench_ai
//! cargo run --release --bin bench_ai -- --models realesrgan_x4plus,denoise_realplksr
//! cargo run --release --bin bench_ai -- --image some/path.png --runs 5 --warmup 2
//! ```

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;

use mimageviewer::ai::{model_manager, runtime::AiRuntime, upscale, ModelKind};
use mimageviewer::ai::upscale::{TileTiming, UpscaleTimings};

const DEFAULT_IMAGES: &[&str] = &[
    "testimage/うちのこ/ComfyUI_2_0003.png",
    "testimage/Sonic Melty _ TuneCore Japan_files/itc316342.png",
];

fn default_models() -> Vec<(&'static str, ModelKind)> {
    vec![
        ("realesrgan_anime6b", ModelKind::UpscaleRealEsrganAnime6B),
        ("denoise_realplksr", ModelKind::DenoiseRealplksr),
    ]
}

fn parse_model(s: &str) -> Option<(&'static str, ModelKind)> {
    match s {
        "realesrgan_x4plus" => Some(("realesrgan_x4plus", ModelKind::UpscaleRealEsrganX4Plus)),
        "realesrgan_anime6b" => Some(("realesrgan_anime6b", ModelKind::UpscaleRealEsrganAnime6B)),
        "realesr_general_v3" => Some(("realesr_general_v3", ModelKind::UpscaleRealEsrGeneralV3)),
        "realcugan_4x" => Some(("realcugan_4x", ModelKind::UpscaleRealCugan4x)),
        "nmkd_siax_4x" => Some(("nmkd_siax_4x", ModelKind::UpscaleNmkdSiax4x)),
        "denoise_realplksr" => Some(("denoise_realplksr", ModelKind::DenoiseRealplksr)),
        _ => None,
    }
}

struct Args {
    images: Vec<PathBuf>,
    models: Vec<(&'static str, ModelKind)>,
    runs: usize,
    warmup: usize,
}

fn parse_args() -> Args {
    let raw: Vec<String> = std::env::args().skip(1).collect();
    let mut images: Vec<PathBuf> = Vec::new();
    let mut models: Vec<(&'static str, ModelKind)> = Vec::new();
    let mut runs: usize = 3;
    let mut warmup: usize = 1;

    let mut i = 0;
    while i < raw.len() {
        match raw[i].as_str() {
            "--image" => {
                i += 1;
                images.push(PathBuf::from(&raw[i]));
            }
            "--models" => {
                i += 1;
                for name in raw[i].split(',') {
                    let name = name.trim();
                    match parse_model(name) {
                        Some(m) => models.push(m),
                        None => panic!("unknown model: {name}"),
                    }
                }
            }
            "--runs" => {
                i += 1;
                runs = raw[i].parse().expect("--runs expects integer");
            }
            "--warmup" => {
                i += 1;
                warmup = raw[i].parse().expect("--warmup expects integer");
            }
            "--help" | "-h" => {
                print_help();
                std::process::exit(0);
            }
            other => panic!("unknown arg: {other}"),
        }
        i += 1;
    }

    if images.is_empty() {
        images = DEFAULT_IMAGES.iter().map(PathBuf::from).collect();
    }
    if models.is_empty() {
        models = default_models();
    }

    Args { images, models, runs, warmup }
}

fn print_help() {
    println!("bench_ai: AI upscale/denoise tile-level timing benchmark\n");
    println!("Usage:");
    println!("  cargo run --release --bin bench_ai [OPTIONS]\n");
    println!("Options:");
    println!("  --image <PATH>          Add an image path (can be repeated)");
    println!("  --models <a,b,c>        Comma-separated model names");
    println!("  --runs <N>              Measured runs per (image,model) [default: 3]");
    println!("  --warmup <N>            Warmup runs per (image,model) [default: 1]");
    println!("  --help, -h              Show this help\n");
    println!("Model names:");
    println!("  realesrgan_x4plus, realesrgan_anime6b, realesr_general_v3,");
    println!("  realcugan_4x, nmkd_siax_4x, denoise_realplksr");
}

fn main() {
    let args = parse_args();

    println!("bench_ai");
    println!("  images: {}", args.images.len());
    for p in &args.images {
        println!("    - {}", p.display());
    }
    println!("  models: {}", args.models.len());
    for (label, _) in &args.models {
        println!("    - {}", label);
    }
    println!("  warmup: {}", args.warmup);
    println!("  runs:   {}", args.runs);
    println!();

    model_manager::ensure_models_extracted();
    let mm = model_manager::ModelManager::new();
    let runtime = AiRuntime::new().expect("AiRuntime::new");

    // 先に全モデルをロード (セッション初期化コストを計測対象外にする)
    for (label, model) in &args.models {
        let path = mm.model_path(*model)
            .unwrap_or_else(|| panic!("model not found: {label}"));
        runtime.load_model(*model, &path)
            .unwrap_or_else(|e| panic!("load_model {label}: {e}"));
        println!("loaded model: {}", label);
    }
    println!();

    let cancel = Arc::new(AtomicBool::new(false));

    for img_path in &args.images {
        let img = match image::open(img_path) {
            Ok(i) => i,
            Err(e) => {
                eprintln!("  skip {}: {e}", img_path.display());
                continue;
            }
        };

        println!("================================================================");
        println!("image: {}", img_path.display());
        println!("  size: {}x{}  color: {:?}", img.width(), img.height(), img.color());
        println!("================================================================");

        for (label, model) in &args.models {
            // warmup
            for _ in 0..args.warmup {
                let (_img, _t) = upscale::upscale_with_timings(&runtime, *model, &img, &cancel)
                    .expect("warmup upscale");
            }

            // measured runs
            let mut runs_timings: Vec<UpscaleTimings> = Vec::new();
            for _ in 0..args.runs {
                let (_img, t) = upscale::upscale_with_timings(&runtime, *model, &img, &cancel)
                    .expect("upscale");
                runs_timings.push(t);
            }

            print_model_summary(label, *model, &runs_timings);
            println!();
        }
    }
}

fn print_model_summary(label: &str, _model: ModelKind, runs: &[UpscaleTimings]) {
    let n_runs = runs.len();
    let sample = &runs[0];
    let n_tiles = sample.tiles.len();

    // 平均値 (全ラン全タイル)
    let mut sum_extract = 0.0f64;
    let mut sum_infer = 0.0f64;
    let mut sum_blend = 0.0f64;
    for r in runs {
        for t in &r.tiles {
            sum_extract += t.extract_ms;
            sum_infer += t.infer_ms;
            sum_blend += t.blend_ms;
        }
    }
    let n_total_tiles = (n_runs * n_tiles) as f64;
    let avg_extract = sum_extract / n_total_tiles;
    let avg_infer = sum_infer / n_total_tiles;
    let avg_blend = sum_blend / n_total_tiles;
    let tile_sum = avg_extract + avg_infer + avg_blend;
    let pct_extract = 100.0 * avg_extract / tile_sum.max(1e-9);
    let pct_infer = 100.0 * avg_infer / tile_sum.max(1e-9);
    let pct_blend = 100.0 * avg_blend / tile_sum.max(1e-9);

    // タイル単位の中央値・最大・最小 (推論時間の分布確認)
    let mut infer_all: Vec<f64> = runs.iter().flat_map(|r| r.tiles.iter().map(|t| t.infer_ms)).collect();
    infer_all.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let infer_median = infer_all[infer_all.len() / 2];
    let infer_min = infer_all[0];
    let infer_max = infer_all[infer_all.len() - 1];

    // 全体平均
    let avg_total: f64 = runs.iter().map(|r| r.total_ms).sum::<f64>() / n_runs as f64;
    let avg_prep: f64 = runs.iter().map(|r| r.prep_ms).sum::<f64>() / n_runs as f64;
    let avg_alpha: f64 = runs.iter().map(|r| r.alpha_resample_ms).sum::<f64>() / n_runs as f64;
    let avg_finalize: f64 = runs.iter().map(|r| r.finalize_ms).sum::<f64>() / n_runs as f64;
    let avg_blend_wait: f64 = runs.iter().map(|r| r.blend_wait_ms).sum::<f64>() / n_runs as f64;

    let tile_total_ms = tile_sum * n_tiles as f64;
    let overhead_ms = avg_total - tile_total_ms - avg_alpha - avg_prep - avg_finalize;

    let (in_w, in_h) = (sample.in_w, sample.in_h);
    let scale = sample.scale;
    let tile_size = sample.tile_size;

    println!("  [{}] {}x{} → {}x{} ({}x), tile={}px, tiles/run={}",
             label, in_w, in_h, in_w * scale, in_h * scale, scale, tile_size, n_tiles);
    println!("    wall total (avg of {} runs): {:8.1} ms", n_runs, avg_total);
    println!("      prep (decode to rgb8 etc): {:8.1} ms", avg_prep);
    println!("      alpha resample (Lanczos3): {:8.1} ms", avg_alpha);
    println!("      tile loop (sum of tiles):  {:8.1} ms  ({:.1}%)",
             tile_total_ms, 100.0 * tile_total_ms / avg_total);
    println!("      blend wait (after loop):   {:8.1} ms", avg_blend_wait);
    println!("      finalize (pixel convert):  {:8.1} ms", avg_finalize);
    println!("      unaccounted overhead:      {:8.1} ms", overhead_ms);
    println!("    per-tile avg (across {} tile samples):", infer_all.len());
    println!("      extract: {:6.2} ms ({:4.1}%)", avg_extract, pct_extract);
    println!("      infer:   {:6.2} ms ({:4.1}%)   [min {:6.2} / median {:6.2} / max {:6.2}]",
             avg_infer, pct_infer, infer_min, infer_median, infer_max);
    println!("      blend:   {:6.2} ms ({:4.1}%)", avg_blend, pct_blend);
    println!("      total:   {:6.2} ms", tile_sum);

    // 各ラン総時間の分布
    let totals: Vec<f64> = runs.iter().map(|r| r.total_ms).collect();
    let min_t = totals.iter().cloned().fold(f64::INFINITY, f64::min);
    let max_t = totals.iter().cloned().fold(f64::NEG_INFINITY, f64::max);
    println!("    run-total min/avg/max: {:.1} / {:.1} / {:.1} ms", min_t, avg_total, max_t);
}

// 未使用警告を回避するため (TileTiming は print_* からのみ参照)
#[allow(dead_code)]
fn _touch_types(t: &TileTiming) -> f64 {
    t.extract_ms + t.infer_ms + t.blend_ms
}
