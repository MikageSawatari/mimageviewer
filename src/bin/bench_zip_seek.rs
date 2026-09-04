//! 書庫ページ送りの内訳計測 (docs/next-release-backlog.md §1.174 改善方針 5)。
//!
//! ZIP open / 目次解析 / エントリ読み出し / 画像デコードを分けて測る。
//! 現行実装 (ページごとに File::open + ZipArchive::new) と、ハンドル再利用の差も出す。
//!
//! 使い方:
//!   cargo run --release --features dev-tools --bin bench_zip_seek -- <zip> [--pages N]

use std::io::Read;
use std::path::Path;
use std::time::Instant;

fn ms(d: std::time::Duration) -> f64 {
    d.as_secs_f64() * 1000.0
}

fn pct(mut v: Vec<f64>, p: f64) -> f64 {
    if v.is_empty() {
        return 0.0;
    }
    v.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let i = ((v.len() - 1) as f64 * p).round() as usize;
    v[i]
}

fn stat(label: &str, v: &[f64]) {
    let mut s: Vec<f64> = v.to_vec();
    s.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let sum: f64 = s.iter().sum();
    println!(
        "  {label:<34} n={:<5} mean={:>8.2}ms  p50={:>8.2}ms  p95={:>8.2}ms  max={:>8.2}ms",
        s.len(),
        sum / s.len().max(1) as f64,
        pct(s.clone(), 0.5),
        pct(s.clone(), 0.95),
        s.last().copied().unwrap_or(0.0),
    );
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let mut zip_path: Option<String> = None;
    let mut pages: usize = 30;
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--pages" => {
                i += 1;
                pages = args[i].parse().unwrap();
            }
            other => zip_path = Some(other.to_string()),
        }
        i += 1;
    }
    let zip_path = zip_path.expect("usage: bench_zip_seek <zip> [--pages N]");
    let path = Path::new(&zip_path);
    let size = std::fs::metadata(path).map(|m| m.len()).unwrap_or(0);
    println!(
        "=== {} ({:.2} GiB) ===",
        path.file_name().and_then(|n| n.to_str()).unwrap_or("?"),
        size as f64 / (1024.0 * 1024.0 * 1024.0)
    );

    // --- 1. 目次解析の単体コスト (ページごとに毎回払っている分) ---
    let mut t_open = Vec::new();
    let mut t_dir = Vec::new();
    let mut entry_names: Vec<String> = Vec::new();
    let mut entry_count = 0usize;
    for round in 0..10 {
        let t = Instant::now();
        let file = std::fs::File::open(path).unwrap();
        let o = t.elapsed();
        let mut ar = zip::ZipArchive::new(std::io::BufReader::new(file)).unwrap();
        let d = t.elapsed();
        t_open.push(ms(o));
        t_dir.push(ms(d - o));
        if round == 0 {
            entry_count = ar.len();
            for i in 0..ar.len() {
                let e = ar.by_index_raw(i).unwrap();
                if e.is_file() {
                    entry_names.push(e.name().to_string());
                }
            }
        }
    }
    println!("\n[1] 1 ページごとに払っている前処理  entries={entry_count}");
    stat("File::open", &t_open);
    stat("ZipArchive::new (中央目次解析)", &t_dir);

    // 順送りを模した対象ページ (中央付近から連番)
    let start = entry_names.len() / 2;
    let targets: Vec<&String> = entry_names
        .iter()
        .skip(start)
        .take(pages.min(entry_names.len().saturating_sub(start)))
        .collect();

    // --- 2. 本番経路: 初回だけ解析し、以後は template clone + 読み出し ---
    let mut cur_total = Vec::new();
    let mut bytes_len = Vec::new();
    mimageviewer::zip_loader::clear_nested_cache();
    for name in &targets {
        let t = Instant::now();
        let buf = mimageviewer::zip_loader::read_entry_bytes(path, name).unwrap();
        cur_total.push(ms(t.elapsed()));
        bytes_len.push(buf.len() as f64);
    }
    println!("\n[2] 本番経路 (解析済み目次を clone)");
    stat("合計 (metadata 照合込み)", &cur_total);
    stat(
        "エントリのバイト数 (KiB 表示)",
        &bytes_len.iter().map(|b| b / 1024.0).collect::<Vec<_>>(),
    );

    // --- 3. ハンドル再利用: 目次解析は 1 回だけ ---
    let mut reuse_read = Vec::new();
    {
        let file = std::fs::File::open(path).unwrap();
        let mut ar = zip::ZipArchive::new(std::io::BufReader::new(file)).unwrap();
        for name in &targets {
            let t = Instant::now();
            let idx = ar.index_for_name(name).unwrap();
            let mut e = ar.by_index(idx).unwrap();
            let mut buf = Vec::with_capacity(e.size() as usize);
            e.read_to_end(&mut buf).unwrap();
            reuse_read.push(ms(t.elapsed()));
        }
    }
    println!("\n[3] ハンドル再利用 (目次解析 1 回)");
    stat("読み出しのみ", &reuse_read);

    // --- 4. 画像デコード ---
    let mut dec = Vec::new();
    {
        let file = std::fs::File::open(path).unwrap();
        let mut ar = zip::ZipArchive::new(std::io::BufReader::new(file)).unwrap();
        for name in targets.iter().take(10) {
            let idx = ar.index_for_name(name).unwrap();
            let mut e = ar.by_index(idx).unwrap();
            let mut buf = Vec::with_capacity(e.size() as usize);
            e.read_to_end(&mut buf).unwrap();
            let t = Instant::now();
            let img = image::load_from_memory(&buf).unwrap();
            dec.push(ms(t.elapsed()));
            if dec.len() == 1 {
                println!("\n[4] 画像デコード  ({}x{})", img.width(), img.height());
            }
        }
    }
    stat("image::load_from_memory", &dec);

    // --- 5. 並列競合: cold cache から本番経路を N 本同時に走らせる ---
    println!(
        "
[5] 並列競合 (cold cache の本番経路を N 本同時)"
    );
    for workers in [1usize, 4, 6, 8, 16, 32, 51] {
        let names: Vec<String> = entry_names
            .iter()
            .skip(start + 100)
            .take(workers)
            .cloned()
            .collect();
        if names.len() < workers {
            break;
        }
        let path_buf = path.to_path_buf();
        mimageviewer::zip_loader::clear_nested_cache();
        let parse_count_before = mimageviewer::zip_loader::archive_directory_parse_count();
        let barrier = std::sync::Arc::new(std::sync::Barrier::new(workers));
        let t = Instant::now();
        let handles: Vec<_> = names
            .into_iter()
            .map(|name| {
                let p = path_buf.clone();
                let barrier = std::sync::Arc::clone(&barrier);
                std::thread::spawn(move || {
                    barrier.wait();
                    let t = Instant::now();
                    let buf = mimageviewer::zip_loader::read_entry_bytes(&p, &name).unwrap();
                    let t_read = t.elapsed();
                    let _ = image::load_from_memory(&buf).unwrap();
                    (ms(t_read), ms(t.elapsed() - t_read), ms(t.elapsed()))
                })
            })
            .collect();
        let mut reads = Vec::new();
        let mut decs = Vec::new();
        let mut alls = Vec::new();
        for h in handles {
            let (r, c, a) = h.join().unwrap();
            reads.push(r);
            decs.push(c);
            alls.push(a);
        }
        let wall = ms(t.elapsed());
        let parse_count =
            mimageviewer::zip_loader::archive_directory_parse_count() - parse_count_before;
        println!(
            "  workers={workers:<3} wall={wall:>8.1}ms  目次解析={parse_count}回  1本あたり: 読み p50={:>6.1} / デコード p50={:>6.1} / 合計 p50={:>7.1} max={:>7.1}",
            pct(reads.clone(), 0.5),
            pct(decs.clone(), 0.5),
            pct(alls.clone(), 0.5),
            alls.iter().cloned().fold(0.0, f64::max),
        );
    }
}
