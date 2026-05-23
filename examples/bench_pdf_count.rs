//! PDF page count benchmark: PDFium (in-process) vs lopdf (pure Rust)
//!
//! Walks the given folder for `*.pdf` files and times the page count operation
//! through both backends. Each PDF is read once from disk first so that both
//! backends compete on warm OS cache (the realistic comparison — cold disk
//! latency dominates anything CPU). The result is a per-file table plus
//! aggregate stats.
//!
//! Usage:
//!   cargo run --release --example bench_pdf_count -- <folder>
//!   cargo run --release --example bench_pdf_count -- --recursive <folder>
//!   cargo run --release --example bench_pdf_count -- --json <folder>
//!
//! Notes:
//! - lopdf failures (encrypted PDF without password, corrupt files, unsupported
//!   variants) are counted separately and the file is marked `LOPDF_ERR`. The
//!   PDFium column still runs to give a reference baseline.
//! - Page count mismatches between the two backends are flagged with `!`.
//! - We open PDFium once (DLL load + binding setup) and reuse the `Pdfium`
//!   instance for all files in the run.

use mimageviewer::pdf_loader::PDF_WORKER_ARG;
use pdfium_render::prelude::*;
use std::path::{Path, PathBuf};
use std::time::Instant;

fn main() {
    // mimageviewer は同じ binary を `--pdf-worker` 引数で worker プロセスとして起動する。
    // bench_pdf_count は別 binary なので worker 引数は受け取らない想定だが、
    // 万が一渡されたら exit して PDFium pool が混乱しないようにする。
    let args: Vec<String> = std::env::args().collect();
    if args.iter().any(|a| a == PDF_WORKER_ARG) {
        eprintln!("bench_pdf_count: not a worker binary; remove --pdf-worker");
        std::process::exit(2);
    }

    let mut recursive = false;
    let mut json_out = false;
    let mut folder: Option<PathBuf> = None;
    for arg in args.iter().skip(1) {
        match arg.as_str() {
            "--recursive" | "-r" => recursive = true,
            "--json" => json_out = true,
            "--help" | "-h" => {
                print_help();
                return;
            }
            other => {
                if folder.is_none() {
                    folder = Some(PathBuf::from(other));
                } else {
                    eprintln!("Unexpected argument: {other}");
                    print_help();
                    std::process::exit(2);
                }
            }
        }
    }
    let folder = match folder {
        Some(p) => p,
        None => {
            eprintln!("Missing folder argument");
            print_help();
            std::process::exit(2);
        }
    };

    if !folder.is_dir() {
        eprintln!("Not a directory: {}", folder.display());
        std::process::exit(1);
    }

    let pdfs = collect_pdfs(&folder, recursive);
    if pdfs.is_empty() {
        eprintln!("No PDFs found in {}", folder.display());
        std::process::exit(1);
    }
    if !json_out {
        println!(
            "Found {} PDFs in {}{}",
            pdfs.len(),
            folder.display(),
            if recursive { " (recursive)" } else { "" },
        );
    }

    // PDFium 初期化 (1 回)
    let pdfium = match init_pdfium() {
        Ok(p) => p,
        Err(e) => {
            eprintln!("PDFium init failed: {e}");
            std::process::exit(1);
        }
    };

    let mut results: Vec<BenchResult> = Vec::with_capacity(pdfs.len());

    if !json_out {
        println!(
            "{:>12}  {:>11}  {:>10}  {:>6}  {}",
            "pdfium_ms", "lopdf_ms", "speedup", "pages", "file",
        );
        println!("{}", "-".repeat(80));
    }

    for pdf in &pdfs {
        // OS disk cache を warm up (両 backend が同じ条件で競うように)
        let _ = std::fs::read(pdf);

        // PDFium
        let (pdfium_ms, pdfium_count, pdfium_err) = time_pdfium(&pdfium, pdf);

        // lopdf
        let (lopdf_ms, lopdf_count, lopdf_err) = time_lopdf(pdf);

        let counts_match = match (pdfium_count, lopdf_count) {
            (Some(a), Some(b)) => a == b,
            _ => false,
        };

        let result = BenchResult {
            path: pdf.clone(),
            file_size: std::fs::metadata(pdf).map(|m| m.len()).unwrap_or(0),
            pdfium_ms,
            pdfium_count,
            pdfium_err: pdfium_err.clone(),
            lopdf_ms,
            lopdf_count,
            lopdf_err: lopdf_err.clone(),
        };

        if !json_out {
            let name = pdf.file_name().and_then(|n| n.to_str()).unwrap_or("?");
            let truncated_name = if name.chars().count() > 50 {
                format!("{}...", name.chars().take(47).collect::<String>())
            } else {
                name.to_string()
            };

            let pages_col = match pdfium_count.or(lopdf_count) {
                Some(n) => format!("{}", n),
                None => "-".to_string(),
            };
            let mark = if counts_match || pdfium_err.is_some() || lopdf_err.is_some() {
                " "
            } else {
                "!"
            };

            let speedup = match (pdfium_ms, lopdf_ms) {
                (Some(p), Some(l)) if l > 0.001 => format!("{:.1}x", p / l),
                _ => "-".to_string(),
            };

            let pdfium_str = match pdfium_ms {
                Some(t) => format!("{:.1}", t),
                None => "ERR".to_string(),
            };
            let lopdf_str = match lopdf_ms {
                Some(t) => format!("{:.1}", t),
                None => "ERR".to_string(),
            };

            println!(
                "{:>12}  {:>11}  {:>10}  {:>6}{}  {}",
                pdfium_str, lopdf_str, speedup, pages_col, mark, truncated_name,
            );
        }
        results.push(result);
    }

    if json_out {
        // JSON output: per-file results + aggregate
        let mut buf = String::new();
        buf.push_str("{\n  \"results\": [\n");
        for (i, r) in results.iter().enumerate() {
            buf.push_str("    {");
            buf.push_str(&format!("\"path\":{:?},", r.path.to_string_lossy()));
            buf.push_str(&format!("\"file_size\":{},", r.file_size));
            if let Some(t) = r.pdfium_ms {
                buf.push_str(&format!("\"pdfium_ms\":{:.3},", t));
            } else {
                buf.push_str("\"pdfium_ms\":null,");
            }
            if let Some(c) = r.pdfium_count {
                buf.push_str(&format!("\"pdfium_count\":{},", c));
            } else {
                buf.push_str("\"pdfium_count\":null,");
            }
            if let Some(t) = r.lopdf_ms {
                buf.push_str(&format!("\"lopdf_ms\":{:.3},", t));
            } else {
                buf.push_str("\"lopdf_ms\":null,");
            }
            if let Some(c) = r.lopdf_count {
                buf.push_str(&format!("\"lopdf_count\":{}", c));
            } else {
                buf.push_str("\"lopdf_count\":null");
            }
            buf.push('}');
            if i + 1 < results.len() {
                buf.push(',');
            }
            buf.push('\n');
        }
        buf.push_str("  ]\n}\n");
        print!("{buf}");
        return;
    }

    println!();
    print_aggregate(&results);
}

fn print_help() {
    eprintln!("Usage: bench_pdf_count [--recursive] [--json] <folder>");
    eprintln!();
    eprintln!("Times PDF page count via PDFium (in-process) and lopdf (pure Rust)");
    eprintln!("for each PDF in <folder>. Each file is read once first to warm OS cache.");
}

fn collect_pdfs(folder: &Path, recursive: bool) -> Vec<PathBuf> {
    let mut out = Vec::new();
    collect_pdfs_inner(folder, recursive, &mut out);
    out.sort();
    out
}

fn collect_pdfs_inner(folder: &Path, recursive: bool, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(folder) else {
        return;
    };
    for entry in entries.flatten() {
        let ft = match entry.file_type() {
            Ok(t) => t,
            Err(_) => continue,
        };
        if ft.is_dir() {
            if recursive {
                collect_pdfs_inner(&entry.path(), recursive, out);
            }
            continue;
        }
        if !ft.is_file() {
            continue;
        }
        let path = entry.path();
        if path
            .extension()
            .and_then(|e| e.to_str())
            .map(|e| e.eq_ignore_ascii_case("pdf"))
            .unwrap_or(false)
        {
            out.push(path);
        }
    }
}

fn init_pdfium() -> Result<Pdfium, String> {
    // PDFium DLL は %APPDATA%/mimageviewer/pdfium.dll に展開済みのものを使う想定
    // (= 通常の mImageViewer を一度起動済みの環境)。bench を独立して走らせる場合は
    // pdf_loader::ensure_dll_extracted を経由しないので、それが既に居ることを前提。
    let data_dir = mimageviewer::data_dir::get();
    let dll_dir = data_dir
        .to_str()
        .ok_or("non-UTF8 data_dir path")?
        .to_string();
    let bindings = Pdfium::bind_to_library(Pdfium::pdfium_platform_library_name_at_path(&dll_dir))
        .map_err(|e| format!("PDFium binding failed: {e} (data_dir={dll_dir})"))?;
    Ok(Pdfium::new(bindings))
}

fn time_pdfium(pdfium: &Pdfium, path: &Path) -> (Option<f64>, Option<u32>, Option<String>) {
    let t = Instant::now();
    match pdfium.load_pdf_from_file(path, None) {
        Ok(doc) => {
            let count = doc.pages().len() as u32;
            let ms = t.elapsed().as_secs_f64() * 1000.0;
            (Some(ms), Some(count), None)
        }
        Err(e) => {
            // 暗号化 PDF など。`time` は計測しても意味が薄いが念のため返す。
            let ms = t.elapsed().as_secs_f64() * 1000.0;
            (Some(ms), None, Some(format!("{e}")))
        }
    }
}

fn time_lopdf(path: &Path) -> (Option<f64>, Option<u32>, Option<String>) {
    let t = Instant::now();
    let doc = match lopdf::Document::load(path) {
        Ok(d) => d,
        Err(e) => {
            let ms = t.elapsed().as_secs_f64() * 1000.0;
            return (Some(ms), None, Some(format!("load: {e}")));
        }
    };
    // Catalog → /Pages → /Count を直接引く (= O(1), Pages tree leaf 走査しない)
    let count = match lopdf_page_count(&doc) {
        Ok(c) => c,
        Err(e) => {
            let ms = t.elapsed().as_secs_f64() * 1000.0;
            return (Some(ms), None, Some(format!("count: {e}")));
        }
    };
    let ms = t.elapsed().as_secs_f64() * 1000.0;
    (Some(ms), Some(count), None)
}

fn lopdf_page_count(doc: &lopdf::Document) -> Result<u32, String> {
    let root_ref = doc
        .trailer
        .get(b"Root")
        .map_err(|e| format!("trailer.Root: {e}"))?
        .as_reference()
        .map_err(|e| format!("Root as_reference: {e}"))?;
    let catalog = doc
        .get_object(root_ref)
        .map_err(|e| format!("get_object(Root): {e}"))?
        .as_dict()
        .map_err(|e| format!("Catalog as_dict: {e}"))?;
    let pages_ref = catalog
        .get(b"Pages")
        .map_err(|e| format!("Catalog.Pages: {e}"))?
        .as_reference()
        .map_err(|e| format!("Pages as_reference: {e}"))?;
    let pages = doc
        .get_object(pages_ref)
        .map_err(|e| format!("get_object(Pages): {e}"))?
        .as_dict()
        .map_err(|e| format!("Pages as_dict: {e}"))?;
    let count = pages
        .get(b"Count")
        .map_err(|e| format!("Pages.Count: {e}"))?
        .as_i64()
        .map_err(|e| format!("Count as_i64: {e}"))?;
    if count < 0 {
        return Err(format!("negative page count: {count}"));
    }
    Ok(count as u32)
}

struct BenchResult {
    path: PathBuf,
    file_size: u64,
    pdfium_ms: Option<f64>,
    pdfium_count: Option<u32>,
    pdfium_err: Option<String>,
    lopdf_ms: Option<f64>,
    lopdf_count: Option<u32>,
    lopdf_err: Option<String>,
}

fn print_aggregate(results: &[BenchResult]) {
    let n = results.len();
    let pdfium_times: Vec<f64> = results.iter().filter_map(|r| r.pdfium_ms).collect();
    let lopdf_times: Vec<f64> = results.iter().filter_map(|r| r.lopdf_ms).collect();
    let lopdf_ok: Vec<&BenchResult> = results.iter().filter(|r| r.lopdf_count.is_some()).collect();
    let lopdf_err: Vec<&BenchResult> = results.iter().filter(|r| r.lopdf_err.is_some()).collect();
    let pdfium_err: Vec<&BenchResult> = results.iter().filter(|r| r.pdfium_err.is_some()).collect();

    println!("=== Aggregate ===");
    println!("Total PDFs:          {n}");
    println!("  PDFium errors:     {}", pdfium_err.len());
    println!("  lopdf errors:      {}", lopdf_err.len());
    println!(
        "  lopdf success:     {} ({:.1}%)",
        lopdf_ok.len(),
        100.0 * lopdf_ok.len() as f64 / n as f64,
    );

    if !pdfium_times.is_empty() {
        let stats = percentiles(&pdfium_times);
        println!(
            "PDFium ms:  min={:.1}  p50={:.1}  p90={:.1}  p95={:.1}  p99={:.1}  max={:.1}  mean={:.1}",
            stats.min, stats.p50, stats.p90, stats.p95, stats.p99, stats.max, stats.mean,
        );
    }
    if !lopdf_times.is_empty() {
        let stats = percentiles(&lopdf_times);
        println!(
            "lopdf  ms:  min={:.1}  p50={:.1}  p90={:.1}  p95={:.1}  p99={:.1}  max={:.1}  mean={:.1}",
            stats.min, stats.p50, stats.p90, stats.p95, stats.p99, stats.max, stats.mean,
        );
    }

    // Page count mismatches (both succeeded but counts differ)
    let mismatches: Vec<&BenchResult> = results
        .iter()
        .filter(|r| matches!((r.pdfium_count, r.lopdf_count), (Some(a), Some(b)) if a != b))
        .collect();
    if !mismatches.is_empty() {
        println!();
        println!(
            "Page count mismatches: {} file(s) (PDFium != lopdf)",
            mismatches.len()
        );
        for r in mismatches.iter().take(10) {
            println!(
                "  pdfium={:?} lopdf={:?}  {}",
                r.pdfium_count,
                r.lopdf_count,
                r.path.file_name().and_then(|n| n.to_str()).unwrap_or("?"),
            );
        }
    }

    // Speedups for files where both succeeded
    let speedups: Vec<f64> = results
        .iter()
        .filter_map(|r| match (r.pdfium_ms, r.lopdf_ms) {
            (Some(p), Some(l)) if l > 0.001 => Some(p / l),
            _ => None,
        })
        .collect();
    if !speedups.is_empty() {
        let stats = percentiles(&speedups);
        println!();
        println!(
            "Speedup (PDFium / lopdf):  min={:.1}x  p50={:.1}x  p90={:.1}x  max={:.1}x  mean={:.1}x",
            stats.min, stats.p50, stats.p90, stats.max, stats.mean,
        );
    }

    // Slowest PDFium files
    let mut by_pdfium: Vec<&BenchResult> =
        results.iter().filter(|r| r.pdfium_ms.is_some()).collect();
    by_pdfium.sort_by(|a, b| {
        b.pdfium_ms
            .unwrap()
            .partial_cmp(&a.pdfium_ms.unwrap())
            .unwrap()
    });
    println!();
    println!("Top 5 slowest by PDFium:");
    for r in by_pdfium.iter().take(5) {
        let speedup = match (r.pdfium_ms, r.lopdf_ms) {
            (Some(p), Some(l)) if l > 0.001 => format!("{:.1}x", p / l),
            _ => "-".into(),
        };
        println!(
            "  pdfium={:>8.1}ms  lopdf={:>8.1}ms  speedup={:>6}  {}",
            r.pdfium_ms.unwrap(),
            r.lopdf_ms.unwrap_or(-1.0),
            speedup,
            r.path.file_name().and_then(|n| n.to_str()).unwrap_or("?"),
        );
    }

    if !lopdf_err.is_empty() {
        println!();
        println!("Top 5 lopdf errors:");
        for r in lopdf_err.iter().take(5) {
            println!(
                "  {}  err={:?}",
                r.path.file_name().and_then(|n| n.to_str()).unwrap_or("?"),
                r.lopdf_err.as_deref().unwrap_or("?"),
            );
        }
    }
}

struct Stats {
    min: f64,
    p50: f64,
    p90: f64,
    p95: f64,
    p99: f64,
    max: f64,
    mean: f64,
}

fn percentiles(values: &[f64]) -> Stats {
    let mut sorted = values.to_vec();
    sorted.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let n = sorted.len();
    let pct = |p: f64| -> f64 {
        let idx = ((n - 1) as f64 * p).round() as usize;
        sorted[idx.min(n - 1)]
    };
    let mean = sorted.iter().sum::<f64>() / n as f64;
    Stats {
        min: sorted[0],
        p50: pct(0.50),
        p90: pct(0.90),
        p95: pct(0.95),
        p99: pct(0.99),
        max: sorted[n - 1],
        mean,
    }
}
