//! `musicvideo_lab` — 音楽ビデオ書き出しのラボ GUI。
//!
//! 正本: `docs/music-video-export-plan.md`。P1a のブリーフ: `docs/brief-musicvideo-p1a.md`。

use std::sync::Arc;

use eframe::egui;
use musicvideo_core::Assets;

mod app;
mod decode;
mod export;
mod project;

fn main() {
    let mut args = std::env::args().skip(1);
    if args.next().as_deref() == Some("--benchmark") {
        let frames = args
            .next()
            .and_then(|value| value.parse::<usize>().ok())
            .unwrap_or(600);
        let Some(font) = project::load_default_font() else {
            eprintln!("ベンチマークに必要な Windows 日本語フォントが見つかりません");
            std::process::exit(1);
        };
        if let Err(error) = export::run_benchmark(frames, &font) {
            eprintln!("ベンチマークに失敗しました: {error}");
            std::process::exit(1);
        }
        return;
    }

    if let Err(error) = run_gui() {
        eprintln!("musicvideo_lab を起動できません: {error}");
    }
}

fn run_gui() -> eframe::Result {
    let font = project::load_default_font();
    let assets = project::create_assets(font.as_ref()).unwrap_or_else(|error| {
        eprintln!("{error}");
        Arc::new(Assets::default())
    });
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default().with_inner_size([1280.0, 820.0]),
        ..Default::default()
    };
    eframe::run_native(
        "mIV 音楽ビデオ ラボ",
        options,
        Box::new(move |creation| {
            project::setup_egui(&creation.egui_ctx, font.as_ref());
            Ok(Box::new(app::MusicVideoLabApp::new(assets)))
        }),
    )
}
