//! サムネイルロードのスクロールシミュレーション ベンチマーク。
//!
//! 特定フォルダを対象に、実際のワーカースレッド構成でスクロールをシミュレートし、
//! サムネイル表示速度を計測する。GUI なしで動作する。
//!
//! ```
//! cargo run --release --bin bench_scroll -- "E:\share\18\VNCG.org" [options]
//!
//! Options:
//!   --cols N         列数 (default: 5)
//!   --rows N         画面行数 (default: 4)
//!   --scroll-to N    スクロール先の行番号 (default: total_rows/2)
//!   --threads N      ワーカー数 (default: auto)
//!   --no-cache       キャッシュを無効化 (常にフルデコード)
//!   --delete-cache   計測前にキャッシュ DB を削除
//! ```

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicUsize, Ordering};
use std::sync::{mpsc, Arc, Mutex, RwLock};
use std::time::Instant;

use mimageviewer::catalog::{self, CatalogDb};
use mimageviewer::folder_tree::{is_apple_double, SUPPORTED_EXTENSIONS, SUPPORTED_VIDEO_EXTENSIONS};
use mimageviewer::grid_item::GridItem;
use mimageviewer::settings::{CachePolicy, SortOrder};
use mimageviewer::stats::ThumbStats;
use mimageviewer::thumb_loader::{
    process_load_request, CacheDecision, LoadRequest, ThumbMsg,
    CACHE_KEY_ZIP, CACHE_KEY_PDF, CACHE_KEY_FOLDER,
};
use mimageviewer::ui_helpers::{mtime_secs, natural_sort_key};

// ───────────────────────────────────────────────────────────────────
// コマンドライン引数
// ───────────────────────────────────────────────────────────────────

struct Args {
    folder: PathBuf,
    cols: usize,
    rows: usize,
    scroll_to: Option<usize>, // None = auto (total/2)
    threads: usize,
    no_cache: bool,
    delete_cache: bool,
}

fn parse_args() -> Args {
    let raw: Vec<String> = std::env::args().skip(1).collect();
    if raw.is_empty() {
        eprintln!("Usage: bench_scroll <folder> [--cols N] [--rows N] [--scroll-to N] [--threads N] [--no-cache] [--delete-cache]");
        std::process::exit(1);
    }
    let mut args = Args {
        folder: PathBuf::new(),
        cols: 5,
        rows: 4,
        scroll_to: None,
        threads: std::thread::available_parallelism()
            .map(|p| p.get().saturating_sub(2).max(2))
            .unwrap_or(4),
        no_cache: false,
        delete_cache: false,
    };
    let mut i = 0;
    while i < raw.len() {
        match raw[i].as_str() {
            "--cols" => {
                i += 1;
                args.cols = raw[i].parse().unwrap_or(5);
            }
            "--rows" => {
                i += 1;
                args.rows = raw[i].parse().unwrap_or(4);
            }
            "--scroll-to" => {
                i += 1;
                args.scroll_to = raw[i].parse().ok();
            }
            "--threads" => {
                i += 1;
                args.threads = raw[i].parse().unwrap_or(4);
            }
            "--no-cache" => args.no_cache = true,
            "--delete-cache" => args.delete_cache = true,
            other => {
                if args.folder.as_os_str().is_empty() {
                    args.folder = PathBuf::from(other);
                }
            }
        }
        i += 1;
    }
    if args.folder.as_os_str().is_empty() {
        eprintln!("Error: folder path is required");
        std::process::exit(1);
    }
    args
}

// ───────────────────────────────────────────────────────────────────
// フォルダ走査 (app.rs の load_folder と同等)
// ───────────────────────────────────────────────────────────────────

struct FolderContents {
    items: Vec<GridItem>,
    image_metas: Vec<Option<(i64, i64)>>,
    existing_keys: HashSet<String>,
    counts: ItemCounts,
}

#[derive(Default)]
struct ItemCounts {
    folders: usize,
    images: usize,
    zips: usize,
    pdfs: usize,
    videos: usize,
}

fn scan_folder(path: &Path) -> FolderContents {
    // ZIP / PDF ファイルを仮想フォルダとして開く
    if path.is_file() {
        let ext = path
            .extension()
            .and_then(|e| e.to_str())
            .map(|e| e.to_ascii_lowercase())
            .unwrap_or_default();
        if ext == "zip" {
            return scan_zip_as_folder(path);
        }
        if ext == "pdf" {
            return scan_pdf_as_folder(path);
        }
    }

    let mut folders: Vec<(GridItem, Option<(i64, i64)>)> = Vec::new();
    let mut all_media: Vec<(PathBuf, bool, i64, i64)> = Vec::new();

    if let Ok(entries) = std::fs::read_dir(path) {
        for entry in entries.flatten() {
            let p = entry.path();
            if p.is_dir() {
                let meta = entry.metadata().ok();
                let mtime = meta.as_ref().map_or(0, |m| mtime_secs(m));
                folders.push((GridItem::Folder(p), Some((mtime, 0))));
            } else if is_apple_double(&p) {
                // skip
            } else if let Some(ext) = p.extension().and_then(|e| e.to_str()) {
                let ext_lower = ext.to_ascii_lowercase();
                let meta = entry.metadata().ok();
                let mtime = meta.as_ref().map_or(0, |m| mtime_secs(m));
                let file_size = meta.map_or(0, |m| m.len() as i64);
                if SUPPORTED_EXTENSIONS.contains(&ext_lower.as_str()) {
                    all_media.push((p, false, mtime, file_size));
                } else if SUPPORTED_VIDEO_EXTENSIONS.contains(&ext_lower.as_str()) {
                    all_media.push((p, true, mtime, file_size));
                } else if ext_lower == "zip" {
                    folders.push((GridItem::ZipFile(p), Some((mtime, file_size))));
                } else if ext_lower == "pdf" {
                    folders.push((GridItem::PdfFile(p), Some((mtime, file_size))));
                }
            }
        }
    }

    folders.sort_by(|(a, _), (b, _)| a.name().to_lowercase().cmp(&b.name().to_lowercase()));
    let sort = SortOrder::Numeric;
    all_media.sort_by(|(a, _, a_mt, _), (b, _, b_mt, _)| {
        let an = a.file_name().and_then(|n| n.to_str()).unwrap_or("");
        let bn = b.file_name().and_then(|n| n.to_str()).unwrap_or("");
        sort.compare(an, *a_mt, bn, *b_mt, natural_sort_key)
    });

    let mut counts = ItemCounts::default();
    let folder_count = folders.len();
    let mut items: Vec<GridItem> = Vec::new();
    let mut image_metas: Vec<Option<(i64, i64)>> = Vec::new();

    for (item, meta) in folders {
        match &item {
            GridItem::Folder(_) => counts.folders += 1,
            GridItem::ZipFile(_) => counts.zips += 1,
            GridItem::PdfFile(_) => counts.pdfs += 1,
            _ => {}
        }
        items.push(item);
        image_metas.push(meta);
    }

    for (p, is_video, mtime, file_size) in &all_media {
        if *is_video {
            items.push(GridItem::Video(p.clone()));
            image_metas.push(None); // 動画はサムネイルロード対象外
            counts.videos += 1;
        } else {
            items.push(GridItem::Image(p.clone()));
            image_metas.push(Some((*mtime, *file_size)));
            counts.images += 1;
        }
    }

    let existing_keys: HashSet<String> = items
        .iter()
        .filter_map(|it| match it {
            GridItem::Image(p) => p.file_name()?.to_str().map(String::from),
            GridItem::ZipFile(p) => {
                let fname = p.file_name()?.to_str()?;
                Some(format!("{CACHE_KEY_ZIP}{fname}"))
            }
            GridItem::PdfFile(p) => {
                let fname = p.file_name()?.to_str()?;
                Some(format!("{CACHE_KEY_PDF}{fname}"))
            }
            GridItem::Folder(p) => {
                let fname = p.file_name()?.to_str()?;
                Some(format!("{CACHE_KEY_FOLDER}{fname}"))
            }
            _ => None,
        })
        .collect();

    FolderContents {
        items,
        image_metas,
        existing_keys,
        counts,
    }
}

/// ZIP ファイルを仮想フォルダとして走査
fn scan_zip_as_folder(zip_path: &Path) -> FolderContents {
    let entries = match mimageviewer::zip_loader::enumerate_image_entries(zip_path) {
        Ok(e) => e,
        Err(_) => {
            return FolderContents {
                items: Vec::new(),
                image_metas: Vec::new(),
                existing_keys: HashSet::new(),
                counts: ItemCounts::default(),
            };
        }
    };

    let mut items: Vec<GridItem> = Vec::new();
    let mut image_metas: Vec<Option<(i64, i64)>> = Vec::new();
    let mut existing_keys: HashSet<String> = HashSet::new();
    let mut counts = ItemCounts::default();

    for e in entries {
        existing_keys.insert(e.entry_name.clone());
        items.push(GridItem::ZipImage {
            zip_path: zip_path.to_path_buf(),
            entry_name: e.entry_name,
        });
        image_metas.push(Some((e.mtime, e.uncompressed_size as i64)));
        counts.images += 1;
    }

    FolderContents { items, image_metas, existing_keys, counts }
}

/// PDF ファイルを仮想フォルダとして走査 (同期版)
fn scan_pdf_as_folder(pdf_path: &Path) -> FolderContents {
    let page_count = match mimageviewer::pdf_loader::enumerate_pages(pdf_path, None) {
        Ok(pages) => pages.len() as u32,
        Err(_) => 0,
    };

    let mtime = std::fs::metadata(pdf_path)
        .ok()
        .map_or(0, |m| mtime_secs(&m));
    let file_size = std::fs::metadata(pdf_path)
        .map(|m| m.len() as i64)
        .unwrap_or(0);

    let mut items: Vec<GridItem> = Vec::new();
    let mut image_metas: Vec<Option<(i64, i64)>> = Vec::new();
    let mut existing_keys: HashSet<String> = HashSet::new();
    let mut counts = ItemCounts { pdfs: page_count as usize, ..Default::default() };

    for page in 0..page_count {
        let key = mimageviewer::grid_item::pdf_page_cache_key(page);
        existing_keys.insert(key);
        items.push(GridItem::PdfPage {
            pdf_path: pdf_path.to_path_buf(),
            page_num: page,
        });
        image_metas.push(Some((mtime, file_size)));
    }

    FolderContents { items, image_metas, existing_keys, counts }
}

// ───────────────────────────────────────────────────────────────────
// LoadRequest 構築 (app.rs の make_load_request と同等)
// ───────────────────────────────────────────────────────────────────

fn make_load_request(
    item: &GridItem,
    idx: usize,
    mtime: i64,
    file_size: i64,
) -> Option<LoadRequest> {
    match item {
        GridItem::Image(p) => Some(LoadRequest {
            idx,
            path: p.clone(),
            mtime,
            file_size,
            skip_cache: false,
            priority: false,
            zip_entry: None,
            pdf_page: None,
            pdf_password: None,
            cache_key_override: None,
            folder_thumb_sort: None, folder_thumb_depth: 0,
        }),
        GridItem::ZipFile(p) => {
            let fname = p.file_name().and_then(|n| n.to_str()).unwrap_or("").to_string();
            Some(LoadRequest {
                idx,
                path: p.clone(),
                mtime,
                file_size,
                skip_cache: false,
                priority: false,
                zip_entry: None,
                pdf_page: None,
                pdf_password: None,
                cache_key_override: Some(format!("{CACHE_KEY_ZIP}{fname}")),
                folder_thumb_sort: None, folder_thumb_depth: 0,
            })
        }
        GridItem::PdfFile(p) => {
            let fname = p.file_name().and_then(|n| n.to_str()).unwrap_or("").to_string();
            Some(LoadRequest {
                idx,
                path: p.clone(),
                mtime,
                file_size,
                skip_cache: false,
                priority: false,
                zip_entry: None,
                pdf_page: Some(0),
                pdf_password: None,
                cache_key_override: Some(format!("{CACHE_KEY_PDF}{fname}")),
                folder_thumb_sort: None, folder_thumb_depth: 0,
            })
        }
        GridItem::Folder(p) => {
            let fname = p.file_name().and_then(|n| n.to_str()).unwrap_or("").to_string();
            Some(LoadRequest {
                idx,
                path: p.clone(),
                mtime,
                file_size,
                skip_cache: false,
                priority: false,
                zip_entry: None,
                pdf_page: None,
                pdf_password: None,
                cache_key_override: Some(format!("{CACHE_KEY_FOLDER}{fname}")),
                folder_thumb_sort: Some(SortOrder::Numeric), folder_thumb_depth: 3,
            })
        }
        GridItem::ZipImage { zip_path, entry_name } => Some(LoadRequest {
            idx,
            path: zip_path.clone(),
            mtime,
            file_size,
            skip_cache: false,
            priority: false,
            zip_entry: Some(entry_name.clone()),
            pdf_page: None,
            pdf_password: None,
            cache_key_override: None,
            folder_thumb_sort: None, folder_thumb_depth: 0,
        }),
        GridItem::PdfPage { pdf_path, page_num } => Some(LoadRequest {
            idx,
            path: pdf_path.clone(),
            mtime,
            file_size,
            skip_cache: false,
            priority: false,
            zip_entry: None,
            pdf_page: Some(*page_num),
            pdf_password: None,
            cache_key_override: None,
            folder_thumb_sort: None, folder_thumb_depth: 0,
        }),
        _ => None,
    }
}

// ───────────────────────────────────────────────────────────────────
// 計測結果
// ───────────────────────────────────────────────────────────────────

struct BenchResult {
    scroll_time_ms: f64,
    first_visible_ms: f64,
    all_visible_ms: f64,
    all_prefetch_ms: f64,
    cache_hits: usize,
    cache_misses: usize,
    total_received: usize,
    zip_resolve_ms: Vec<f64>,
    folder_resolve_ms: Vec<f64>,
}

fn percentile(sorted: &[f64], p: f64) -> f64 {
    if sorted.is_empty() {
        return 0.0;
    }
    let idx = ((sorted.len() as f64 - 1.0) * p).round() as usize;
    sorted[idx.min(sorted.len() - 1)]
}

fn print_result(label: &str, r: &BenchResult) {
    println!("--- {label} ---");
    println!("  Scroll time:           {:>7.0} ms", r.scroll_time_ms);
    println!("  First visible thumb:   {:>7.0} ms  (after scroll stop)", r.first_visible_ms);
    println!("  All visible complete:  {:>7.0} ms  (after scroll stop)", r.all_visible_ms);
    println!("  All prefetch complete: {:>7.0} ms  (after scroll stop)", r.all_prefetch_ms);
    let total = r.cache_hits + r.cache_misses;
    if total > 0 {
        println!(
            "  Cache hit rate: {:.0}% ({}/{})",
            r.cache_hits as f64 / total as f64 * 100.0,
            r.cache_hits,
            total,
        );
    }
    if !r.zip_resolve_ms.is_empty() {
        let mut s = r.zip_resolve_ms.clone();
        s.sort_by(|a, b| a.partial_cmp(b).unwrap());
        println!(
            "  ZIP resolve: p50={:.0}ms p90={:.0}ms max={:.0}ms (n={})",
            percentile(&s, 0.5),
            percentile(&s, 0.9),
            s.last().unwrap_or(&0.0),
            s.len(),
        );
    }
    if !r.folder_resolve_ms.is_empty() {
        let mut s = r.folder_resolve_ms.clone();
        s.sort_by(|a, b| a.partial_cmp(b).unwrap());
        println!(
            "  Folder resolve: p50={:.0}ms p90={:.0}ms max={:.0}ms (n={})",
            percentile(&s, 0.5),
            percentile(&s, 0.9),
            s.last().unwrap_or(&0.0),
            s.len(),
        );
    }
    println!("  Total received: {}", r.total_received);
    println!();
}

// ───────────────────────────────────────────────────────────────────
// スクロールシミュレーション
// ───────────────────────────────────────────────────────────────────

fn run_bench(
    args: &Args,
    contents: &FolderContents,
    cache_map: Arc<RwLock<HashMap<String, catalog::CacheEntry>>>,
    catalog_arc: Option<Arc<CatalogDb>>,
    label: &str,
) -> BenchResult {
    let cols = args.cols;
    let rows = args.rows;
    let items_per_page = cols * rows;
    let total = contents.items.len();
    let total_rows = total.div_ceil(cols);
    let target_row = args.scroll_to.unwrap_or(total_rows / 2).min(total_rows.saturating_sub(1));

    let prev_pages: usize = 2;
    let next_pages: usize = 4;

    // 共有状態
    let cancel = Arc::new(AtomicBool::new(false));
    let scroll_hint = Arc::new(AtomicUsize::new(0));
    let display_px_shared = Arc::new(AtomicU32::new(512));
    let keep_start_shared = Arc::new(AtomicUsize::new(0));
    let keep_end_shared = Arc::new(AtomicUsize::new(0));
    let cache_gen_done = Arc::new(AtomicUsize::new(0));
    let stats = Arc::new(Mutex::new(ThumbStats::new()));
    let reload_queue: Arc<Mutex<Vec<LoadRequest>>> = Arc::new(Mutex::new(Vec::new()));
    let heavy_io_queue: Arc<Mutex<Vec<LoadRequest>>> = Arc::new(Mutex::new(Vec::new()));

    let thumb_px = 512u32;
    let thumb_quality = 75u8;
    let display_px = 512u32;
    let cache_decision = CacheDecision {
        policy: CachePolicy::Auto,
        threshold_ms: 25,
        size_threshold: 2 * 1024 * 1024,
        webp_always: false,
        pdf_always: false,
        zip_always: false,
    };

    // 結果チャネル
    let (tx, rx) = mpsc::channel::<ThumbMsg>();

    // ワーカー起動 (app.rs の spawn_thumbnail_workers と同じ 2 種類)
    let io_threads = if args.threads <= 4 { 1 } else { 2 };
    let regular_threads = args.threads.saturating_sub(io_threads).max(1);

    let spawn_worker = |queue: Arc<Mutex<Vec<LoadRequest>>>| {
        let tx_w = tx.clone();
        let cancel_w = Arc::clone(&cancel);
        let hint_w = Arc::clone(&scroll_hint);
        let cache_map_w = Arc::clone(&cache_map);
        let catalog_w = catalog_arc.clone();
        let done_w = Arc::clone(&cache_gen_done);
        let display_px_w = Arc::clone(&display_px_shared);
        let stats_w = Arc::clone(&stats);
        let ks_w = Arc::clone(&keep_start_shared);
        let ke_w = Arc::clone(&keep_end_shared);

        std::thread::spawn(move || {
            loop {
                if cancel_w.load(Ordering::Relaxed) { break; }
                let req_opt: Option<LoadRequest> = {
                    let mut q = queue.lock().unwrap();
                    if q.is_empty() {
                        None
                    } else {
                        let vis = hint_w.load(Ordering::Relaxed);
                        let best = q
                            .iter()
                            .enumerate()
                            .min_by_key(|(_, r)| {
                                let tier: usize = if r.priority { 0 } else { 1 };
                                let i = r.idx;
                                let dist = if i < vis { vis - i } else { i - vis };
                                (tier, dist)
                            })
                            .map(|(pos, _)| pos)
                            .unwrap();
                        Some(q.swap_remove(best))
                    }
                };
                match req_opt {
                    Some(req) => {
                        if cancel_w.load(Ordering::Relaxed) { break; }
                        let ks = ks_w.load(Ordering::Relaxed);
                        let ke = ke_w.load(Ordering::Relaxed);
                        if req.idx < ks || req.idx >= ke { continue; }
                        let dp = display_px_w.load(Ordering::Relaxed);
                        process_load_request(
                            &req, &cache_map_w, &tx_w, catalog_w.as_deref(),
                            thumb_px, thumb_quality, dp, cache_decision, &done_w, &stats_w,
                            Some(&cancel_w), &ks_w, &ke_w,
                        );
                    }
                    None => {
                        std::thread::sleep(std::time::Duration::from_millis(5));
                    }
                }
            }
        });
    };

    // 通常ワーカー: reload_queue
    for _ in 0..regular_threads {
        spawn_worker(Arc::clone(&reload_queue));
    }
    // I/O ワーカー: heavy_io_queue
    for _ in 0..io_threads {
        spawn_worker(Arc::clone(&heavy_io_queue));
    }
    drop(tx); // メインスレッドの tx を閉じる

    // ── スクロールシミュレーション ──
    let t0 = Instant::now();
    let mut requested: HashSet<usize> = HashSet::new();
    let mut completed: HashSet<usize> = HashSet::new();
    let mut cache_hits = 0usize;
    let mut cache_misses = 0usize;
    let mut first_visible_ms: Option<f64> = None;
    let mut all_visible_ms: Option<f64> = None;
    let mut scroll_stopped_at: Option<Instant> = None;

    // スクロール: 1 行 = 30ms (33fps 相当)
    let scroll_step_ms = 30;
    let mut current_row: usize = 0;

    loop {
        // keep_range 計算
        let vis_first = current_row * cols;
        let vis_end = (vis_first + items_per_page).min(total);
        let keep_start = vis_first.saturating_sub(prev_pages * items_per_page);
        let keep_end = (vis_first + (1 + next_pages) * items_per_page).min(total);

        scroll_hint.store(vis_first, Ordering::Relaxed);
        keep_start_shared.store(keep_start, Ordering::Relaxed);
        keep_end_shared.store(keep_end, Ordering::Relaxed);

        // keep_range 外の requested を除去
        requested.retain(|&idx| idx >= keep_start && idx < keep_end);

        // 新規リクエストをキューに投入
        let mut new_reqs: Vec<LoadRequest> = Vec::new();
        for i in keep_start..keep_end {
            if requested.contains(&i) || completed.contains(&i) {
                continue;
            }
            let Some((mtime, file_size)) = contents.image_metas.get(i).copied().flatten() else {
                continue;
            };
            let Some(mut req) = contents.items.get(i).and_then(|item| {
                make_load_request(item, i, mtime, file_size)
            }) else {
                continue;
            };
            req.priority = i >= vis_first && i < vis_end;
            new_reqs.push(req);
        }

        if !new_reqs.is_empty() {
            let mut regular: Vec<LoadRequest> = Vec::new();
            let mut heavy: Vec<LoadRequest> = Vec::new();
            for r in new_reqs {
                let is_heavy = matches!(
                    contents.items.get(r.idx),
                    Some(GridItem::ZipFile(_) | GridItem::PdfFile(_) | GridItem::Folder(_))
                );
                if is_heavy { heavy.push(r); } else { regular.push(r); }
            }
            {
                let mut q = reload_queue.lock().unwrap();
                q.retain(|r| r.idx >= keep_start && r.idx < keep_end);
                for r in q.iter_mut() { r.priority = r.idx >= vis_first && r.idx < vis_end; }
                for r in regular { requested.insert(r.idx); q.push(r); }
            }
            {
                let mut q = heavy_io_queue.lock().unwrap();
                q.retain(|r| r.idx >= keep_start && r.idx < keep_end);
                for r in q.iter_mut() { r.priority = r.idx >= vis_first && r.idx < vis_end; }
                for r in heavy { requested.insert(r.idx); q.push(r); }
            }
        }

        // 結果受信 (ノンブロッキング)
        while let Ok((idx, _color_image, from_cache, _dims)) = rx.try_recv() {
            requested.remove(&idx);
            completed.insert(idx);
            if from_cache {
                cache_hits += 1;
            } else {
                cache_misses += 1;
            }

            // スクロール停止後の可視範囲チェック
            if let Some(ts) = scroll_stopped_at {
                let vis_first_final = target_row * cols;
                let vis_end_final = (vis_first_final + items_per_page).min(total);
                if idx >= vis_first_final && idx < vis_end_final {
                    if first_visible_ms.is_none() {
                        first_visible_ms = Some(ts.elapsed().as_secs_f64() * 1000.0);
                    }
                }
            }
        }

        // スクロール中は 1 行ずつ進む
        if current_row < target_row {
            current_row += 1;
            std::thread::sleep(std::time::Duration::from_millis(scroll_step_ms));
            continue;
        }

        // スクロール停止時刻を記録 (1 回のみ)
        if scroll_stopped_at.is_none() {
            scroll_stopped_at = Some(Instant::now());
        }
        let t_stop = scroll_stopped_at.unwrap();

        // スクロール停止後: 可視範囲の完了を待つ
        let vis_first_final = target_row * cols;
        let vis_end_final = (vis_first_final + items_per_page).min(total);

        // 可視範囲のロード対象アイテム数
        let vis_loadable: Vec<usize> = (vis_first_final..vis_end_final)
            .filter(|&i| contents.image_metas.get(i).copied().flatten().is_some())
            .filter(|&i| make_load_request(&contents.items[i], i, 0, 0).is_some())
            .collect();

        let vis_done = vis_loadable.iter().all(|i| completed.contains(i));
        if vis_done && all_visible_ms.is_none() {
            all_visible_ms = Some(t_stop.elapsed().as_secs_f64() * 1000.0);
        }

        // keep_range 全体の完了を待つ
        let keep_start_final = vis_first_final.saturating_sub(prev_pages * items_per_page);
        let keep_end_final = (vis_first_final + (1 + next_pages) * items_per_page).min(total);
        let keep_loadable: Vec<usize> = (keep_start_final..keep_end_final)
            .filter(|&i| contents.image_metas.get(i).copied().flatten().is_some())
            .filter(|i| contents.items.get(*i).and_then(|item| make_load_request(item, *i, 0, 0)).is_some())
            .collect();

        let keep_done = keep_loadable.iter().all(|i| completed.contains(i));
        if keep_done {
            break;
        }

        // 10ms 待ってリトライ
        std::thread::sleep(std::time::Duration::from_millis(10));
    }

    let t_stop = scroll_stopped_at.unwrap();
    let all_prefetch_ms = t_stop.elapsed().as_secs_f64() * 1000.0;
    let scroll_time_ms = (t_stop - t0).as_secs_f64() * 1000.0;

    // ワーカー停止
    cancel.store(true, Ordering::Relaxed);

    BenchResult {
        scroll_time_ms,
        first_visible_ms: first_visible_ms.unwrap_or(all_prefetch_ms),
        all_visible_ms: all_visible_ms.unwrap_or(all_prefetch_ms),
        all_prefetch_ms,
        cache_hits,
        cache_misses,
        total_received: completed.len(),
        zip_resolve_ms: Vec::new(),   // ログパース不要 — 全体時間で判断
        folder_resolve_ms: Vec::new(),
    }
}

// ───────────────────────────────────────────────────────────────────
// メイン
// ───────────────────────────────────────────────────────────────────

fn main() {
    // data_dir 初期化 (catalog パスの解決に必要)
    mimageviewer::data_dir::init();

    let args = parse_args();

    println!("=== Bench Scroll: {} ===", args.folder.display());

    // フォルダ走査
    let t_scan = Instant::now();
    let contents = scan_folder(&args.folder);
    let scan_ms = t_scan.elapsed().as_secs_f64() * 1000.0;
    let total = contents.items.len();
    let total_rows = total.div_ceil(args.cols);
    let target_row = args
        .scroll_to
        .unwrap_or(total_rows / 2)
        .min(total_rows.saturating_sub(1));

    println!(
        "Items: {} (folders={}, images={}, zips={}, pdfs={}, videos={})",
        total,
        contents.counts.folders,
        contents.counts.images,
        contents.counts.zips,
        contents.counts.pdfs,
        contents.counts.videos,
    );
    println!(
        "Grid: {} cols x {} rows = {} visible items",
        args.cols,
        args.rows,
        args.cols * args.rows,
    );
    println!(
        "Scroll target: row {} (item {})",
        target_row,
        target_row * args.cols,
    );
    println!("Threads: {}", args.threads);
    println!("Folder scan: {scan_ms:.0} ms");

    let cache_dir = catalog::default_cache_dir();

    // --delete-cache
    if args.delete_cache {
        let db_path = catalog::db_path_for(&cache_dir, &args.folder);
        if db_path.exists() {
            let _ = std::fs::remove_file(&db_path);
            println!("Cache deleted: {}", db_path.display());
        } else {
            println!("No cache to delete");
        }
    }

    // キャッシュ準備
    let catalog_arc: Option<Arc<CatalogDb>> =
        CatalogDb::open(&cache_dir, &args.folder).ok().map(Arc::new);

    let full_cache_map: HashMap<String, catalog::CacheEntry> = if args.no_cache {
        HashMap::new()
    } else {
        catalog_arc
            .as_ref()
            .and_then(|c| c.load_all().ok())
            .unwrap_or_default()
    };
    println!("Cache entries: {}", full_cache_map.len());
    println!();

    // ── Run 1: キャッシュあり ──
    {
        let cache_map = Arc::new(RwLock::new(full_cache_map));
        let result = run_bench(&args, &contents, cache_map, catalog_arc.clone(), "With cache");
        print_result("With cache", &result);
    }

    // ── Run 2: キャッシュなし ──
    if !args.no_cache {
        let cache_map = Arc::new(RwLock::new(HashMap::new()));
        let result = run_bench(&args, &contents, cache_map, catalog_arc.clone(), "Without cache");
        print_result("Without cache", &result);
    }
}
