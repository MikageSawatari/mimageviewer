//! `musicvideo_lab` — 音楽ビデオ書き出しのラボ GUI。
//!
//! 正本: `docs/music-video-export-plan.md`。各 increment のブリーフは `docs/brief-musicvideo-*.md`。

use std::path::PathBuf;
use std::sync::Arc;

use eframe::egui;
use musicvideo_core::Assets;

mod app;
mod decode;
mod export;
mod playback;
mod project;
mod project_file;

fn main() {
    let mut args = std::env::args().skip(1);
    let command = args.next();
    if command.as_deref() == Some("--benchmark") {
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
    if command.as_deref() == Some("--sample-frame") {
        let output = args
            .next()
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("target/musicvideo-p2a-sample.png"));
        let Some(font) = project::load_default_font() else {
            eprintln!("サンプル描画に必要な Windows 日本語フォントが見つかりません");
            std::process::exit(1);
        };
        if let Err(error) = export::render_sample_frame(&output, &font) {
            eprintln!("サンプル描画に失敗しました: {error}");
            std::process::exit(1);
        }
        println!("サンプル画像を書き出しました: {}", output.display());
        return;
    }
    if command.as_deref() == Some("--preview-benchmark") {
        let frames = args
            .next()
            .and_then(|value| value.parse::<usize>().ok())
            .unwrap_or(300);
        if let Err(error) = run_preview_benchmark(frames) {
            eprintln!("プレビューベンチマークに失敗しました: {error}");
            std::process::exit(1);
        }
        return;
    }
    if command.as_deref() == Some("--keyboard-distribution") {
        if let Err(error) = export::run_keyboard_distribution() {
            eprintln!("鍵盤点灯分布の測定に失敗しました: {error}");
            std::process::exit(1);
        }
        return;
    }
    if command.as_deref() == Some("--stationary-note-response") {
        if let Err(error) = export::run_stationary_note_response() {
            eprintln!("定常正弦波応答の測定に失敗しました: {error}");
            std::process::exit(1);
        }
        return;
    }
    if command.as_deref() == Some("--kick-envelope-response") {
        if let Err(error) = export::run_kick_envelope_response() {
            eprintln!("キック追従特性の測定に失敗しました: {error}");
            std::process::exit(1);
        }
        return;
    }
    if command.as_deref() == Some("--project-modulator-response") {
        let Some(path) = args.next().map(PathBuf::from) else {
            eprintln!("--project-modulator-response には project.mivmv.json のパスが必要です");
            std::process::exit(2);
        };
        if let Err(error) = export::run_project_modulator_response(&path) {
            eprintln!("実素材のモジュレータ測定に失敗しました: {error}");
            std::process::exit(1);
        }
        return;
    }
    if command.as_deref() == Some("--project-text-drag-diagnostic") {
        let Some(path) = args.next().map(PathBuf::from) else {
            eprintln!("--project-text-drag-diagnostic には project.mivmv.json のパスが必要です");
            std::process::exit(2);
        };
        if let Err(error) = export::run_project_text_drag_diagnostic(&path) {
            eprintln!("実素材のテキスト診断に失敗しました: {error}");
            std::process::exit(1);
        }
        return;
    }
    if let Err(error) = run_gui() {
        eprintln!("musicvideo_lab を起動できません: {error}");
    }
}

fn run_preview_benchmark(frames: usize) -> Result<(), String> {
    let font = project::load_default_font();
    let assets = project::create_assets(font.as_ref())?;
    assets.insert_image(
        musicvideo_core::ImageId::new(project::BACKGROUND_ID),
        project::synthetic_background(3000, 1687),
    );
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default().with_inner_size([1280.0, 820.0]),
        ..Default::default()
    };
    eframe::run_native(
        "mIV 音楽ビデオ ラボ — プレビュー計測",
        options,
        Box::new(move |creation| {
            project::setup_egui(&creation.egui_ctx, font.as_ref());
            app::MusicVideoLabApp::new_preview_benchmark(assets, frames)
                .map(|app| Box::new(app) as Box<dyn eframe::App>)
                .map_err(Into::into)
        }),
    )
    .map_err(|error| error.to_string())
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
