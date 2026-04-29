//! vst3-host-tester
//!
//! mIV 本体に依存しない検証用 GUI 単独 exe。
//! 開発フローは `cargo run -p vst3-host-tester`。
//!
//! 目的:
//!  - VST3 プラグイン (.vst3 bundle) のスキャン
//!  - bridge 子プロセス (`mimageviewer-vst3-host.exe`) 経由でロード
//!  - 固定音源 (440Hz サイン波) を生成し、bridge 経路と直結を A/B 比較
//!  - latency / エラー / プラグイン GUI 表示の検証
//!
//! プラグイン GUI 表示 (`SetParent` + winit) は別イテレーションで追加する。
//! まずは無 GUI でロード〜パススルー音声を確認できる状態が目標。

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod audio;
mod bridge;
mod scanner;

use std::sync::{Arc, Mutex};

use audio::{AudioEngine, Mode, ToneParams};
use bridge::{Bridge, Cmd, Event};
use scanner::DiscoveredPlugin;

fn main() -> eframe::Result {
    let native_options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([720.0, 520.0])
            .with_title("VST3 Host Tester"),
        ..Default::default()
    };
    eframe::run_native(
        "vst3-host-tester",
        native_options,
        Box::new(|cc| {
            let app = TesterApp::new(cc);
            Ok(Box::new(app))
        }),
    )
}

struct TesterApp {
    plugins: Vec<DiscoveredPlugin>,
    selected: Option<usize>,

    bridge: Option<Arc<Bridge>>,
    audio: Option<AudioEngine>,
    mode: Arc<Mutex<Mode>>,
    tone: ToneParams,

    last_loaded_name: Option<String>,
    last_latency: Option<u32>,
    bridge_exe_path: std::path::PathBuf,

    // log buffer for the bottom panel
    log_lines: Arc<Mutex<Vec<String>>>,
}

impl TesterApp {
    fn new(_cc: &eframe::CreationContext<'_>) -> Self {
        // bridge exe は worktree 内の vendor/vst3-host/ にある (CMake の出力先)
        // 実行時の cwd は workspace ルートまたは tester crate ディレクトリのどちらかが想定される
        let candidates = [
            std::path::PathBuf::from("vendor/vst3-host/mimageviewer-vst3-host.exe"),
            std::path::PathBuf::from("../../vendor/vst3-host/mimageviewer-vst3-host.exe"),
        ];
        let bridge_exe_path = candidates
            .iter()
            .find(|p| p.exists())
            .cloned()
            .unwrap_or_else(|| candidates[0].clone());

        let mut app = Self {
            plugins: Vec::new(),
            selected: None,
            bridge: None,
            audio: None,
            mode: Arc::new(Mutex::new(Mode::Bypass)),
            tone: ToneParams::default(),
            last_loaded_name: None,
            last_latency: None,
            bridge_exe_path,
            log_lines: Arc::new(Mutex::new(Vec::new())),
        };
        app.scan();
        app.start_audio();
        app
    }

    fn log(&self, line: impl Into<String>) {
        let mut lines = self.log_lines.lock().unwrap();
        lines.push(line.into());
        // 上限: 200 行
        if lines.len() > 200 {
            let drop_n = lines.len() - 200;
            lines.drain(..drop_n);
        }
    }

    fn scan(&mut self) {
        let roots = scanner::default_vst3_paths();
        self.plugins = scanner::scan(&roots);
        self.log(format!("scan: {} plugin(s) found", self.plugins.len()));
    }

    fn start_audio(&mut self) {
        match AudioEngine::start(self.bridge.clone(), Arc::clone(&self.mode), self.tone.clone()) {
            Ok(eng) => {
                self.log(format!(
                    "audio engine started: {}Hz, block_size={}",
                    eng.sample_rate, eng.block_size
                ));
                self.audio = Some(eng);
            }
            Err(e) => {
                self.log(format!("audio engine failed: {e}"));
            }
        }
    }

    fn restart_audio(&mut self) {
        self.audio = None;
        self.start_audio();
    }

    fn load_selected(&mut self) {
        let Some(sel) = self.selected else {
            self.log("no plugin selected");
            return;
        };
        let Some(plugin) = self.plugins.get(sel).cloned() else {
            return;
        };
        self.log(format!("loading: {}", plugin.path.display()));

        // bridge 起動 (まだなら)
        if self.bridge.is_none() {
            if !self.bridge_exe_path.exists() {
                self.log(format!(
                    "bridge exe not found at {} — build it via cmake first",
                    self.bridge_exe_path.display()
                ));
                return;
            }
            match Bridge::spawn(&self.bridge_exe_path) {
                Ok(mut br) => {
                    if let Err(e) = br.send(&Cmd::Hello { version: 1 }) {
                        self.log(format!("bridge hello send failed: {e}"));
                        return;
                    }
                    match br.recv() {
                        Ok(Event::Ready { version }) => {
                            self.log(format!("bridge ready (protocol v{version})"));
                        }
                        Ok(other) => {
                            self.log(format!("unexpected event: {other:?}"));
                            return;
                        }
                        Err(e) => {
                            self.log(format!("bridge recv failed: {e}"));
                            return;
                        }
                    }
                    let sample_rate =
                        self.audio.as_ref().map(|a| a.sample_rate).unwrap_or(48_000);
                    let block_size = self.audio.as_ref().map(|a| a.block_size).unwrap_or(480);
                    let plugin_path = plugin.path.to_string_lossy().to_string();
                    if let Err(e) = br.open_audio_pipe(&plugin_path, sample_rate, block_size) {
                        self.log(format!("open_audio_pipe failed: {e}"));
                        return;
                    }
                    // open 応答を待つ
                    match br.recv() {
                        Ok(Event::Loaded {
                            plugin_name,
                            latency_samples,
                        }) => {
                            self.log(format!(
                                "loaded: {} (latency={} samples)",
                                plugin_name, latency_samples
                            ));
                            self.last_loaded_name = Some(plugin_name);
                            self.last_latency = Some(latency_samples);
                        }
                        Ok(Event::Error { detail }) => {
                            self.log(format!("load error: {detail}"));
                            return;
                        }
                        Ok(other) => {
                            self.log(format!("unexpected open response: {other:?}"));
                        }
                        Err(e) => {
                            self.log(format!("recv after open failed: {e}"));
                            return;
                        }
                    }
                    self.bridge = Some(Arc::new(br));
                    // bridge ハンドルが変わったので audio engine を再構築
                    self.restart_audio();
                }
                Err(e) => {
                    self.log(format!("bridge spawn failed: {e}"));
                }
            }
        }
    }

    fn unload(&mut self) {
        // audio engine を先に止めて bridge への参照を 1 つに減らす
        self.audio = None;
        if let Some(br) = self.bridge.take() {
            match Arc::try_unwrap(br) {
                Ok(mut br) => {
                    let _ = br.send(&Cmd::Close);
                    let _ = br.shutdown();
                    self.log("bridge shut down");
                }
                Err(_arc) => {
                    // ここに来るのは設計バグ。audio engine 以外で Arc を保持しているなら
                    // それを先に手放す必要がある。Arc を drop して bridge は Drop の
                    // kill フローに任せる。
                    self.log("warning: bridge had extra refs; relying on Drop kill");
                }
            }
            self.last_loaded_name = None;
            self.last_latency = None;
        }
        self.restart_audio();
    }
}

impl eframe::App for TesterApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        egui::TopBottomPanel::top("top").show(ctx, |ui| {
            ui.horizontal(|ui| {
                ui.heading("VST3 Host Tester");
                ui.separator();
                ui.label(format!("bridge exe: {}", self.bridge_exe_path.display()));
            });
        });

        egui::SidePanel::left("plugins")
            .resizable(true)
            .default_width(280.0)
            .show(ctx, |ui| {
                ui.heading("Plugins");
                if ui.button("Rescan").clicked() {
                    self.scan();
                }
                ui.separator();
                let mut clicked: Option<usize> = None;
                egui::ScrollArea::vertical().show(ui, |ui| {
                    for (i, p) in self.plugins.iter().enumerate() {
                        let selected = self.selected == Some(i);
                        if ui
                            .selectable_label(selected, &p.display_name)
                            .on_hover_text(p.path.display().to_string())
                            .clicked()
                        {
                            clicked = Some(i);
                        }
                    }
                });
                if let Some(i) = clicked {
                    self.selected = Some(i);
                }
            });

        egui::CentralPanel::default().show(ctx, |ui| {
            ui.horizontal(|ui| {
                let load_enabled = self.selected.is_some() && self.bridge.is_none();
                if ui
                    .add_enabled(load_enabled, egui::Button::new("Load selected"))
                    .clicked()
                {
                    self.load_selected();
                }
                let unload_enabled = self.bridge.is_some();
                if ui
                    .add_enabled(unload_enabled, egui::Button::new("Unload"))
                    .clicked()
                {
                    self.unload();
                }
            });
            ui.separator();
            if let Some(name) = &self.last_loaded_name {
                ui.label(format!("Loaded plugin: {name}"));
            }
            if let Some(lat) = self.last_latency {
                ui.label(format!("Reported latency: {lat} samples"));
            }

            ui.separator();
            ui.heading("Audio");
            {
                let mut mode_val = *self.mode.lock().unwrap();
                ui.horizontal(|ui| {
                    ui.label("Mode:");
                    if ui
                        .radio_value(&mut mode_val, Mode::Bypass, "Bypass (no plugin)")
                        .changed()
                    {
                        *self.mode.lock().unwrap() = mode_val;
                    }
                    let through_enabled = self.bridge.is_some();
                    let resp = ui.add_enabled(
                        through_enabled,
                        egui::RadioButton::new(mode_val == Mode::Through, "Through (via plugin)"),
                    );
                    if resp.clicked() && through_enabled {
                        *self.mode.lock().unwrap() = Mode::Through;
                    }
                });
            }

            ui.horizontal(|ui| {
                use std::sync::atomic::Ordering;
                let mut muted = self.tone.muted.load(Ordering::Relaxed);
                if ui.checkbox(&mut muted, "Mute tone").changed() {
                    self.tone.muted.store(muted, Ordering::Relaxed);
                }
                let mut freq = self.tone.freq_hz.load(Ordering::Relaxed) as f32;
                if ui
                    .add(
                        egui::Slider::new(&mut freq, 80.0..=4000.0)
                            .logarithmic(true)
                            .text("Tone Hz"),
                    )
                    .changed()
                {
                    self.tone.freq_hz.store(freq as u32, Ordering::Relaxed);
                }
                let mut amp_milli = self.tone.amplitude_milli.load(Ordering::Relaxed) as f32;
                if ui
                    .add(egui::Slider::new(&mut amp_milli, 0.0..=500.0).text("Amp (× 0.001)"))
                    .changed()
                {
                    self.tone
                        .amplitude_milli
                        .store(amp_milli as u32, Ordering::Relaxed);
                }
            });

            ui.separator();
            ui.heading("Log");
            egui::ScrollArea::vertical()
                .auto_shrink([false; 2])
                .stick_to_bottom(true)
                .show(ui, |ui| {
                    let lines = self.log_lines.lock().unwrap();
                    for line in lines.iter() {
                        ui.label(line);
                    }
                });
        });

        // bridge から非同期に来るイベント (latency_changed 等) のポーリングは
        // Phase 0b 後段で追加する。現状は load 時の同期 recv のみ。
        ctx.request_repaint_after(std::time::Duration::from_millis(100));
    }
}
