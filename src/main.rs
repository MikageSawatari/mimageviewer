#![windows_subsystem = "windows"]

pub mod ai;
mod app;
pub mod catalog;
pub mod data_dir;
pub mod folder_tree;
pub mod fs_animation;
pub mod gpu_info;
pub mod grid_item;
pub mod exif_reader;
pub mod logger;
pub mod monitor;
pub mod png_metadata;
pub mod rotation_db;
pub mod settings;
pub mod spread_db;
pub mod stats;
pub mod thumb_loader;
pub mod ui_dialogs;
mod ui_analysis_panel;
mod ui_fullscreen;
pub mod ui_helpers;
mod ui_main;
mod ui_metadata_panel;
pub mod video_thumb;
pub mod open_with;
pub mod pdf_loader;
pub mod pdf_passwords;
pub mod wic_decoder;
pub mod zip_loader;

use std::sync::Arc;

fn main() -> eframe::Result {
    // --pdf-worker モード: GUI なしで PDFium ワーカープロセスとして起動
    if std::env::args().any(|a| a == pdf_loader::PDF_WORKER_ARG) {
        pdf_loader::run_worker_process();
        std::process::exit(0);
    }

    data_dir::init();

    // デバッグビルドでは常にログ出力。リリースビルドでは --log 引数で有効化
    let log_enabled = cfg!(debug_assertions)
        || std::env::args().any(|a| a == "--log");
    if log_enabled {
        logger::init();
    }

    // 保存済み設定からウィンドウ初期状態を決定する
    let saved = settings::Settings::load();

    let default_size = [1280.0_f32, 800.0_f32];
    // --window-size WxH 引数があればそれを優先（スクリーンショット用）
    let size = parse_window_size_arg().unwrap_or_else(|| saved.window_size.unwrap_or(default_size));

    let mut viewport = egui::ViewportBuilder::default()
        .with_title("mimageviewer")
        .with_inner_size(size)
        .with_icon(Arc::new(load_icon()));

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

    eframe::run_native(
        "mimageviewer",
        options,
        Box::new(|cc| {
            setup_fonts(&cc.egui_ctx);
            Ok(Box::new(app::App::default()))
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
