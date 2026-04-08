#![windows_subsystem = "windows"]

mod app;
pub mod catalog;
pub mod logger;
pub mod monitor;
pub mod settings;

use std::sync::Arc;

fn main() -> eframe::Result {
    logger::init();

    // 保存済み設定からウィンドウ初期状態を決定する
    let saved = settings::Settings::load();

    let default_size = [1280.0_f32, 800.0_f32];
    let size = saved.window_size.unwrap_or(default_size);

    let mut viewport = egui::ViewportBuilder::default()
        .with_title("mimageviewer")
        .with_inner_size(size);

    // 保存済み位置にモニターが接続されている場合のみ適用する。
    // 接続モニターが減って座標が画面外になった場合はデフォルト位置を使う。
    if let Some([x, y]) = saved.window_pos {
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
