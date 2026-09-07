//! シークバーのサムネイル抽出を、**デコーダーを開いたまま**ハードウェアとソフトウェアで比較する。
//!
//! `ffmpeg -ss` での計測はシークのたびにデコーダーを作り直すため、ハードウェア復号が
//! D3D11 デバイス生成を毎回払って不利になる。mIV はセッション中デコーダーを保持するので、
//! その差を含めない数字がいる (backlog §1.190、利用者指摘 2026-09-05)。
//!
//! 実際の [`crate::video::seek_strip_thumbs`] の `decode_run` をそのまま呼ぶので、
//! シーク・キーフレーム判定・swscale・向き補正まで本番と同じ経路を通る。
//!
//! ```text
//! cargo run --release --features dev-tools --bin bench_seek_decode -- <file> [cells]
//! ```
//!
//! 既定は 12 セル (全体表示と同じ数) を尺全体へ等間隔で置く。1 本目の結果は
//! デコーダー起動直後なので、**2 本目以降と分けて**表示する。

use std::path::PathBuf;

use mimageviewer::video::seek_strip_bench::{SeekDecodeSample, bench_persistent_decoder};

fn duration_secs(path: &std::path::Path) -> Option<f64> {
    ffmpeg_the_third::init().ok()?;
    let input = ffmpeg_the_third::format::input(path).ok()?;
    let raw = input.duration();
    (raw > 0).then(|| raw as f64 / f64::from(ffmpeg_the_third::ffi::AV_TIME_BASE))
}

fn summarize(label: &str, decode_path: &str, samples: &[SeekDecodeSample]) {
    if samples.is_empty() {
        println!("{label:<22} (no samples)");
        return;
    }
    let first = &samples[0];
    let rest = &samples[1..];
    let mut totals: Vec<f64> = rest.iter().map(|s| s.total_ms).collect();
    totals.sort_by(f64::total_cmp);
    let median = if totals.is_empty() {
        first.total_ms
    } else {
        totals[totals.len() / 2]
    };
    let sum: f64 = samples.iter().map(|s| s.total_ms).sum();
    let seek: f64 = samples.iter().map(|s| s.seek_ms).sum();
    let frames: usize = samples.iter().map(|s| s.decoded_frames).sum();
    let missed = samples.iter().filter(|s| !s.published).count();
    println!(
        "{label:<22} path={decode_path:<10} first={:>7.1}ms  median={median:>7.1}ms  \
         total={sum:>8.1}ms  seek={seek:>7.1}ms  frames={frames:>6}  missed={missed}",
        first.total_ms
    );
}

fn main() {
    let mut args = std::env::args_os().skip(1);
    let Some(path) = args.next().map(PathBuf::from) else {
        eprintln!("usage: bench_seek_decode <video> [cell-count]");
        std::process::exit(2);
    };
    let cells: usize = args
        .next()
        .and_then(|value| value.to_string_lossy().parse().ok())
        .unwrap_or(12);

    let Some(duration) = duration_secs(&path) else {
        eprintln!("could not read a usable duration from {}", path.display());
        std::process::exit(1);
    };
    // 全体表示と同じ置き方: 尺 / セル数 の等間隔。
    let interval = duration / cells as f64;
    let targets: Vec<f64> = (0..cells).map(|i| i as f64 * interval).collect();

    println!(
        "{} : {:.1}s / {} cells / {:.1}s apart",
        path.display(),
        duration,
        cells,
        interval
    );
    println!();

    for keyframes_only in [true, false] {
        for hw in [true, false] {
            let label = format!(
                "{} {}",
                if hw { "hw" } else { "sw" },
                if keyframes_only { "keyframes" } else { "full" }
            );
            match bench_persistent_decoder(&path, hw, keyframes_only, interval, &targets) {
                Ok((decode_path, samples)) => summarize(&label, &decode_path, &samples),
                Err(error) => println!("{label:<22} failed: {error}"),
            }
        }
    }
}
