#![windows_subsystem = "windows"]

pub mod activity_gate;
pub mod adjustment;
pub mod adjustment_db;
pub mod ai;
mod app;
pub mod archive_cache;
pub mod archive_converter;
pub mod cache_maintenance;
pub mod catalog;
pub mod data_dir;
pub mod delete_worker;
pub mod dwm_transitions;
pub mod exif_reader;
pub mod fast_resize;
pub mod folder_rating_counter;
pub mod folder_tree;
pub mod fs_animation;
pub mod fts_index;
pub mod fts_meta;
pub mod fts_writer_dispatcher;
pub mod global_search;
mod global_search_ui;
pub mod gpu_info;
pub mod grid_item;
pub mod indexer_manager;
pub mod indexer_progress;
pub mod indexer_supervisor;
pub mod ingest_text;
pub mod ingest_worker;
pub mod io_semaphore;
pub mod logger;
pub mod mask_db;
pub mod monitor;
pub mod name_bulk_indexer;
pub mod name_index_supervisor;
pub mod open_with;
pub mod os_theme;
pub mod path_key;
pub mod pdf_loader;
pub mod pdf_passwords;
pub mod perf;
pub mod png_metadata;
pub mod post_filter;
pub mod rating_db;
pub mod rotation_db;
pub mod search_index_db;
pub mod search_norm;
pub mod search_query;
pub mod search_walker;
pub mod search_watcher;
pub mod settings;
pub mod sidecar;
pub mod single_instance;
pub mod spread_db;
pub mod stats;
pub mod susie_loader;
pub mod sys_memory;
mod tag_ops;
mod tag_prewarm;
pub mod rating_write_worker;
pub mod tag_write_worker;
pub mod thumb_loader;
mod undo_ops;
pub mod undo_stack;
pub mod tray;
mod tray_integration;
mod ui_adjustment_panel;
mod ui_analysis_panel;
pub mod ui_dialogs;
mod ui_erase;
mod ui_fullscreen;
pub mod ui_helpers;
mod ui_main;
mod ui_metadata_panel;
pub mod ui_susie_diagnostic;
pub mod update_check;
pub mod video;
pub mod video_thumb;
pub mod wic_decoder;
pub mod xmp_reader;
pub mod xmp_writer;
pub mod zip_loader;

use std::sync::Arc;
use std::time::Instant;

/// `startup.<step>` perf イベントを emit する共通ヘルパー。
/// `phase_start` を渡すと当該フェーズの `ms` + 累計 `total_ms` を、
/// `None` を渡すとマーカー用として `total_ms` のみを記録する。
/// `total_ms` は `perf::program_start()` (= `perf::init` に渡した基準 Instant)
/// 経由で計算するので、事前に `perf::init(enabled, Some(prog_start))` を呼んでおくこと。
/// `perf::is_enabled()` が false なら no-op。
fn emit_startup(step: &str, phase_start: Option<Instant>) {
    if !perf::is_enabled() {
        return;
    }
    let Some(base) = perf::program_start() else {
        return;
    };
    let total_ms = base.elapsed().as_secs_f64() * 1000.0;
    let mut extras: Vec<(&str, serde_json::Value)> = Vec::with_capacity(2);
    if let Some(start) = phase_start {
        extras.push((
            "ms",
            serde_json::Value::from(start.elapsed().as_secs_f64() * 1000.0),
        ));
    }
    extras.push(("total_ms", serde_json::Value::from(total_ms)));
    perf::event("startup", step, None, 0, &extras);
}

fn main() -> eframe::Result {
    // main() 入口の Instant を起動時間計測の t=0 とする。
    // --pdf-worker モードでは計測しないので worker 判定の前に取らない。
    // --perf-log 無効時は `emit_startup` が no-op なのでコストはゼロ。
    let prog_start = Instant::now();

    // --pdf-worker モード: GUI なしで PDFium ワーカープロセスとして起動
    if std::env::args().any(|a| a == pdf_loader::PDF_WORKER_ARG) {
        pdf_loader::run_worker_process();
        std::process::exit(0);
    }

    // シングルインスタンス検出 (Windows): Named Mutex で 2 重起動を排除する。
    // インストーラの AppMutex と名前を合わせることでアップデート時の「閉じてください」
    // ダイアログ自動連携も兼ねる (`single_instance::MUTEX_NAME` 参照)。
    // is_first_instance() == false のときは既にもう 1 つ mIV が動いているので
    // 静かに exit する (トレイ常駐中でもここで落ちる = ユーザーはトレイアイコンから
    // 復帰することで操作を再開できる)。
    let _single_instance = single_instance::SingleInstanceGuard::acquire();
    if !_single_instance.is_first_instance() {
        // 2 重起動: 既存インスタンスの activate event を叩いてウィンドウを前面に出す。
        // ユーザーが「もう一度 mIV を起動」した意図を既存インスタンスで復帰として解釈する。
        let signaled = single_instance::signal_activate_existing();
        eprintln!(
            "mImageViewer is already running (activate signaled: {signaled}). Exiting second instance."
        );
        std::process::exit(0);
    }

    // data_dir::init() は perf::init が logs_dir を使うため先行させる必要がある。
    let t0 = Instant::now();
    data_dir::init();
    let data_dir_elapsed = t0.elapsed();

    // デバッグビルドでは常にログ出力。リリースビルドでは --log 引数で有効化
    let log_enabled = cfg!(debug_assertions) || std::env::args().any(|a| a == "--log");
    if log_enabled {
        logger::init();
    }

    // --perf-log: 構造化イベントログ (JSON Lines) を有効化する。
    // 無指定時は `perf::is_enabled()` が false のまま、全 perf::event 呼出しが即 return。
    // prog_start を基準にすることで startup.* イベントの `total_ms` が真の経過時間を指す。
    let perf_enabled = std::env::args().any(|a| a == "--perf-log");
    perf::init(perf_enabled, Some(prog_start));

    // 起動時間計測: data_dir 初期化は先行ステップなので perf::init 後に後追いで打つ。
    // phase_start を渡すと ms を載せられるが、ここは経過分を再現できないので
    // data_dir_elapsed を直接 ms として埋める。
    if perf::is_enabled() {
        let total_ms = prog_start.elapsed().as_secs_f64() * 1000.0;
        perf::event(
            "startup",
            "data_dir_init",
            None,
            0,
            &[
                (
                    "ms",
                    serde_json::Value::from(data_dir_elapsed.as_secs_f64() * 1000.0),
                ),
                ("total_ms", serde_json::Value::from(total_ms)),
            ],
        );
    }

    // AI モデルを %APPDATA%\mimageviewer\models\ に展開（サイズ一致ならスキップ）
    let t = Instant::now();
    ai::model_manager::ensure_models_extracted();
    emit_startup("models_extract", Some(t));

    // Susie 32bit ワーカー exe を %APPDATA%\mimageviewer\mimageviewer-susie32.exe に展開。
    // PDFium DLL と同じパターンで本体 exe に埋め込み、初回起動時に書き出す。
    let t = Instant::now();
    susie_loader::ensure_worker_extracted();
    emit_startup("susie_worker_extract", Some(t));

    // Susie プラグインワーカープール: バックグラウンドで初期化する
    // (プラグインが多いと handshake に数百ms かかる可能性があるため、
    //  起動 UI をブロックしないようスレッドに逃がす)
    std::thread::Builder::new()
        .name("susie-init".to_string())
        .spawn(|| {
            let _ = susie_loader::get_pool();
        })
        .ok();

    // パニック時にログファイルへ記録するフック（windows_subsystem = "windows" では
    // stderr が見えないため、ここで捕捉しないとクラッシュ原因が不明になる）
    std::panic::set_hook(Box::new(|info| {
        let payload = if let Some(s) = info.payload().downcast_ref::<&str>() {
            (*s).to_string()
        } else if let Some(s) = info.payload().downcast_ref::<String>() {
            s.clone()
        } else {
            "unknown payload".to_string()
        };
        let location = info
            .location()
            .map(|l| format!("{}:{}:{}", l.file(), l.line(), l.column()))
            .unwrap_or_else(|| "unknown location".to_string());
        let bt = std::backtrace::Backtrace::force_capture();
        let msg = format!("PANIC at {location}: {payload}\n{bt}");
        logger::log(&msg);
        let log_dir = data_dir::logs_dir();
        let _ = std::fs::create_dir_all(&log_dir);
        let panic_log = log_dir.join("panic.log");
        if let Ok(mut f) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&panic_log)
        {
            use std::io::Write;
            let _ = writeln!(f, "[{:?}] {msg}", std::time::SystemTime::now());
        }
    }));

    // 保存済み設定からウィンドウ初期状態を決定する
    let t = Instant::now();
    let saved = settings::Settings::load();
    emit_startup("settings_load", Some(t));

    let default_size = [1280.0_f32, 800.0_f32];
    // --window-size WxH 引数があればそれを優先（スクリーンショット用）
    let size = parse_window_size_arg().unwrap_or_else(|| saved.window_size.unwrap_or(default_size));

    let t = Instant::now();
    let icon = Arc::new(load_icon());
    emit_startup("load_icon", Some(t));

    let mut viewport = egui::ViewportBuilder::default()
        .with_title("mimageviewer")
        .with_inner_size(size)
        .with_icon(icon);

    // --window-size 指定時は位置を画面左上寄りに固定（保存済み位置は無視）
    if parse_window_size_arg().is_some() {
        viewport = viewport.with_position(egui::pos2(60.0, 40.0));
    } else if let Some([x, y]) = saved.window_pos {
        let w = saved.window_size.map(|[w, _]| w).unwrap_or(1280.0);
        if monitor::title_bar_on_some_monitor(x, y, w) {
            viewport = viewport.with_position(egui::pos2(x, y));
        }
    }

    let options = eframe::NativeOptions {
        viewport,
        ..Default::default()
    };

    // eframe::run_native に入る手前までを 1 つの marker として記録する。
    // これ以降は eframe (winit + wgpu) の初期化が走り、creator closure が呼ばれる。
    emit_startup("before_run_native", None);

    eframe::run_native(
        "mimageviewer",
        options,
        Box::new(move |cc| {
            // creator closure: wgpu/winit 初期化後に 1 回だけ呼ばれる。
            // この closure の先頭までの所要時間 = eframe 自体のセットアップ時間。
            emit_startup("creator_enter", None);
            let t = Instant::now();
            setup_fonts(&cc.egui_ctx);
            emit_startup("setup_fonts", Some(t));
            // 起動時点で UI テーマを先行適用して、初回フレームでの
            // ダーク/ライト切替ちらつきを避ける (set_visuals は次フレームから
            // 効くため、App::update 内で適用すると 1 フレームだけデフォルト
            // ダーク表示になる)。
            let t = Instant::now();
            let resolved = os_theme::resolve(saved.ui_theme);
            os_theme::apply_resolved(&cc.egui_ctx, resolved);
            emit_startup("apply_theme", Some(t));
            let t = Instant::now();
            let mut app = app::App::default();
            emit_startup("app_default", Some(t));
            app.applied_ui_theme = Some(resolved);

            // 動画 GPU レンダリング用の wgpu::Device / Queue を保存。
            // また同時に共有 D3D11 デバイスを初期化 (失敗してもアプリは起動継続、
            // 動画は旧経路 = CPU readback + swscale にフォールバック)。
            #[cfg(windows)]
            {
                if let Some(rs) = cc.wgpu_render_state.clone() {
                    app.wgpu_render_state = Some(rs);
                    match crate::video::gpu_renderer::GpuVideoDevice::new(
                        app.settings.video_rtx_vsr,
                    ) {
                        Ok(dev) => {
                            crate::logger::log(
                                "GPU video device: created (D3D11 + video processor)".to_string(),
                            );
                            app.gpu_video_device = Some(dev);
                        }
                        Err(e) => {
                            crate::logger::log(format!(
                                "GPU video device: failed (will fallback to CPU readback): {e}"
                            ));
                        }
                    }
                }
            }
            // お気に入り単位の補正標準を DB から復元 (+ 削除されたお気に入りの orphan 行を掃除)。
            let t = Instant::now();
            app.hydrate_adjustment_favorite_params();
            emit_startup("hydrate_adj_favs", Some(t));
            // name index supervisor を起動時に spawn (auto_index_structure=true なお気に入り)。
            // IndexerManager::sync_with_favorites がメタ側の対応処理を既に走らせているが、
            // 名前索引は IndexerManager 外の管理なのでここで別途 spawn する。
            let t = Instant::now();
            app.spawn_initial_name_index_supervisors();
            emit_startup("spawn_name_idx_sup", Some(t));
            // DPI 確定後の初回フレームで意図したサイズを再適用する
            // (egui#4918 / winit#923 対策)。ViewportBuilder 段階では
            // マルチモニタ DPI 混在時にサイズが壊れるケースがある。
            app.pending_initial_size = Some(size);
            emit_startup("creator_exit", None);
            Ok(Box::new(app))
        }),
    )
}

/// `--window-size WxH` 引数をパース（例: `--window-size 1400x860`）。
fn parse_window_size_arg() -> Option<[f32; 2]> {
    let args: Vec<String> = std::env::args().collect();
    for i in 0..args.len().saturating_sub(1) {
        if args[i] == "--window-size" {
            let parts: Vec<&str> = args[i + 1].split('x').collect();
            if parts.len() == 2 {
                if let (Ok(w), Ok(h)) = (parts[0].parse::<f32>(), parts[1].parse::<f32>()) {
                    return Some([w, h]);
                }
            }
        }
    }
    None
}

fn load_icon() -> egui::IconData {
    let bytes = include_bytes!("../assets/icon.png");
    let img = image::load_from_memory(bytes)
        .expect("icon.png の読み込み失敗")
        .into_rgba8();
    let (width, height) = img.dimensions();
    egui::IconData {
        rgba: img.into_raw(),
        width,
        height,
    }
}

fn setup_fonts(ctx: &egui::Context) {
    let mut fonts = egui::FontDefinitions::default();

    // Windows システムフォントから日本語フォントを読み込む
    let font_paths = [
        r"C:\Windows\Fonts\YuGothM.ttc",
        r"C:\Windows\Fonts\meiryo.ttc",
        r"C:\Windows\Fonts\msgothic.ttc",
    ];

    for path in &font_paths {
        if let Ok(data) = std::fs::read(path) {
            fonts.font_data.insert(
                "japanese".to_owned(),
                Arc::new(egui::FontData::from_owned(data)),
            );
            // 日本語フォントをリストの先頭に挿入してプライマリにする。
            // fallback（末尾追加）にすると Latin フォントとメトリクスが混在し、
            // TextEdit 等で文字の縦位置がずれる。
            fonts
                .families
                .entry(egui::FontFamily::Proportional)
                .or_default()
                .insert(0, "japanese".to_owned());
            fonts
                .families
                .entry(egui::FontFamily::Monospace)
                .or_default()
                .insert(0, "japanese".to_owned());
            break;
        }
    }

    ctx.set_fonts(fonts);
}
