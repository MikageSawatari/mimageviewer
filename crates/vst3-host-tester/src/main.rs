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
#[cfg(windows)]
mod plugin_gui;
mod scanner;

use std::sync::{Arc, Mutex};

use audio::{AudioEngine, Mode, ToneParams};
use bridge::{Bridge, Cmd, Event};
use scanner::DiscoveredPlugin;

/// Windows のシステムフォントから日本語フォントを読み込んで egui に設定する。
/// 未設定だとログ等の日本語が □ で表示される。
fn setup_fonts(ctx: &egui::Context) {
    let mut fonts = egui::FontDefinitions::default();
    let font_paths = [
        r"C:\Windows\Fonts\YuGothM.ttc",
        r"C:\Windows\Fonts\meiryo.ttc",
        r"C:\Windows\Fonts\msgothic.ttc",
    ];
    for path in &font_paths {
        if let Ok(data) = std::fs::read(path) {
            fonts.font_data.insert(
                "japanese".to_owned(),
                std::sync::Arc::new(egui::FontData::from_owned(data)),
            );
            // 先頭に挿入 = プライマリ。fallback (末尾) だと Latin と
            // メトリクスが混在して TextEdit の縦位置がずれる。
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
    /// 選択中のプラグインインデックス (`plugins` に対する index)。
    /// フィルタ後の表示インデックスではなく、絞り込みが変わっても同じ要素を指し続けるため
    /// `plugins` 側を使う。
    selected: Option<usize>,
    /// 検索フィルタ (case-insensitive 部分一致)。空なら全件表示。
    filter: String,

    bridge: Option<Arc<Bridge>>,
    audio: Option<AudioEngine>,
    mode: Arc<Mutex<Mode>>,
    tone: ToneParams,

    last_loaded_name: Option<String>,
    last_latency: Option<u32>,
    bridge_exe_path: std::path::PathBuf,

    /// audio engine 診断ログ用の最終値
    last_audio_log_at: std::time::Instant,
    last_logged_frames: u32,
    last_logged_underruns: u32,
    last_logged_partials: u32,
    /// 診断用: bridge 経由するが plugin process を skip する。
    /// 歪みの原因切り分け用。
    bridge_passthrough: bool,

    // GUI 表示まわり
    #[cfg(windows)]
    gui_host: plugin_gui::GuiHost,
    /// 現在表示中のプラグイン GUI ウィンドウの HWND (u64 化)。0 = 表示なし。
    gui_hwnd: u64,
    /// GUI ウィンドウの × クリックを GUI スレッドから受け取る側。Some の時のみ表示中。
    gui_close_signal:
        Option<std::sync::Arc<std::sync::Mutex<Option<std::sync::mpsc::Receiver<()>>>>>,
    /// GUI ウィンドウのユーザーリサイズを GUI スレッドから受け取る側。
    gui_resize_signal:
        Option<std::sync::Arc<std::sync::Mutex<Option<std::sync::mpsc::Receiver<(u32, u32)>>>>>,

    // log buffer for the bottom panel
    log_lines: Arc<Mutex<Vec<String>>>,
    /// 永続ログファイル (= 解析依頼時に Claude に直接読ませる用)。
    /// 場所: %TEMP%\vst3-host-tester.log (= 例: C:\Users\<USER>\AppData\Local\Temp\)
    log_file: Option<Arc<Mutex<std::fs::File>>>,
    log_file_path: std::path::PathBuf,
}

impl TesterApp {
    fn new(cc: &eframe::CreationContext<'_>) -> Self {
        setup_fonts(&cc.egui_ctx);
        // 永続ログファイル: %TEMP%\vst3-host-tester.log
        // 起動ごとに上書き (truncate) して 1 セッション分だけ残す。
        // 解析時はこのファイルを Claude Code に Read させればよい。
        let log_file_path = std::env::temp_dir().join("vst3-host-tester.log");
        let log_file = std::fs::OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(true)
            .open(&log_file_path)
            .ok()
            .map(|f| Arc::new(Mutex::new(f)));
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
            filter: String::new(),
            bridge: None,
            audio: None,
            mode: Arc::new(Mutex::new(Mode::Bypass)),
            tone: ToneParams::default(),
            last_loaded_name: None,
            last_latency: None,
            bridge_exe_path,
            last_audio_log_at: std::time::Instant::now(),
            last_logged_frames: 0,
            last_logged_underruns: 0,
            last_logged_partials: 0,
            bridge_passthrough: false,
            log_lines: Arc::new(Mutex::new(Vec::new())),
            log_file,
            log_file_path,
            #[cfg(windows)]
            gui_host: plugin_gui::GuiHost::spawn(),
            gui_hwnd: 0,
            gui_close_signal: None,
            gui_resize_signal: None,
        };
        app.log(format!("ログファイル: {}", app.log_file_path.display()));
        app.scan();
        app.start_audio();
        app
    }

    fn log(&self, line: impl Into<String>) {
        let line: String = line.into();
        // タイムスタンプ付きでファイルに append (UI 側はそのまま)
        if let Some(file) = self.log_file.as_ref() {
            use std::io::Write;
            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_secs_f64())
                .unwrap_or(0.0);
            if let Ok(mut f) = file.lock() {
                let _ = writeln!(f, "[{:>13.3}] {}", now, &line);
                let _ = f.flush();
            }
        }
        let mut lines = self.log_lines.lock().unwrap();
        lines.push(line);
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
        match AudioEngine::start(
            self.bridge.clone(),
            Arc::clone(&self.mode),
            self.tone.clone(),
        ) {
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
            // bridge の stderr はログファイル + UI ログに合流させる
            let log_lines_for_bridge = Arc::clone(&self.log_lines);
            let log_file_for_bridge = self.log_file.clone();
            let stderr_cb = move |line: String| {
                if let Some(file) = &log_file_for_bridge {
                    use std::io::Write;
                    let now = std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .map(|d| d.as_secs_f64())
                        .unwrap_or(0.0);
                    if let Ok(mut f) = file.lock() {
                        let _ = writeln!(f, "[{:>13.3}] [bridge-stderr] {}", now, &line);
                        let _ = f.flush();
                    }
                }
                if let Ok(mut lines) = log_lines_for_bridge.lock() {
                    lines.push(format!("[bridge] {line}"));
                    if lines.len() > 200 {
                        let drop_n = lines.len() - 200;
                        lines.drain(..drop_n);
                    }
                }
            };
            match Bridge::spawn(&self.bridge_exe_path, stderr_cb) {
                Ok(mut br) => {
                    // T09 (v0.9.0) round 4: PROTOCOL_VERSION は 2 にbump 済み。tester も
                    // 同じ定数を使い、Ready の version を比較する (= stale bridge / 古い
                    // tester の組み合わせを早期検出)。
                    if let Err(e) = br.send(&Cmd::Hello {
                        version: bridge::PROTOCOL_VERSION,
                    }) {
                        self.log(format!("bridge hello send failed: {e}"));
                        return;
                    }
                    match br.recv() {
                        Ok(Event::Ready { version }) => {
                            if version != bridge::PROTOCOL_VERSION {
                                self.log(format!(
                                    "bridge protocol version mismatch (bridge=v{version}, tester=v{})",
                                    bridge::PROTOCOL_VERSION
                                ));
                                return;
                            }
                            self.log(format!("bridge ready (protocol v{version})"));
                        }
                        Ok(Event::Error { detail }) => {
                            self.log(format!("bridge handshake error: {detail}"));
                            return;
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
                    let sample_rate = self.audio.as_ref().map(|a| a.sample_rate).unwrap_or(48_000);
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
                    self.log("bridge online — Show GUI / Through mode が利用可能になりました");
                    // bridge ハンドルが変わったので audio engine を再構築
                    self.restart_audio();
                }
                Err(e) => {
                    self.log(format!("bridge spawn failed: {e}"));
                }
            }
        } else {
            self.log("bridge は既に起動済みです (先に Unload してください)");
        }
    }

    #[cfg(windows)]
    fn show_gui(&mut self) {
        if self.bridge.is_none() {
            self.log("show_gui: no plugin loaded");
            return;
        }
        if self.gui_hwnd != 0 {
            self.log("show_gui: GUI already shown");
            return;
        }
        let bridge = self.bridge.as_ref().unwrap().clone();

        // ── ステップ 1: プラグインに推奨サイズを問い合わせる ──
        // attach の前に host HWND を正しいサイズで作るため。
        if let Err(e) = bridge.send(&Cmd::QueryGuiSize) {
            self.log(format!("send QueryGuiSize: {e}"));
            return;
        }
        let (pref_w, pref_h) = match bridge.recv() {
            Ok(Event::GuiSize { width, height }) => (width, height),
            Ok(Event::Error { detail }) => {
                self.log(format!("query_gui_size error: {detail}"));
                // フォールバック: 推奨サイズ取れなければ 800x500 で開く
                (800u32, 500u32)
            }
            Ok(other) => {
                self.log(format!("unexpected event after QueryGuiSize: {other:?}"));
                (800u32, 500u32)
            }
            Err(e) => {
                self.log(format!("recv after QueryGuiSize: {e}"));
                return;
            }
        };
        self.log(format!("plugin preferred size: {}x{}", pref_w, pref_h));

        // ── ステップ 2: 推奨サイズでホストウィンドウを作成 ──
        let title = self
            .last_loaded_name
            .as_deref()
            .unwrap_or("VST3 Plugin")
            .to_string();
        let reply = match self.gui_host.show(&title, pref_w, pref_h) {
            Ok(r) => r,
            Err(e) => {
                self.log(format!("create gui window: {e}"));
                return;
            }
        };
        if reply.hwnd_u64 == 0 {
            self.log("create gui window: HWND not returned");
            return;
        }
        self.log(format!(
            "host window: requested {}x{}, actual client {}x{} (dpi={})",
            pref_w, pref_h, reply.actual_client_w, reply.actual_client_h, reply.used_dpi
        ));
        self.gui_hwnd = reply.hwnd_u64;
        self.gui_close_signal = Some(reply.close_signal);
        self.gui_resize_signal = Some(reply.resize_signal);

        // ── ステップ 3: 正しいサイズの HWND で attach ──
        if let Err(e) = bridge.send(&Cmd::ShowGui {
            hwnd: self.gui_hwnd,
        }) {
            self.log(format!("send ShowGui: {e}"));
            self.close_gui();
            return;
        }
        match bridge.recv() {
            Ok(Event::GuiAttached { width, height }) => {
                self.log(format!("gui attached: {}x{}", width, height));
                // 想定外の差分があれば追従リサイズ
                if width > 0 && height > 0 && (width != pref_w || height != pref_h) {
                    plugin_gui::resize_window_client(self.gui_hwnd, width, height);
                }
            }
            Ok(Event::Error { detail }) => {
                self.log(format!("gui attach error: {detail}"));
                self.close_gui();
            }
            Ok(other) => {
                self.log(format!("unexpected event after ShowGui: {other:?}"));
            }
            Err(e) => {
                self.log(format!("recv after ShowGui: {e}"));
                self.close_gui();
            }
        }
    }

    #[cfg(windows)]
    fn close_gui(&mut self) {
        if self.gui_hwnd == 0 {
            return;
        }
        // bridge にデタッチ命令を先に送る (順序逆だと crash することがある)
        if let Some(br) = self.bridge.as_ref() {
            let _ = br.send(&Cmd::HideGui);
            // 応答 (gui_detached) は best-effort で待つが、blocking しない。
            // 短い timeout が無いので skip。
        }
        self.gui_host.close();
        self.gui_hwnd = 0;
        self.gui_close_signal = None;
        self.gui_resize_signal = None;
        self.log("gui closed");
    }

    /// ホストウィンドウのリサイズを polling して、来てたら bridge に通知する。
    /// プラグインの子ウィンドウサイズが追従する。
    #[cfg(windows)]
    fn poll_gui_resize(&mut self) {
        let Some(arc) = self.gui_resize_signal.as_ref() else {
            return;
        };
        // 連続した WM_SIZE は最後の 1 回だけ処理 (ドラッグ中に多発するため)
        let mut last: Option<(u32, u32)> = None;
        {
            let guard = arc.lock().unwrap();
            if let Some(rx) = guard.as_ref() {
                while let Ok(size) = rx.try_recv() {
                    last = Some(size);
                }
            }
        }
        if let Some((w, h)) = last {
            if let Some(br) = self.bridge.as_ref() {
                let _ = br.send(&Cmd::NotifyHostResize {
                    width: w,
                    height: h,
                });
            }
        }
    }

    /// GUI ウィンドウからの × クリックを polling して、来てたら close_gui する。
    #[cfg(windows)]
    fn poll_gui_close(&mut self) {
        let should_close = {
            let Some(arc) = self.gui_close_signal.as_ref() else {
                return;
            };
            let guard = arc.lock().unwrap();
            let Some(rx) = guard.as_ref() else {
                return;
            };
            matches!(rx.try_recv(), Ok(()))
        };
        if should_close {
            self.close_gui();
        }
    }

    fn unload(&mut self) {
        // GUI が出ていれば先に閉じる (bridge より先)
        #[cfg(windows)]
        self.close_gui();
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
        // ── graceful shutdown ──
        // ウィンドウ × クリック等で close 要求が出たら、bridge とプラグイン GUI を
        // 順番に破棄してから終了する。Rust の Drop だと宣言順の逆で gui_host が
        // bridge より先に落ちて、プラグイン attach 中にホスト HWND が消える
        // → bridge が応答待ちでハング、になりがち。
        if ctx.input(|i| i.viewport().close_requested()) {
            #[cfg(windows)]
            self.close_gui();
            self.unload();
            self.audio = None;
            // close_requested はそのまま伝播してウィンドウが閉じる
        }

        egui::TopBottomPanel::top("top").show(ctx, |ui| {
            ui.horizontal(|ui| {
                ui.heading("VST3 Host Tester");
                ui.separator();
                ui.label(format!("bridge exe: {}", self.bridge_exe_path.display()));
            });
            ui.horizontal(|ui| {
                ui.small(format!("ログ: {}", self.log_file_path.display()));
            });
        });

        egui::SidePanel::left("plugins")
            .resizable(true)
            .default_width(280.0)
            .show(ctx, |ui| {
                ui.heading("Plugins");
                ui.horizontal(|ui| {
                    if ui.button("Rescan").clicked() {
                        self.scan();
                    }
                    if ui
                        .button("Clear")
                        .on_hover_text("検索フィルタをクリア")
                        .clicked()
                    {
                        self.filter.clear();
                    }
                });
                // 検索ボックス。display_name に対する case-insensitive 部分一致。
                // path 側にもマッチさせる (ベンダー名がディレクトリ名に入っているケース対策)。
                ui.add(
                    egui::TextEdit::singleline(&mut self.filter)
                        .hint_text("検索 (名前・パス、部分一致)")
                        .desired_width(f32::INFINITY),
                );

                let needle = self.filter.trim().to_ascii_lowercase();
                let total = self.plugins.len();
                let matches: Vec<usize> = if needle.is_empty() {
                    (0..total).collect()
                } else {
                    self.plugins
                        .iter()
                        .enumerate()
                        .filter(|(_, p)| {
                            p.display_name.to_ascii_lowercase().contains(&needle)
                                || p.path
                                    .to_string_lossy()
                                    .to_ascii_lowercase()
                                    .contains(&needle)
                        })
                        .map(|(i, _)| i)
                        .collect()
                };
                ui.label(format!("{} / {} 件", matches.len(), total));
                ui.separator();

                let mut clicked: Option<usize> = None;
                egui::ScrollArea::vertical().show(ui, |ui| {
                    for &i in &matches {
                        let p = &self.plugins[i];
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
                #[cfg(windows)]
                {
                    let show_gui_enabled = self.bridge.is_some() && self.gui_hwnd == 0;
                    let resp = ui
                        .add_enabled(show_gui_enabled, egui::Button::new("Show GUI"))
                        .on_disabled_hover_text(format!(
                            "disabled — bridge: {}, gui_hwnd: 0x{:X}",
                            if self.bridge.is_some() {
                                "loaded"
                            } else {
                                "not loaded"
                            },
                            self.gui_hwnd
                        ));
                    if resp.clicked() {
                        self.show_gui();
                    }
                    let close_gui_enabled = self.gui_hwnd != 0;
                    if ui
                        .add_enabled(close_gui_enabled, egui::Button::new("Close GUI"))
                        .clicked()
                    {
                        self.close_gui();
                    }
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
                let prev_mode = mode_val;
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
                        mode_val = Mode::Through;
                    }
                });
                if prev_mode != mode_val {
                    self.log(format!("mode changed: {:?} -> {:?}", prev_mode, mode_val));
                }
            }

            ui.horizontal(|ui| {
                let was = self.bridge_passthrough;
                ui.checkbox(
                    &mut self.bridge_passthrough,
                    "Bridge passthrough (plugin スキップ、診断用)",
                );
                if was != self.bridge_passthrough {
                    if let Some(br) = self.bridge.as_ref() {
                        let _ = br.send(&Cmd::SetPassthrough {
                            enable: if self.bridge_passthrough { 1 } else { 0 },
                        });
                    }
                    self.log(format!("bridge passthrough: {}", self.bridge_passthrough));
                }
            });

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

        // GUI ウィンドウの × クリックを拾う
        #[cfg(windows)]
        self.poll_gui_close();
        // GUI ウィンドウのリサイズを bridge に転送
        #[cfg(windows)]
        self.poll_gui_resize();

        // audio 診断: cpal の実 callback サイズと underrun 回数を 1 秒ごとに log
        if let Some(audio) = self.audio.as_ref() {
            let now = std::time::Instant::now();
            if now.duration_since(self.last_audio_log_at).as_secs() >= 1 {
                use std::sync::atomic::Ordering;
                let frames = audio.actual_n_frames.load(Ordering::Relaxed);
                // min/max を読んでリセット
                let mn = audio.min_n_frames.swap(u32::MAX, Ordering::Relaxed);
                let mx = audio.max_n_frames.swap(0, Ordering::Relaxed);
                let total_under = audio.underruns.load(Ordering::Relaxed);
                let total_partial = audio.partial_pulls.load(Ordering::Relaxed);
                let delta_under = total_under.wrapping_sub(self.last_logged_underruns);
                let delta_partial = total_partial.wrapping_sub(self.last_logged_partials);
                let mn_print = if mn == u32::MAX { 0 } else { mn };
                self.log(format!(
                    "audio: cpal n_frames={}, min={}, max={} (block={}) underruns/s={} partial_pulls/s={}",
                    frames, mn_print, mx, audio.block_size, delta_under, delta_partial
                ));
                self.last_logged_frames = frames;
                self.last_logged_underruns = total_under;
                self.last_logged_partials = total_partial;
                self.last_audio_log_at = now;
            }
        }

        // bridge から非同期に来るイベント (latency_changed 等) のポーリングは
        // Phase 0b 後段で追加する。現状は load 時の同期 recv のみ。
        ctx.request_repaint_after(std::time::Duration::from_millis(100));
    }
}
