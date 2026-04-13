//! ZIP サムネイル取得のベンチマーク。
//!
//! 使い方:
//!   cargo run --release --bin bench_zip -- "E:\share\18\VNCG.org\Soredemo Tsuma Wo Aishiteru (60u).zip"
//!
//! 複数ファイルを指定すると順番に計測。
//! --parallel N で N 並列読み込みもテスト。

use std::io::{BufReader, Read};
use std::path::Path;
use std::time::Instant;

const IMAGE_EXTS: &[&str] = &["jpg", "jpeg", "png", "webp", "bmp", "gif"];

fn bench_one(zip_path: &Path) {
    let file_size = std::fs::metadata(zip_path).map(|m| m.len()).unwrap_or(0);
    println!(
        "\n=== {} ({:.1} MB) ===",
        zip_path.file_name().and_then(|n| n.to_str()).unwrap_or("?"),
        file_size as f64 / (1024.0 * 1024.0),
    );

    // --- 計測 1: BufReader 8KB (デフォルト) ---
    {
        let t = Instant::now();
        let file = std::fs::File::open(zip_path).unwrap();
        let t_open = t.elapsed();
        let mut archive = zip::ZipArchive::new(BufReader::new(file)).unwrap();
        let t_archive = t.elapsed();
        let entry_count = archive.len();

        let mut found_idx: Option<(usize, String)> = None;
        for i in 0..entry_count {
            let Ok(entry) = archive.by_index_raw(i) else { continue };
            if !entry.is_file() { continue; }
            let name = entry.name().to_string();
            if name.contains("__MACOSX/") || name.starts_with('.') { continue; }
            let Some(dot) = name.rfind('.') else { continue };
            let ext = name[dot + 1..].to_ascii_lowercase();
            if IMAGE_EXTS.contains(&ext.as_str()) {
                found_idx = Some((i, name.replace('\\', "/")));
                break;
            }
        }
        let t_scan = t.elapsed();

        if let Some((idx, ref _name)) = found_idx {
            let mut entry = archive.by_index(idx).unwrap();
            let mut bytes = Vec::with_capacity(entry.size() as usize);
            entry.read_to_end(&mut bytes).unwrap();
            let t_read = t.elapsed();
            println!(
                "  [8KB buf]  entries={entry_count}  open={:.1}ms  archive_new={:.1}ms  scan={:.1}ms  read={:.1}ms  total={:.1}ms  ({} bytes)",
                t_open.as_secs_f64() * 1000.0,
                (t_archive - t_open).as_secs_f64() * 1000.0,
                (t_scan - t_archive).as_secs_f64() * 1000.0,
                (t_read - t_scan).as_secs_f64() * 1000.0,
                t_read.as_secs_f64() * 1000.0,
                bytes.len(),
            );
        } else {
            println!("  [8KB buf]  entries={entry_count}  no image found  total={:.1}ms", t.elapsed().as_secs_f64() * 1000.0);
        }
    }

    // --- 計測 2: BufReader 256KB ---
    {
        let t = Instant::now();
        let file = std::fs::File::open(zip_path).unwrap();
        let t_open = t.elapsed();
        let mut archive = zip::ZipArchive::new(BufReader::with_capacity(256 * 1024, file)).unwrap();
        let t_archive = t.elapsed();
        let entry_count = archive.len();

        let mut found_idx: Option<(usize, String)> = None;
        for i in 0..entry_count {
            let Ok(entry) = archive.by_index_raw(i) else { continue };
            if !entry.is_file() { continue; }
            let name = entry.name().to_string();
            if name.contains("__MACOSX/") || name.starts_with('.') { continue; }
            let Some(dot) = name.rfind('.') else { continue };
            let ext = name[dot + 1..].to_ascii_lowercase();
            if IMAGE_EXTS.contains(&ext.as_str()) {
                found_idx = Some((i, name.replace('\\', "/")));
                break;
            }
        }
        let t_scan = t.elapsed();

        if let Some((idx, ref _name)) = found_idx {
            let mut entry = archive.by_index(idx).unwrap();
            let mut bytes = Vec::with_capacity(entry.size() as usize);
            entry.read_to_end(&mut bytes).unwrap();
            let t_read = t.elapsed();
            println!(
                "  [256KB buf] entries={entry_count}  open={:.1}ms  archive_new={:.1}ms  scan={:.1}ms  read={:.1}ms  total={:.1}ms  ({} bytes)",
                t_open.as_secs_f64() * 1000.0,
                (t_archive - t_open).as_secs_f64() * 1000.0,
                (t_scan - t_archive).as_secs_f64() * 1000.0,
                (t_read - t_scan).as_secs_f64() * 1000.0,
                t_read.as_secs_f64() * 1000.0,
                bytes.len(),
            );
        } else {
            println!("  [256KB buf] entries={entry_count}  no image found  total={:.1}ms", t.elapsed().as_secs_f64() * 1000.0);
        }
    }

    // --- 計測 3: 全部メモリに読んでから処理 ---
    {
        let t = Instant::now();
        let data = std::fs::read(zip_path).unwrap();
        let t_read_all = t.elapsed();
        let cursor = std::io::Cursor::new(&data);
        let mut archive = zip::ZipArchive::new(cursor).unwrap();
        let t_archive = t.elapsed();
        let entry_count = archive.len();

        let mut found_idx: Option<(usize, String)> = None;
        for i in 0..entry_count {
            let Ok(entry) = archive.by_index_raw(i) else { continue };
            if !entry.is_file() { continue; }
            let name = entry.name().to_string();
            if name.contains("__MACOSX/") || name.starts_with('.') { continue; }
            let Some(dot) = name.rfind('.') else { continue };
            let ext = name[dot + 1..].to_ascii_lowercase();
            if IMAGE_EXTS.contains(&ext.as_str()) {
                found_idx = Some((i, name.replace('\\', "/")));
                break;
            }
        }
        let t_scan = t.elapsed();

        if let Some((idx, ref _name)) = found_idx {
            let mut entry = archive.by_index(idx).unwrap();
            let mut bytes = Vec::with_capacity(entry.size() as usize);
            entry.read_to_end(&mut bytes).unwrap();
            let t_decompress = t.elapsed();
            println!(
                "  [in-memory] entries={entry_count}  fs_read={:.1}ms  archive_new={:.1}ms  scan={:.1}ms  decompress={:.1}ms  total={:.1}ms  ({} bytes)",
                t_read_all.as_secs_f64() * 1000.0,
                (t_archive - t_read_all).as_secs_f64() * 1000.0,
                (t_scan - t_archive).as_secs_f64() * 1000.0,
                (t_decompress - t_scan).as_secs_f64() * 1000.0,
                t_decompress.as_secs_f64() * 1000.0,
                bytes.len(),
            );
        } else {
            println!("  [in-memory] entries={entry_count}  no image found  total={:.1}ms", t.elapsed().as_secs_f64() * 1000.0);
        }
    }
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.is_empty() {
        eprintln!("Usage: bench_zip <zip_file> [zip_file2] ...");
        eprintln!("       bench_zip --parallel N <zip_file1> <zip_file2> ...");
        std::process::exit(1);
    }

    let mut parallel = 0usize;
    let mut files: Vec<&str> = Vec::new();
    let mut i = 0;
    while i < args.len() {
        if args[i] == "--parallel" {
            i += 1;
            parallel = args.get(i).and_then(|s| s.parse().ok()).unwrap_or(4);
        } else {
            files.push(&args[i]);
        }
        i += 1;
    }

    // 直列テスト
    println!("--- Sequential ---");
    for f in &files {
        bench_one(Path::new(f));
    }

    // 並列テスト
    if parallel > 0 && files.len() > 1 {
        println!("\n\n--- Parallel ({parallel} threads) ---");
        let paths: Vec<std::path::PathBuf> = files.iter().map(|f| Path::new(f).to_path_buf()).collect();
        let t = Instant::now();
        let handles: Vec<_> = paths.into_iter().enumerate().map(|(idx, p)| {
            std::thread::spawn(move || {
                let t0 = Instant::now();
                let file = std::fs::File::open(&p).unwrap();
                let mut archive = zip::ZipArchive::new(BufReader::with_capacity(256 * 1024, file)).unwrap();
                for i in 0..archive.len() {
                    let Ok(entry) = archive.by_index_raw(i) else { continue };
                    if !entry.is_file() { continue; }
                    let name = entry.name().to_string();
                    if name.contains("__MACOSX/") { continue; }
                    let Some(dot) = name.rfind('.') else { continue };
                    let ext = name[dot + 1..].to_ascii_lowercase();
                    if IMAGE_EXTS.contains(&ext.as_str()) {
                        drop(entry);
                        let mut e = archive.by_index(i).unwrap();
                        let mut bytes = Vec::with_capacity(e.size() as usize);
                        e.read_to_end(&mut bytes).unwrap();
                        println!("  [t{idx}] {:.1}ms  {}", t0.elapsed().as_secs_f64() * 1000.0,
                            p.file_name().and_then(|n| n.to_str()).unwrap_or("?"));
                        return;
                    }
                }
                println!("  [t{idx}] no image  {:.1}ms", t0.elapsed().as_secs_f64() * 1000.0);
            })
        }).collect();
        for h in handles {
            h.join().unwrap();
        }
        println!("  Total parallel time: {:.1}ms", t.elapsed().as_secs_f64() * 1000.0);
    }
}
