//! サムネイル生成パフォーマンス計測ツール
//!
//! リリースビルド専用の計測ツール。設定ファイル (`%APPDATA%\mimageviewer\settings.json`)
//! からお気に入りフォルダを読み込み、再帰的にフォルダをスキャンして、
//! 各フォルダからランダムに数枚の画像/動画を選び、サムネイル生成に関する時間を計測する。
//!
//! 使い方:
//!     cargo run --release --bin bench_thumbs -- --sample   # 5フォルダだけで動作確認
//!     cargo run --release --bin bench_thumbs               # 全フォルダ本計測
//!
//! 結果は `bench_thumbs.tsv` に TSV で出力される。

#[allow(dead_code)]
#[path = "../settings.rs"]
mod settings;

#[allow(dead_code)]
#[path = "../video_thumb.rs"]
mod video_thumb;

use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::Instant;

// -----------------------------------------------------------------------
// 定数
// -----------------------------------------------------------------------

const IMAGE_EXTS: &[&str] = &["jpg", "jpeg", "png", "webp", "bmp", "gif"];
const VIDEO_EXTS: &[&str] = &["mpg", "mpeg", "mp4", "avi", "mov", "mkv", "wmv"];

const DEFAULT_FILES_PER_FOLDER: usize = 5;
const DEFAULT_THUMB_PX: u32 = 512;
const DEFAULT_QUALITY: f32 = 75.0;
const SAMPLE_MODE_FOLDER_LIMIT: usize = 5;
const OUTPUT_FILENAME: &str = "bench_thumbs.tsv";

// -----------------------------------------------------------------------
// 依存を足したくないので自前の xorshift64 PRNG
// -----------------------------------------------------------------------

struct Rng(u64);

impl Rng {
    fn new() -> Self {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos() as u64)
            .unwrap_or(0xdead_beef_cafe_babe);
        // 0 を避ける
        Self(nanos.wrapping_mul(0x2545_f491_4f6c_dd1d).max(1))
    }

    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }

    /// 0..n の乱数 (n >= 1)
    fn range(&mut self, n: usize) -> usize {
        (self.next() as usize) % n.max(1)
    }
}

// -----------------------------------------------------------------------
// main
// -----------------------------------------------------------------------

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let sample_mode = args.iter().any(|a| a == "--sample");

    println!("=== bench_thumbs ===");
    println!("sample_mode:    {}", sample_mode);
    println!("thumb_px:       {}", DEFAULT_THUMB_PX);
    println!("webp_quality:   {}", DEFAULT_QUALITY);
    println!("files / folder: {}", DEFAULT_FILES_PER_FOLDER);
    println!();

    // ----- 設定ファイル読み込み -----
    let settings_obj = settings::Settings::load();
    println!("お気に入り数: {}", settings_obj.favorites.len());
    if settings_obj.favorites.is_empty() {
        eprintln!("エラー: お気に入りが1件も登録されていません。");
        eprintln!("先に mimageviewer 本体でお気に入りを登録してください。");
        std::process::exit(1);
    }
    for (i, fav) in settings_obj.favorites.iter().enumerate() {
        println!("  [{}] {}", i + 1, fav.display());
    }
    println!();

    // ----- フォルダツリー収集 -----
    println!("フォルダツリーをスキャン中...");
    let scan_t = Instant::now();
    let mut all_folders: Vec<PathBuf> = Vec::new();
    for fav in &settings_obj.favorites {
        walk_dirs(fav, &mut all_folders);
    }
    println!(
        "スキャン完了: {} フォルダ ({:.2}秒)",
        all_folders.len(),
        scan_t.elapsed().as_secs_f64()
    );
    println!();

    if sample_mode && all_folders.len() > SAMPLE_MODE_FOLDER_LIMIT {
        // サンプルモードでも分布を見たいので単純 truncate ではなく均等サンプリング
        let step = all_folders.len() / SAMPLE_MODE_FOLDER_LIMIT;
        let sampled: Vec<PathBuf> = (0..SAMPLE_MODE_FOLDER_LIMIT)
            .map(|i| all_folders[i * step].clone())
            .collect();
        all_folders = sampled;
        println!(
            "サンプルモード: 均等間隔で {} フォルダを抽出",
            SAMPLE_MODE_FOLDER_LIMIT
        );
        println!();
    }

    // ----- 出力 TSV オープン -----
    let out_path = Path::new(OUTPUT_FILENAME);
    let mut out = match std::fs::File::create(out_path) {
        Ok(f) => f,
        Err(e) => {
            eprintln!("出力ファイル作成失敗 {}: {}", out_path.display(), e);
            std::process::exit(1);
        }
    };
    writeln!(
        out,
        "folder\tfile\text\tkind\tsize_bytes\twidth\theight\tdims_ms\topen_ms\tresize_ms\tencode_ms\twebp_bytes\tshell_ms\tstatus"
    )
    .ok();
    out.flush().ok();

    // ----- 計測ループ -----
    let mut rng = Rng::new();
    let mut total_measured: usize = 0;
    let mut total_skipped_empty: usize = 0;
    let total_t = Instant::now();

    for (i, folder) in all_folders.iter().enumerate() {
        // フォルダ内のメディアファイルを列挙
        let mut files: Vec<(PathBuf, String, &'static str)> = Vec::new();
        if let Ok(entries) = std::fs::read_dir(folder) {
            for entry in entries.flatten() {
                let p = entry.path();
                if !p.is_file() {
                    continue;
                }
                let Some(ext) = p.extension().and_then(|e| e.to_str()) else {
                    continue;
                };
                let ext_lower = ext.to_lowercase();
                if IMAGE_EXTS.contains(&ext_lower.as_str()) {
                    files.push((p, ext_lower, "image"));
                } else if VIDEO_EXTS.contains(&ext_lower.as_str()) {
                    files.push((p, ext_lower, "video"));
                }
            }
        }

        if files.is_empty() {
            total_skipped_empty += 1;
            if sample_mode {
                println!(
                    "[{:>5}/{}] (空) {}",
                    i + 1,
                    all_folders.len(),
                    folder.display()
                );
            }
            continue;
        }

        // 部分 Fisher-Yates: 先頭 min(take, n) 要素だけシャッフル
        let n = files.len();
        let take = DEFAULT_FILES_PER_FOLDER.min(n);
        for j in 0..take {
            let k = j + rng.range(n - j);
            files.swap(j, k);
        }

        println!(
            "[{:>5}/{}] {} files (候補 {}) {}",
            i + 1,
            all_folders.len(),
            take,
            n,
            folder.display()
        );

        for (path, ext_lower, kind) in files.iter().take(take) {
            measure_file(path.as_path(), folder, ext_lower.as_str(), *kind, &mut out);
            total_measured += 1;
        }

        // 進捗をこまめに flush しておくと中断時もデータが残る
        out.flush().ok();
    }

    let total_sec = total_t.elapsed().as_secs_f64();
    println!();
    println!("=== 計測完了 ===");
    println!("計測ファイル数:   {}", total_measured);
    println!("空フォルダスキップ: {}", total_skipped_empty);
    println!("総時間:           {:.2}秒", total_sec);
    println!("出力ファイル:     {}", out_path.display());
}

// -----------------------------------------------------------------------
// フォルダ再帰走査
// -----------------------------------------------------------------------

fn walk_dirs(path: &Path, out: &mut Vec<PathBuf>) {
    if !path.is_dir() {
        return;
    }
    out.push(path.to_path_buf());
    if let Ok(entries) = std::fs::read_dir(path) {
        for entry in entries.flatten() {
            let p = entry.path();
            if p.is_dir() {
                walk_dirs(&p, out);
            }
        }
    }
}

// -----------------------------------------------------------------------
// 1 ファイルの計測
// -----------------------------------------------------------------------

fn measure_file(
    path: &Path,
    folder: &Path,
    ext: &str,
    kind: &'static str,
    out: &mut std::fs::File,
) {
    let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("?");
    let size = std::fs::metadata(path).map(|m| m.len()).unwrap_or(0);

    // TSV に影響しないようタブを空白に置換
    let folder_s = folder.display().to_string().replace('\t', " ");
    let name_s = name.replace('\t', " ");

    if kind == "video" {
        // Windows Shell API サムネイル取得を計測
        let t = Instant::now();
        let result = video_thumb::get_video_thumbnail(path, DEFAULT_THUMB_PX as i32);
        let shell_ms = t.elapsed().as_secs_f64() * 1000.0;

        let (w, h) = match &result {
            Some(ci) => (ci.size[0] as u32, ci.size[1] as u32),
            None => (0, 0),
        };
        let status = if result.is_some() { "OK" } else { "FAIL" };

        writeln!(
            out,
            "{folder_s}\t{name_s}\t{ext}\t{kind}\t{size}\t{w}\t{h}\t\t\t\t\t\t{shell_ms:.2}\t{status}"
        )
        .ok();
        return;
    }

    // ----- 画像 -----
    // 1. image_dimensions (ヘッダのみ)
    let t1 = Instant::now();
    let dims = image::image_dimensions(path).ok();
    let dims_ms = t1.elapsed().as_secs_f64() * 1000.0;
    let (hw, hh) = dims.unwrap_or((0, 0));

    // 2. image::open (フルデコード)
    //    拡張子が間違っているファイルにも対応するため、失敗時は with_guessed_format でリトライ
    //    (本体 app.rs の load_one_cached と同じ二段構え)
    let t2 = Instant::now();
    let img_result = image::open(path).or_else(|_| {
        use std::io::BufReader;
        let f = std::fs::File::open(path)?;
        image::ImageReader::new(BufReader::new(f))
            .with_guessed_format()
            .map_err(image::ImageError::IoError)?
            .decode()
    });
    let img = match img_result {
        Ok(i) => i,
        Err(e) => {
            let err_msg = e.to_string().replace('\t', " ").replace('\n', " ");
            writeln!(
                out,
                "{folder_s}\t{name_s}\t{ext}\t{kind}\t{size}\t{hw}\t{hh}\t{dims_ms:.2}\t\t\t\t\t\tFAIL:{err_msg}"
            )
            .ok();
            return;
        }
    };
    let open_ms = t2.elapsed().as_secs_f64() * 1000.0;
    let (w, h) = (img.width(), img.height());

    // 3. resize to DEFAULT_THUMB_PX (Lanczos3)
    let t3 = Instant::now();
    let resized = img.resize(
        DEFAULT_THUMB_PX,
        DEFAULT_THUMB_PX,
        image::imageops::FilterType::Lanczos3,
    );
    let resize_ms = t3.elapsed().as_secs_f64() * 1000.0;

    // 4. WebP encode
    let t4 = Instant::now();
    let rgb = resized.to_rgb8();
    let encoder = webp::Encoder::from_rgb(rgb.as_raw(), rgb.width(), rgb.height());
    let webp_data = encoder.encode(DEFAULT_QUALITY);
    let encode_ms = t4.elapsed().as_secs_f64() * 1000.0;
    let webp_bytes = webp_data.len();

    writeln!(
        out,
        "{folder_s}\t{name_s}\t{ext}\t{kind}\t{size}\t{w}\t{h}\t{dims_ms:.2}\t{open_ms:.2}\t{resize_ms:.2}\t{encode_ms:.2}\t{webp_bytes}\t\tOK"
    )
    .ok();
}
