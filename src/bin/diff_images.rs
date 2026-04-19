//! 2 つの画像のピクセル差分を計算し、統計を表示する + 差分可視化 PNG を保存する。
//!
//! 用途: タイルサイズ別アップスケール結果など、視覚的に同じに見える出力が
//! 実際にどの程度ピクセル差分を持つかを定量化する。
//!
//! ```
//! cargo run --release --bin diff_images -- a.png b.png --out diff.png
//! cargo run --release --bin diff_images -- a.png b.png --out diff.png --amp 20
//! ```

use std::path::PathBuf;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.len() < 2 || args[0] == "--help" || args[0] == "-h" {
        eprintln!("usage: diff_images <a.png> <b.png> [--out diff.png] [--amp N]");
        eprintln!();
        eprintln!("  --out <PATH>  差分可視化 PNG の出力先");
        eprintln!("  --amp <N>     可視化時に差分値を N 倍する (既定: 10)");
        std::process::exit(2);
    }
    let a_path = PathBuf::from(&args[0]);
    let b_path = PathBuf::from(&args[1]);

    let mut out: Option<PathBuf> = None;
    let mut amp: u32 = 10;

    let mut i = 2;
    while i < args.len() {
        match args[i].as_str() {
            "--out" => {
                i += 1;
                out = Some(PathBuf::from(&args[i]));
            }
            "--amp" => {
                i += 1;
                amp = args[i].parse().expect("--amp expects integer");
            }
            other => panic!("unknown arg: {other}"),
        }
        i += 1;
    }

    let a = image::open(&a_path).expect("load A");
    let b = image::open(&b_path).expect("load B");

    let a_rgba = a.to_rgba8();
    let b_rgba = b.to_rgba8();

    if a_rgba.dimensions() != b_rgba.dimensions() {
        panic!(
            "dimensions differ: {:?} vs {:?}",
            a_rgba.dimensions(),
            b_rgba.dimensions()
        );
    }

    let (w, h) = a_rgba.dimensions();
    let npix = (w * h) as usize;

    let mut diffs_r: Vec<u8> = Vec::with_capacity(npix);
    let mut diffs_g: Vec<u8> = Vec::with_capacity(npix);
    let mut diffs_b: Vec<u8> = Vec::with_capacity(npix);
    let mut max_per_pixel: Vec<u8> = Vec::with_capacity(npix);

    for (pa, pb) in a_rgba.pixels().zip(b_rgba.pixels()) {
        let dr = (pa.0[0] as i32 - pb.0[0] as i32).unsigned_abs() as u8;
        let dg = (pa.0[1] as i32 - pb.0[1] as i32).unsigned_abs() as u8;
        let db_ = (pa.0[2] as i32 - pb.0[2] as i32).unsigned_abs() as u8;
        diffs_r.push(dr);
        diffs_g.push(dg);
        diffs_b.push(db_);
        max_per_pixel.push(dr.max(dg).max(db_));
    }

    println!("A: {} ({}x{})", a_path.display(), w, h);
    println!("B: {} ({}x{})", b_path.display(), w, h);
    println!("pixels: {}", npix);
    println!();

    let (r_mean, r_max, r_p50, r_p95, r_p99, r_p999) = stats(&diffs_r);
    let (g_mean, g_max, g_p50, g_p95, g_p99, g_p999) = stats(&diffs_g);
    let (b_mean, b_max, b_p50, b_p95, b_p99, b_p999) = stats(&diffs_b);
    let (mpp_mean, mpp_max, mpp_p50, mpp_p95, mpp_p99, mpp_p999) = stats(&max_per_pixel);

    println!("Per-channel absolute difference (|A - B|, 0..255):");
    println!(
        "  R: mean {:5.2}  max {:3}  p50 {:3}  p95 {:3}  p99 {:3}  p99.9 {:3}",
        r_mean, r_max, r_p50, r_p95, r_p99, r_p999
    );
    println!(
        "  G: mean {:5.2}  max {:3}  p50 {:3}  p95 {:3}  p99 {:3}  p99.9 {:3}",
        g_mean, g_max, g_p50, g_p95, g_p99, g_p999
    );
    println!(
        "  B: mean {:5.2}  max {:3}  p50 {:3}  p95 {:3}  p99 {:3}  p99.9 {:3}",
        b_mean, b_max, b_p50, b_p95, b_p99, b_p999
    );
    println!();
    println!("Max-of-channel per pixel (L∞ over R/G/B):");
    println!(
        "     mean {:5.2}  max {:3}  p50 {:3}  p95 {:3}  p99 {:3}  p99.9 {:3}",
        mpp_mean, mpp_max, mpp_p50, mpp_p95, mpp_p99, mpp_p999
    );

    let thresholds: [u8; 8] = [0, 1, 2, 5, 10, 20, 50, 100];
    println!();
    println!("Pixels with max-channel diff > threshold:");
    for t in thresholds {
        let c = max_per_pixel.iter().filter(|&&x| x > t).count();
        let pct = 100.0 * c as f64 / npix as f64;
        println!("  > {:3}: {:10} ({:6.3}%)", t, c, pct);
    }

    if let Some(out_path) = out {
        let mut diff_img = image::RgbaImage::new(w, h);
        for (i, p) in diff_img.pixels_mut().enumerate() {
            let dr = (diffs_r[i] as u32 * amp).min(255) as u8;
            let dg = (diffs_g[i] as u32 * amp).min(255) as u8;
            let db = (diffs_b[i] as u32 * amp).min(255) as u8;
            *p = image::Rgba([dr, dg, db, 255]);
        }
        diff_img.save(&out_path).expect("save diff");
        println!();
        println!(
            "diff visualization saved to {} (amplified {}x)",
            out_path.display(),
            amp
        );
    }
}

/// 差分値の統計: (mean, max, p50, p95, p99, p99.9)
fn stats(v: &[u8]) -> (f64, u8, u8, u8, u8, u8) {
    let mean = v.iter().map(|&x| x as f64).sum::<f64>() / v.len() as f64;
    let max = *v.iter().max().unwrap_or(&0);
    let mut sorted: Vec<u8> = v.iter().copied().collect();
    sorted.sort_unstable();
    let n = sorted.len();
    let p50 = sorted[n / 2];
    let p95 = sorted[(n * 95) / 100];
    let p99 = sorted[(n * 99) / 100];
    let p999 = sorted[((n * 999) / 1000).min(n - 1)];
    (mean, max, p50, p95, p99, p999)
}
