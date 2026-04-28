//! AI アップスケール/デノイズのタイル単位性能ベンチマーク。
//!
//! 「GPU 使用率 30% 弱」が CPU 前処理待ちか GPU 推論律速かを切り分けるための計測ツール。
//! `upscale::upscale_with_timings()` を呼んで各タイルの extract / infer / blend 時間を
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

use mimageviewer::ai::upscale::UpscaleTimings;
use mimageviewer::ai::{AiBackend, ModelKind, model_manager, runtime::AiRuntime, upscale};
use serde_json::json;

/// 1 (image, model, tile_size) 組み合わせ分の集約結果。後で JSON サマリに出す。
#[derive(Debug, Clone)]
struct BenchRecord {
    image: String,
    image_w: u32,
    image_h: u32,
    model: String,
    tile_size: u32,
    n_tiles: usize,
    runs: usize,
    /// 全ラン平均の wall total (ms)
    wall_total_avg_ms: f64,
    wall_total_min_ms: f64,
    wall_total_max_ms: f64,
    wall_total_stddev_ms: f64,
    /// per-tile 平均の infer 時間 (ms)
    infer_avg_ms: f64,
    /// per-tile 平均の session_run (GPU 計算 + 転送) 時間 (ms)
    session_run_avg_ms: f64,
    /// per-tile 平均の blend 時間 (ms)
    blend_avg_ms: f64,
}

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
    ModelKind::from_str(s).map(|k| (k.as_str(), k))
}

struct Args {
    images: Vec<PathBuf>,
    models: Vec<(&'static str, ModelKind)>,
    runs: usize,
    warmup: usize,
    /// カンマ区切りでタイルサイズを複数指定すると、各サイズで計測してまとめて表示する。
    /// 空のときはモデル既定値 (model_tile_size) で 1 回のみ測定。
    tile_sizes: Vec<u32>,
    /// 出力された ColorImage を PNG として保存するディレクトリ (画質目視比較用)。
    /// ファイル名: <image_stem>__<model>__tile<N>.png
    save_output: Option<PathBuf>,
    /// AI バックエンド (DirectML / TensorRT / CPU)。
    /// TensorRT は %APPDATA%/mimageviewer/tensorrt/ に pack が展開されている前提。
    backend: AiBackend,
    /// JSON 形式のサマリ出力先 (--json /path/to/file.json)。
    /// 後で複数バックエンド/モデルの結果を比較しやすくするため。
    json_output: Option<PathBuf>,
    /// `--legacy-direct-trt`: Phase 2 の旧挙動 (main プロセスで直接 TRT ロード、
    /// worker 不使用) で実行する。Phase 3 と Phase 2 のパフォーマンス比較用。
    legacy_direct_trt: bool,
}

fn parse_args() -> Args {
    let raw: Vec<String> = std::env::args().skip(1).collect();
    let mut images: Vec<PathBuf> = Vec::new();
    let mut models: Vec<(&'static str, ModelKind)> = Vec::new();
    let mut runs: usize = 3;
    let mut warmup: usize = 1;
    let mut tile_sizes: Vec<u32> = Vec::new();
    let mut save_output: Option<PathBuf> = None;
    let mut backend: AiBackend = AiBackend::DirectMl;
    let mut json_output: Option<PathBuf> = None;
    let mut legacy_direct_trt: bool = false;

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
            "--tile-size" => {
                i += 1;
                for s in raw[i].split(',') {
                    let n: u32 = s
                        .trim()
                        .parse()
                        .expect("--tile-size expects comma-separated integers");
                    tile_sizes.push(n);
                }
            }
            "--save-output" => {
                i += 1;
                save_output = Some(PathBuf::from(&raw[i]));
            }
            "--backend" => {
                i += 1;
                backend = AiBackend::from_str(&raw[i]).unwrap_or_else(|| {
                    panic!(
                        "unknown backend: {} (expected: directml, tensorrt, cpu)",
                        &raw[i]
                    )
                });
            }
            "--json" => {
                i += 1;
                json_output = Some(PathBuf::from(&raw[i]));
            }
            "--legacy-direct-trt" => {
                legacy_direct_trt = true;
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

    Args {
        images,
        models,
        runs,
        warmup,
        tile_sizes,
        save_output,
        backend,
        json_output,
        legacy_direct_trt,
    }
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
    println!("  --tile-size <a,b,c>     Comma-separated tile sizes to sweep (overrides");
    println!("                          model_tile_size). Fixed-size models may fail.");
    println!("  --save-output <DIR>     Save the last measured run's output PNG per");
    println!("                          (image, model, tile_size) for visual comparison.");
    println!("  --backend <NAME>        AI backend: directml (default), tensorrt, cpu");
    println!("                          tensorrt requires the pack to be installed at");
    println!("                          %APPDATA%/mimageviewer/tensorrt/ (run scripts/setup-tensorrt-pack.ps1)");
    println!("  --json <PATH>           Write a JSON summary of all (image,model,tile) results");
    println!("                          for later cross-backend comparison");
    println!("  --help, -h              Show this help\n");
    println!("Model names:");
    println!("  realesrgan_x4plus, realesrgan_anime6b, realesr_general_v3,");
    println!("  realcugan_4x, nmkd_siax_4x, denoise_realplksr");
}

fn main() {
    // ロガー初期化 (DLL preload 等の診断ログを %APPDATA%/mimageviewer/logs/ に出す)
    mimageviewer::logger::init();

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
    println!("  warmup:  {}", args.warmup);
    println!("  runs:    {}", args.runs);
    println!("  backend: {}", args.backend.as_str());
    if args.backend == AiBackend::TensorRt {
        println!("    fp16:  on (hardcoded、画質劣化は知覚不能、1.5-2x 高速化)");
    }
    println!();

    model_manager::ensure_models_extracted();
    let mm = model_manager::ModelManager::new();

    // Phase 3 アーキテクチャ: メインは常に DirectML、TRT は別プロセスのワーカー。
    // --legacy-direct-trt 時のみ、Phase 2 互換で main で直接 TRT を init する。
    // (Phase 3 vs Phase 2 のパフォーマンス比較用)
    let runtime = if args.legacy_direct_trt && args.backend == AiBackend::TensorRt {
        println!("[legacy] AiRuntime::new_with_backend(TensorRt) — main で直接 TRT 初期化");
        AiRuntime::new_with_backend(AiBackend::TensorRt)
            .expect("AiRuntime::new_with_backend(TensorRt) [legacy]")
    } else {
        AiRuntime::new_with_backend(AiBackend::DirectMl)
            .expect("AiRuntime::new_with_backend(DirectMl)")
    };
    let runtime = std::sync::Arc::new(runtime);

    if args.backend == AiBackend::TensorRt && !args.legacy_direct_trt {
        // bench_ai は別 bin なので current_exe() は bench_ai.exe を返す。
        // ワーカーは mimageviewer.exe (--tensorrt-infer-worker subcommand を持つ
        // バイナリ) でなければならないため、sibling の mimageviewer.exe を指す。
        let bench_exe = std::env::current_exe().expect("current_exe");
        let main_exe = bench_exe
            .parent()
            .map(|d| d.join("mimageviewer.exe"))
            .expect("sibling path");
        if !main_exe.exists() {
            eprintln!(
                "ERROR: ワーカー用 mimageviewer.exe が見つからない: {}",
                main_exe.display()
            );
            eprintln!("  cargo build --release でメインバイナリも一緒にビルドしてから再実行してください。");
            std::process::exit(1);
        }
        println!("Spawning TRT worker pool (sync, worker={})...", main_exe.display());
        let t0 = std::time::Instant::now();
        match mimageviewer::ai::trt_worker_pool::TrtWorkerPool::start_with_exe(&main_exe) {
            Ok(pool) => {
                let elapsed = t0.elapsed().as_secs_f64();
                println!("  worker pool ready in {elapsed:.2} s");
                runtime.attach_worker_pool(std::sync::Arc::new(pool));
            }
            Err(e) => {
                eprintln!("ERROR: failed to start TRT worker pool: {e}");
                eprintln!("  TensorRT pack may be missing or broken at %APPDATA%/mimageviewer/tensorrt/.");
                eprintln!("  Run scripts/setup-tensorrt-pack.ps1 to install it, then retry.");
                std::process::exit(1);
            }
        }
    }

    let active = runtime.active_backend();
    println!(
        "AiRuntime ready (main backend: requested={:?}, effective={:?}, dll={})",
        active.requested,
        active.effective,
        active.dll_path.display()
    );
    if runtime.has_worker_pool() {
        println!("  TRT worker pool: attached (Upscale/Denoise route to worker)");
    }
    if let Some(reason) = &active.fallback_reason {
        println!("  fallback reason: {}", reason);
    }
    println!();

    // 先に全モデルをロード (セッション初期化コストを計測対象外にする)
    for (label, model) in &args.models {
        let path = mm
            .model_path(*model)
            .unwrap_or_else(|| panic!("model not found: {label}"));
        runtime
            .load_model(*model, &path)
            .unwrap_or_else(|e| panic!("load_model {label}: {e}"));
        println!("loaded model: {}", label);
    }
    println!();

    let cancel = Arc::new(AtomicBool::new(false));

    // JSON 出力用に全レコードを集める
    let mut records: Vec<BenchRecord> = Vec::new();

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
        println!(
            "  size: {}x{}  color: {:?}",
            img.width(),
            img.height(),
            img.color()
        );
        println!("================================================================");

        for (label, model) in &args.models {
            // タイルサイズ指定なしの場合はモデル既定値で 1 回だけ測定、
            // 指定ありなら各サイズで測定して並べる。
            let sweep: Vec<Option<u32>> = if args.tile_sizes.is_empty() {
                vec![None]
            } else {
                args.tile_sizes.iter().map(|&s| Some(s)).collect()
            };

            for ts in &sweep {
                // warmup (固定入力モデル等で失敗した場合はこの tile_size をスキップ)
                let mut warmup_failed = false;
                for _ in 0..args.warmup {
                    if let Err(e) =
                        upscale::upscale_with_timings(&runtime, *model, &img, &cancel, *ts)
                    {
                        eprintln!("  skip [{}] tile_size={:?}: {}", label, ts, e);
                        warmup_failed = true;
                        break;
                    }
                }
                if warmup_failed {
                    continue;
                }

                // measured runs。 --save-output 指定時は最後のランの出力を PNG に保存。
                let mut runs_timings: Vec<UpscaleTimings> = Vec::new();
                let mut run_failed = false;
                let mut last_output: Option<egui::ColorImage> = None;
                for run_idx in 0..args.runs {
                    match upscale::upscale_with_timings(&runtime, *model, &img, &cancel, *ts) {
                        Ok((out, t)) => {
                            runs_timings.push(t);
                            if args.save_output.is_some() && run_idx + 1 == args.runs {
                                last_output = Some(out);
                            }
                        }
                        Err(e) => {
                            eprintln!("  skip [{}] tile_size={:?} mid-run: {}", label, ts, e);
                            run_failed = true;
                            break;
                        }
                    }
                }
                if run_failed || runs_timings.is_empty() {
                    continue;
                }

                // 出力 PNG 保存
                if let (Some(dir), Some(out)) = (args.save_output.as_ref(), last_output.as_ref()) {
                    let stem = img_path
                        .file_stem()
                        .and_then(|s| s.to_str())
                        .unwrap_or("image");
                    let ts_tag = match ts {
                        Some(n) => format!("tile{}", n),
                        None => String::from("tiledefault"),
                    };
                    let filename = format!("{}__{}__{}.png", stem, label, ts_tag);
                    let path = dir.join(&filename);
                    if let Err(e) = save_color_image_png(out, &path) {
                        eprintln!("  failed to save {}: {}", path.display(), e);
                    } else {
                        println!("  saved: {}", path.display());
                    }
                }

                let ts_label = match ts {
                    Some(n) => format!("{} (override)", n),
                    None => String::from("default"),
                };
                let mut rec = print_model_summary(label, *model, &runs_timings, &ts_label);
                rec.image = img_path.display().to_string();
                records.push(rec);
                println!();
            }
        }
    }

    // JSON サマリ出力 (--json 指定時)
    if let Some(path) = &args.json_output {
        let summary = json!({
            "backend": args.backend.as_str(),
            "tensorrt_fp16": args.backend == AiBackend::TensorRt,
            "warmup": args.warmup,
            "runs": args.runs,
            "records": records.iter().map(|r| json!({
                "image": r.image,
                "image_w": r.image_w,
                "image_h": r.image_h,
                "model": r.model,
                "tile_size": r.tile_size,
                "n_tiles": r.n_tiles,
                "runs": r.runs,
                "wall_total_avg_ms": r.wall_total_avg_ms,
                "wall_total_min_ms": r.wall_total_min_ms,
                "wall_total_max_ms": r.wall_total_max_ms,
                "wall_total_stddev_ms": r.wall_total_stddev_ms,
                "infer_avg_ms": r.infer_avg_ms,
                "session_run_avg_ms": r.session_run_avg_ms,
                "blend_avg_ms": r.blend_avg_ms,
            })).collect::<Vec<_>>(),
        });
        match std::fs::write(path, serde_json::to_string_pretty(&summary).unwrap()) {
            Ok(()) => println!("JSON summary written: {}", path.display()),
            Err(e) => eprintln!("Failed to write JSON summary {}: {}", path.display(), e),
        }
    }
}

/// `print_model_summary` で出力したサマリの主要数値を BenchRecord として返す。
/// `image_*` と `model` は呼び出し側で詰める (ここでは UpscaleTimings からは取れない)。
fn print_model_summary(
    label: &str,
    _model: ModelKind,
    runs: &[UpscaleTimings],
    tile_size_label: &str,
) -> BenchRecord {
    let n_runs = runs.len();
    let sample = &runs[0];
    let n_tiles = sample.tiles.len();

    // 平均値 (全ラン全タイル)
    let mut sum_extract = 0.0f64;
    let mut sum_infer = 0.0f64;
    let mut sum_blend = 0.0f64;
    let mut sum_tbuild = 0.0f64;
    let mut sum_srun = 0.0f64;
    let mut sum_textract = 0.0f64;
    let mut sum_pcopy = 0.0f64;
    for r in runs {
        for t in &r.tiles {
            sum_extract += t.extract_ms;
            sum_infer += t.infer_ms;
            sum_blend += t.blend_ms;
            sum_tbuild += t.tensor_build_ms;
            sum_srun += t.session_run_ms;
            sum_textract += t.tensor_extract_ms;
            sum_pcopy += t.post_copy_ms;
        }
    }
    let n_total_tiles = (n_runs * n_tiles) as f64;
    let avg_extract = sum_extract / n_total_tiles;
    let avg_infer = sum_infer / n_total_tiles;
    let avg_blend = sum_blend / n_total_tiles;
    let avg_tbuild = sum_tbuild / n_total_tiles;
    let avg_srun = sum_srun / n_total_tiles;
    let avg_textract = sum_textract / n_total_tiles;
    let avg_pcopy = sum_pcopy / n_total_tiles;
    let tile_sum = avg_extract + avg_infer + avg_blend;
    let pct_extract = 100.0 * avg_extract / tile_sum.max(1e-9);
    let pct_infer = 100.0 * avg_infer / tile_sum.max(1e-9);
    let pct_blend = 100.0 * avg_blend / tile_sum.max(1e-9);
    let infer_overhead = avg_infer - (avg_tbuild + avg_srun + avg_textract + avg_pcopy);

    // タイル単位の中央値・最大・最小 (推論時間の分布確認)
    let mut infer_all: Vec<f64> = runs
        .iter()
        .flat_map(|r| r.tiles.iter().map(|t| t.infer_ms))
        .collect();
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

    println!(
        "  [{}] tile_size={}  actual={}px  {}x{} → {}x{} ({}x), tiles/run={}",
        label,
        tile_size_label,
        tile_size,
        in_w,
        in_h,
        in_w * scale,
        in_h * scale,
        scale,
        n_tiles
    );
    println!(
        "    wall total (avg of {} runs): {:8.1} ms",
        n_runs, avg_total
    );
    println!("      prep (decode to rgb8 etc): {:8.1} ms", avg_prep);
    println!("      alpha resample (Lanczos3): {:8.1} ms", avg_alpha);
    println!(
        "      tile loop (sum of tiles):  {:8.1} ms  ({:.1}%)",
        tile_total_ms,
        100.0 * tile_total_ms / avg_total
    );
    println!("      blend wait (after loop):   {:8.1} ms", avg_blend_wait);
    println!("      finalize (pixel convert):  {:8.1} ms", avg_finalize);
    println!("      unaccounted overhead:      {:8.1} ms", overhead_ms);
    println!(
        "    per-tile avg (across {} tile samples):",
        infer_all.len()
    );
    println!(
        "      extract: {:6.2} ms ({:4.1}%)",
        avg_extract, pct_extract
    );
    println!(
        "      infer:   {:6.2} ms ({:4.1}%)   [min {:6.2} / median {:6.2} / max {:6.2}]",
        avg_infer, pct_infer, infer_min, infer_median, infer_max
    );
    println!(
        "        tensor_build:    {:6.3} ms ({:4.1}% of infer)",
        avg_tbuild,
        100.0 * avg_tbuild / avg_infer.max(1e-9)
    );
    println!(
        "        session_run:     {:6.3} ms ({:4.1}% of infer)  <- GPU compute + transfer",
        avg_srun,
        100.0 * avg_srun / avg_infer.max(1e-9)
    );
    println!(
        "        tensor_extract:  {:6.3} ms ({:4.1}% of infer)",
        avg_textract,
        100.0 * avg_textract / avg_infer.max(1e-9)
    );
    println!(
        "        post_copy:       {:6.3} ms ({:4.1}% of infer)",
        avg_pcopy,
        100.0 * avg_pcopy / avg_infer.max(1e-9)
    );
    println!(
        "        (residual):      {:6.3} ms (with_session lock/Mutex etc)",
        infer_overhead
    );
    println!("      blend:   {:6.2} ms ({:4.1}%)", avg_blend, pct_blend);
    println!("      total:   {:6.2} ms", tile_sum);

    // 各ラン総時間の分布 (min / avg / max + stddev + CI)
    let totals: Vec<f64> = runs.iter().map(|r| r.total_ms).collect();
    let min_t = totals.iter().cloned().fold(f64::INFINITY, f64::min);
    let max_t = totals.iter().cloned().fold(f64::NEG_INFINITY, f64::max);
    let n = totals.len() as f64;
    let mean = avg_total;
    let variance = totals.iter().map(|x| (x - mean).powi(2)).sum::<f64>() / n;
    let stddev = variance.sqrt();
    let cv_pct = 100.0 * stddev / mean.max(1e-9);
    // 95% CI (正規近似; t 分布ではないが小 N でもおおまかな目安)
    let ci95 = 1.96 * stddev / n.sqrt();
    println!(
        "    run-total min/avg/max: {:.1} / {:.1} / {:.1} ms",
        min_t, mean, max_t
    );
    println!(
        "    run-total stddev:      {:.1} ms  (CV {:.1}%, 95% CI ±{:.1} ms, N={})",
        stddev,
        cv_pct,
        ci95,
        totals.len()
    );

    BenchRecord {
        image: String::new(),  // 呼び出し側で詰める
        image_w: in_w,
        image_h: in_h,
        model: label.to_string(),
        tile_size,
        n_tiles,
        runs: n_runs,
        wall_total_avg_ms: mean,
        wall_total_min_ms: min_t,
        wall_total_max_ms: max_t,
        wall_total_stddev_ms: stddev,
        infer_avg_ms: avg_infer,
        session_run_avg_ms: avg_srun,
        blend_avg_ms: avg_blend,
    }
}

/// egui::ColorImage を PNG として保存する (RGBA8)。
fn save_color_image_png(img: &egui::ColorImage, path: &std::path::Path) -> Result<(), String> {
    let w = img.size[0] as u32;
    let h = img.size[1] as u32;
    let mut rgba = image::RgbaImage::new(w, h);
    for y in 0..h {
        for x in 0..w {
            let c = img.pixels[(y as usize) * (w as usize) + (x as usize)];
            rgba.put_pixel(x, y, image::Rgba([c.r(), c.g(), c.b(), c.a()]));
        }
    }
    rgba.save(path).map_err(|e| e.to_string())
}
