//! 音楽ビュー下段の 108-band spectrum アナライザ + ピッチ鍵盤描画 (Inc 4)。
//!
//! ラボ (`tools/music_lab`) の `draw_spectrum` / `draw_pitch_keyboard` + spectrum worker を
//! 本体へ移植したもの。設計はラボと同じ:
//!
//! - `music-core::SpectrumAnalyzer` (多解像度 FFT、書き換えず再利用) を **常駐ワーカースレッド**
//!   が所有し、UI から `MusicSpectrumRequest` を受けて `analyze_moving_window` を回す。
//! - 解析窓は再生位置周辺 **±1 秒** の PCM。この窓幅 (~96k サンプル) は cpal ring buffer
//!   (`src/video/audio.rs` の `AudioBuffer.processed`、約 100ms 分) では全く足りないため、
//!   ラボと同じく **展開済み全尺 PCM を playhead 周辺でスライス** する
//!   (`docs/music-integration-plan.md` 案A、§11 の「ring buffer tap の口」に決着)。
//! - PCM は解析ワーカー (`app.rs` の `run_music_analysis`) が全尺デコードした 48kHz interleaved
//!   stereo f32 を `Arc<MusicPcm>` で保持したもの。UI スレッドは `Arc` を渡すだけで、窓の
//!   切り出しはワーカー側で行う (ゼロコピー)。
//!
//! 描画 (`draw`) のピクセル/カラー計算はラボ実装を字面どおり移植している (ラボが機能の正本、
//! §2.1「再利用する」原則)。egui 依存があるため music-core には置けず本体側モジュールとして持つ。

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use music_core::{
    SPECTRUM_NOTE_MAX_MIDI, SPECTRUM_NOTE_MIN_MIDI, SpectrumAnalysis, SpectrumAnalyzer,
};

// ── レイアウト・解析定数 (ラボと同値) ──
/// スペクトラムのバンド数 (108 = 20Hz〜18kHz を約 1 半音幅で刻む)。
const SPECTRUM_BANDS: usize = 108;
/// 下段ピッチ鍵盤の高さ (px)。
const SPECTRUM_KEYBOARD_H: f32 = 34.0;
/// スペクトラムプロットと鍵盤の間の余白 (px)。
const SPECTRUM_PANEL_GAP: f32 = 8.0;
const SPECTRUM_TRAIL_DECAY: f32 = 0.994;
/// 解析窓の半径 (秒)。再生位置 ±この秒数を切り出して FFT に食わせる。
const SPECTRUM_SNAPSHOT_RADIUS_SECS: f64 = 1.0;
const KEY_HIGHLIGHT_DECAY: f32 = 0.925;
const KEY_HIGHLIGHT_MIN_PEAK: f32 = 0.035;
const KEY_SUSTAIN_ATTACK: f32 = 0.18;
const KEY_SUSTAIN_RELEASE: f32 = 0.965;
const KEYBOARD_DISPLAY_MIN_MIDI: u8 = 12; // C0
const KEYBOARD_DISPLAY_MAX_MIDI: u8 = 143; // B10, 18kHz 軸で右端はクリップされる。
const SPECTRUM_ANALYSIS_MIN_HZ: f32 = 20.0;
const SPECTRUM_AXIS_MIN_HZ: f32 = SPECTRUM_ANALYSIS_MIN_HZ;
const SPECTRUM_VIEW_MAX_HZ: f32 = 18_000.0;

/// スペクトラム更新のリクエスト間隔。再生中はフレーム毎に届くが、この間隔で throttle して
/// 過剰リクエストを抑える (1 フレーム 1 リクエストが上限)。
const SPECTRUM_REFRESH_INTERVAL: Duration = Duration::from_millis(16);

/// spectrum 用に常駐保持する PCM の上限サンプル数 (= 30 分 @ 48kHz stereo)。これを超える
/// 長尺ファイルは PCM を常駐させず spectrum を無効化する (決定的な固定上限。空きメモリ等の
/// 実行時状態には依存しない)。タイムライン解析自体は上限なしで動く。
pub const MUSIC_SPECTRUM_MAX_PCM_SAMPLES: usize = 48_000 * 2 * 60 * 30;

/// 音楽ビューの再生位置周辺スペクトラム用に、解析ワーカーが全尺デコードした PCM。
/// 48kHz interleaved stereo f32。再生エンジン (`VideoPlayer`) とは独立した、時刻でインデクス
/// できる並行コピー (再生バッファそのものではない)。
pub struct MusicPcm {
    /// interleaved stereo f32 サンプル (`[-1.0, 1.0]`)。
    pub samples: Vec<f32>,
    /// サンプルレート (Hz)。`audio_decode` の出力なので通常 48000。
    pub sample_rate: u32,
}

struct MusicSpectrumRequest {
    /// 全尺 PCM を `Arc` で共有 (UI スレッドは refcount +1 のみ、窓切り出しはワーカー側)。
    pcm: Arc<MusicPcm>,
    /// 解析中心 = 現在の再生位置 (秒)。
    center_secs: f64,
}

/// 音楽ビュー下段スペクトラムの状態 + 常駐ワーカーのハンドル。
///
/// `TimelineTextureCache` (`ui_music_timeline`) と同じく、ワーカー配線と描画状態を 1 つの
/// 構造体に閉じて `App` を薄く保つ。
pub struct MusicSpectrumState {
    tx: Option<mpsc::Sender<MusicSpectrumRequest>>,
    rx: Option<mpsc::Receiver<SpectrumAnalysis>>,
    cancel: Option<Arc<AtomicBool>>,
    /// 送信済みで結果待ちのリクエストが in-flight か (常に高々 1 件)。
    pending: bool,
    last_request: Option<Instant>,
    /// 直近リクエストした中心位置 (秒)。一時停止中は変化時のみ再リクエストする。
    last_center: f64,
    // ── 描画状態 (draw で毎フレーム減衰更新) ──
    bands: Vec<f32>,
    notes: Vec<f32>,
    trail: Vec<f32>,
    prev_bands: Vec<f32>,
    onsets: Vec<f32>,
    note_sustain: Vec<f32>,
    note_trail: Vec<f32>,
}

impl Default for MusicSpectrumState {
    fn default() -> Self {
        Self {
            tx: None,
            rx: None,
            cancel: None,
            pending: false,
            last_request: None,
            last_center: f64::NEG_INFINITY,
            bands: Vec::new(),
            notes: Vec::new(),
            trail: Vec::new(),
            prev_bands: Vec::new(),
            onsets: Vec::new(),
            note_sustain: Vec::new(),
            note_trail: Vec::new(),
        }
    }
}

impl Drop for MusicSpectrumState {
    fn drop(&mut self) {
        self.cancel_worker();
    }
}

impl MusicSpectrumState {
    /// 状態を丸ごと破棄してワーカーを止める。開くファイルが変わった / 音楽ビューを閉じたら呼ぶ。
    pub fn clear(&mut self) {
        self.cancel_worker();
        self.bands.clear();
        self.notes.clear();
        self.trail.clear();
        self.prev_bands.clear();
        self.onsets.clear();
        self.note_sustain.clear();
        self.note_trail.clear();
        self.pending = false;
        self.last_request = None;
        self.last_center = f64::NEG_INFINITY;
    }

    fn cancel_worker(&mut self) {
        if let Some(cancel) = self.cancel.take() {
            cancel.store(true, Ordering::Relaxed);
        }
        self.tx = None;
        self.rx = None;
        self.pending = false;
    }

    fn disconnect_worker(&mut self) {
        self.tx = None;
        self.rx = None;
        self.cancel = None;
        self.pending = false;
    }

    fn ensure_worker(&mut self) {
        if self.tx.is_some() {
            return;
        }
        let (request_tx, request_rx) = mpsc::channel::<MusicSpectrumRequest>();
        let (result_tx, result_rx) = mpsc::channel::<SpectrumAnalysis>();
        let cancel = Arc::new(AtomicBool::new(false));
        let worker_cancel = Arc::clone(&cancel);
        let spawned = std::thread::Builder::new()
            .name("miv-music-spectrum".into())
            .spawn(move || run_music_spectrum_worker(request_rx, result_tx, worker_cancel));
        if spawned.is_ok() {
            self.tx = Some(request_tx);
            self.rx = Some(result_rx);
            self.cancel = Some(cancel);
            self.pending = false;
            self.last_request = None;
        }
    }

    /// 1 フレーム分の更新: ワーカー結果を取り込み、必要なら新しいリクエストを送る。
    ///
    /// `pcm` が None (まだデコード中 / 上限超で spectrum 無効) の間は何もリクエストしない
    /// (描画は空バンド = 鍵盤ベースラインのみ)。再生中 or 結果待ちの間は軽い間隔で repaint を要求。
    pub fn update(
        &mut self,
        ctx: &egui::Context,
        pcm: Option<&Arc<MusicPcm>>,
        center_secs: f64,
        playing: bool,
    ) {
        self.poll();

        let Some(pcm) = pcm else {
            return;
        };
        self.ensure_worker();

        let due = self
            .last_request
            .is_none_or(|t| t.elapsed() >= SPECTRUM_REFRESH_INTERVAL);
        let center_changed = (self.last_center - center_secs).abs() > 1.0e-4;
        let want = self.bands.is_empty() || playing || center_changed;
        if want
            && !self.pending
            && due
            && let Some(tx) = self.tx.as_ref()
        {
            let request = MusicSpectrumRequest {
                pcm: Arc::clone(pcm),
                center_secs,
            };
            if tx.send(request).is_ok() {
                self.pending = true;
                self.last_request = Some(Instant::now());
                self.last_center = center_secs;
            } else {
                self.disconnect_worker();
            }
        }

        if playing || self.pending {
            ctx.request_repaint_after(SPECTRUM_REFRESH_INTERVAL);
        }
    }

    fn poll(&mut self) {
        let Some(rx) = self.rx.as_ref() else {
            return;
        };
        let mut latest = None;
        loop {
            match rx.try_recv() {
                Ok(analysis) => {
                    latest = Some(analysis);
                    self.pending = false;
                }
                Err(mpsc::TryRecvError::Empty) => break,
                Err(mpsc::TryRecvError::Disconnected) => {
                    self.disconnect_worker();
                    break;
                }
            }
        }
        if let Some(analysis) = latest {
            self.bands = analysis.bands;
            self.notes = analysis.notes;
        }
    }

    /// 下段スペクトラム + ピッチ鍵盤を `rect` に描く。ラボの `draw_spectrum` を本体向けに
    /// 移植したもの (描画状態 trail/onset/note は `self` に持つ)。
    pub fn draw(&mut self, ui: &egui::Ui, rect: egui::Rect) {
        let response = ui.interact(
            rect,
            ui.id().with("music_spectrum_panel"),
            egui::Sense::hover(),
        );
        let painter = ui.painter_at(rect);
        painter.rect_filled(rect, 0.0, egui::Color32::BLACK);

        let inner = rect.shrink2(egui::vec2(18.0, 12.0));
        let keyboard_rect = egui::Rect::from_min_max(
            egui::pos2(inner.left(), inner.bottom() - SPECTRUM_KEYBOARD_H),
            inner.right_bottom(),
        );
        let plot = egui::Rect::from_min_max(
            inner.min,
            egui::pos2(
                inner.right(),
                (keyboard_rect.top() - SPECTRUM_PANEL_GAP).max(inner.top()),
            ),
        );
        painter.rect_filled(plot, 0.0, egui::Color32::BLACK);
        painter.rect_stroke(
            plot,
            0.0,
            egui::Stroke::new(1.0, egui::Color32::from_rgba_unmultiplied(70, 92, 116, 130)),
            egui::StrokeKind::Inside,
        );
        painter.line_segment(
            [
                egui::pos2(plot.left(), plot.bottom() - 1.0),
                egui::pos2(plot.right(), plot.bottom() - 1.0),
            ],
            egui::Stroke::new(
                1.0,
                egui::Color32::from_rgba_unmultiplied(255, 255, 255, 35),
            ),
        );

        if self.bands.is_empty() {
            draw_pitch_keyboard(
                &painter,
                keyboard_rect,
                &self.notes,
                &mut self.note_sustain,
                &mut self.note_trail,
            );
            return;
        }
        let band_len = self.bands.len();
        if self.trail.len() != band_len {
            self.trail.clear();
            self.trail.resize(band_len, 0.0);
        }
        if self.prev_bands.len() != band_len {
            self.prev_bands.clear();
            self.prev_bands.extend_from_slice(&self.bands);
        }
        if self.onsets.len() != band_len {
            self.onsets.clear();
            self.onsets.resize(band_len, 0.0);
        }

        for i in 0..band_len {
            let value = self.bands[i].clamp(0.0, 1.0);
            let rise = (value - self.prev_bands[i]).max(0.0);
            self.onsets[i] = (self.onsets[i] * 0.86).max((rise * 2.8).clamp(0.0, 1.0));
            self.prev_bands[i] = self.prev_bands[i] * 0.25 + value * 0.75;
            self.trail[i] = (self.trail[i] * SPECTRUM_TRAIL_DECAY).max(value);
            let (band_low_hz, band_high_hz) = spectrum_band_hz_range(i, band_len);
            let x0 = (spectrum_axis_x(plot, band_low_hz) + 0.25).max(plot.left());
            let x1 = (spectrum_axis_x(plot, band_high_hz) - 0.25)
                .max(x0 + 0.75)
                .min(plot.right());
            let band_corner = if x1 - x0 < 2.0 { 0.0 } else { 1.0 };
            let ghost_h = (plot.height() - 3.0) * (self.trail[i] * 0.72).max(0.012);
            painter.rect_filled(
                egui::Rect::from_min_max(
                    egui::pos2((x0 - 0.45).max(plot.left()), plot.bottom() - 2.0 - ghost_h),
                    egui::pos2((x1 + 0.45).min(plot.right()), plot.bottom() - 2.0),
                ),
                band_corner,
                color_with_alpha(spectrum_color(i, band_len, self.trail[i]), 48),
            );
            let trail_h = (plot.height() - 3.0) * self.trail[i].max(0.015);
            painter.rect_filled(
                egui::Rect::from_min_max(
                    egui::pos2((x0 - 0.2).max(plot.left()), plot.bottom() - 2.0 - trail_h),
                    egui::pos2((x1 + 0.2).min(plot.right()), plot.bottom() - 2.0),
                ),
                band_corner,
                color_with_alpha(spectrum_color(i, band_len, self.trail[i]), 100),
            );
            let h = (plot.height() - 3.0) * value.max(0.015);
            painter.rect_filled(
                egui::Rect::from_min_max(
                    egui::pos2(x0, plot.bottom() - 2.0 - h),
                    egui::pos2(x1, plot.bottom() - 2.0),
                ),
                band_corner,
                spectrum_color(i, band_len, value),
            );
            let onset = self.onsets[i].clamp(0.0, 1.0);
            if onset > 0.025 {
                let accent = brighten_color(spectrum_color(i, band_len, value.max(onset)), 1.18);
                painter.rect_filled(
                    egui::Rect::from_min_max(
                        egui::pos2(x0, plot.bottom() - 2.0 - h),
                        egui::pos2(x1, plot.bottom() - 2.0),
                    ),
                    band_corner,
                    color_with_alpha(accent, (24.0 + onset * 68.0) as u8),
                );
            }
        }

        if let Some(pointer) = response
            .hover_pos()
            .filter(|pointer| plot.contains(*pointer) || keyboard_rect.contains(*pointer))
        {
            draw_spectrum_hover(&painter, plot, pointer);
        }
        draw_pitch_keyboard(
            &painter,
            keyboard_rect,
            &self.notes,
            &mut self.note_sustain,
            &mut self.note_trail,
        );
    }
}

fn run_music_spectrum_worker(
    request_rx: mpsc::Receiver<MusicSpectrumRequest>,
    result_tx: mpsc::Sender<SpectrumAnalysis>,
    cancel: Arc<AtomicBool>,
) {
    let mut analyzer = SpectrumAnalyzer::new(SPECTRUM_BANDS);
    while !cancel.load(Ordering::Relaxed) {
        let mut request = match request_rx.recv() {
            Ok(request) => request,
            Err(_) => break,
        };
        // 溜まったリクエストは最新だけ処理する (スクロール中の raster と同じ coalescing)。
        while let Ok(next) = request_rx.try_recv() {
            request = next;
        }
        if cancel.load(Ordering::Relaxed) {
            break;
        }
        let analysis = compute_spectrum(&mut analyzer, &request);
        if result_tx.send(analysis).is_err() {
            break;
        }
    }
}

fn compute_spectrum(
    analyzer: &mut SpectrumAnalyzer,
    request: &MusicSpectrumRequest,
) -> SpectrumAnalysis {
    let pcm = &request.pcm;
    let available_frames = pcm.samples.len() / 2;
    let center_frame =
        (request.center_secs.max(0.0) * pcm.sample_rate.max(1) as f64).round() as usize;
    match spectrum_window_range(
        available_frames,
        pcm.sample_rate,
        center_frame,
        SPECTRUM_SNAPSHOT_RADIUS_SECS,
    ) {
        Some((start_frame, end_frame, local_center)) => analyzer.analyze_moving_window(
            &pcm.samples[start_frame * 2..end_frame * 2],
            pcm.sample_rate,
            local_center,
        ),
        None => SpectrumAnalysis::default(),
    }
}

/// 再生位置周辺 ±`radius_secs` の PCM 窓範囲 `[start, end)` (フレーム単位) と、窓内での
/// 中心位置 (秒) を返す。ラボの `spectrum_request_from_samples` を「Vec を作らず範囲だけ返す」
/// 形にしたもの (ワーカーがこの範囲で全 PCM から部分スライスして FFT に渡す)。
fn spectrum_window_range(
    available_frames: usize,
    sample_rate: u32,
    center_frame: usize,
    radius_secs: f64,
) -> Option<(usize, usize, f64)> {
    if sample_rate == 0 || available_frames == 0 {
        return None;
    }
    let center_frame = center_frame.min(available_frames.saturating_sub(1));
    let radius_frames = (radius_secs.max(0.05) * sample_rate as f64)
        .round()
        .max(1.0) as usize;
    let start_frame = center_frame.saturating_sub(radius_frames);
    let end_frame = center_frame
        .saturating_add(radius_frames)
        .saturating_add(1)
        .min(available_frames);
    if end_frame <= start_frame {
        return None;
    }
    let local_center = (center_frame - start_frame) as f64 / sample_rate as f64;
    Some((start_frame, end_frame, local_center))
}

// ── 描画ヘルパー (ラボ移植) ──

fn draw_spectrum_hover(painter: &egui::Painter, plot: egui::Rect, pointer: egui::Pos2) {
    let x = pointer.x.clamp(plot.left(), plot.right());
    let hz = spectrum_axis_hz(plot, x);
    let label = format!("{:.1} Hz  {}", hz, note_label_for_hz(hz));
    painter.line_segment(
        [egui::pos2(x, plot.top()), egui::pos2(x, plot.bottom())],
        egui::Stroke::new(
            1.0,
            egui::Color32::from_rgba_unmultiplied(230, 240, 255, 150),
        ),
    );
    let label_w = (label.chars().count() as f32 * 7.2 + 16.0).min(plot.width());
    let label_h = 22.0;
    let label_x = if x + 10.0 + label_w <= plot.right() {
        x + 10.0
    } else {
        (x - 10.0 - label_w).max(plot.left() + 4.0)
    };
    let label_rect = egui::Rect::from_min_size(
        egui::pos2(label_x, plot.top() + 7.0),
        egui::vec2(label_w, label_h),
    );
    painter.rect_filled(
        label_rect,
        3.0,
        egui::Color32::from_rgba_unmultiplied(5, 8, 12, 224),
    );
    painter.rect_stroke(
        label_rect,
        3.0,
        egui::Stroke::new(
            1.0,
            egui::Color32::from_rgba_unmultiplied(120, 150, 176, 180),
        ),
        egui::StrokeKind::Inside,
    );
    painter.text(
        label_rect.center(),
        egui::Align2::CENTER_CENTER,
        label,
        egui::FontId::monospace(12.0),
        egui::Color32::from_rgb(238, 244, 250),
    );
}

fn draw_pitch_keyboard(
    painter: &egui::Painter,
    rect: egui::Rect,
    notes: &[f32],
    note_sustain: &mut Vec<f32>,
    note_trail: &mut Vec<f32>,
) {
    painter.rect_filled(rect, 0.0, egui::Color32::BLACK);
    let note_count = (SPECTRUM_NOTE_MAX_MIDI - SPECTRUM_NOTE_MIN_MIDI + 1) as usize;
    if note_sustain.len() != note_count {
        note_sustain.clear();
        note_sustain.resize(note_count, 0.0);
    }
    if note_trail.len() != note_count {
        note_trail.clear();
        note_trail.resize(note_count, 0.0);
    }
    let sustained_notes = update_keyboard_sustain(notes, note_sustain);
    let highlight_targets = keyboard_highlight_targets(&sustained_notes, note_count);
    for (trail, target) in note_trail.iter_mut().zip(highlight_targets) {
        *trail = (*trail * KEY_HIGHLIGHT_DECAY).max(target);
    }

    for c_midi in (KEYBOARD_DISPLAY_MIN_MIDI..=144_u8).step_by(12) {
        let x = spectrum_axis_x(rect, midi_to_hz(c_midi));
        if x > rect.left() && x < rect.right() {
            painter.line_segment(
                [egui::pos2(x, rect.top()), egui::pos2(x, rect.bottom())],
                egui::Stroke::new(
                    0.8,
                    egui::Color32::from_rgba_unmultiplied(120, 134, 148, 82),
                ),
            );
        }
    }

    for midi in KEYBOARD_DISPLAY_MIN_MIDI..=KEYBOARD_DISPLAY_MAX_MIDI {
        if is_black_key(midi) {
            continue;
        }
        let Some(key_rect) = conventional_key_rect(rect, midi, false) else {
            continue;
        };
        let real_key = (SPECTRUM_NOTE_MIN_MIDI..=SPECTRUM_NOTE_MAX_MIDI).contains(&midi);
        let value = if real_key {
            note_trail[(midi - SPECTRUM_NOTE_MIN_MIDI) as usize]
        } else {
            0.0
        };
        let base = if real_key {
            egui::Color32::from_rgb(208, 214, 219)
        } else {
            egui::Color32::from_rgb(58, 64, 70)
        };
        let active = key_color(midi, value);
        let fill = if real_key {
            lerp_color(base, active, (value * 0.94).clamp(0.0, 1.0))
        } else {
            base
        };
        painter.rect_filled(key_rect, 0.0, fill);
        painter.rect_stroke(
            key_rect,
            0.0,
            egui::Stroke::new(0.8, egui::Color32::from_rgba_unmultiplied(0, 0, 0, 155)),
            egui::StrokeKind::Inside,
        );
    }

    for midi in KEYBOARD_DISPLAY_MIN_MIDI..=KEYBOARD_DISPLAY_MAX_MIDI {
        if !is_black_key(midi) {
            continue;
        }
        let Some(key_rect) = conventional_key_rect(rect, midi, true) else {
            continue;
        };
        let real_key = (SPECTRUM_NOTE_MIN_MIDI..=SPECTRUM_NOTE_MAX_MIDI).contains(&midi);
        let value = if real_key {
            note_trail[(midi - SPECTRUM_NOTE_MIN_MIDI) as usize]
        } else {
            0.0
        };
        let base = if real_key {
            egui::Color32::from_rgb(13, 16, 19)
        } else {
            egui::Color32::from_rgb(34, 39, 44)
        };
        let active = key_color(midi, value);
        let fill = if real_key {
            lerp_color(base, active, (value * 0.96).clamp(0.0, 1.0))
        } else {
            base
        };
        painter.rect_filled(key_rect, 1.0, fill);
        painter.rect_stroke(
            key_rect,
            1.0,
            egui::Stroke::new(
                0.75,
                egui::Color32::from_rgba_unmultiplied(255, 255, 255, 80),
            ),
            egui::StrokeKind::Inside,
        );
    }
}

fn update_keyboard_sustain(notes: &[f32], sustain: &mut [f32]) -> Vec<f32> {
    let mut sustained = Vec::with_capacity(sustain.len());
    let raw_peak = (0..sustain.len())
        .map(|idx| notes.get(idx).copied().unwrap_or(0.0).clamp(0.0, 1.0))
        .fold(0.0_f32, f32::max);
    let broad_threshold = raw_peak * 0.42;
    let broad_count = if raw_peak > KEY_HIGHLIGHT_MIN_PEAK {
        (0..sustain.len())
            .filter(|idx| notes.get(*idx).copied().unwrap_or(0.0).clamp(0.0, 1.0) > broad_threshold)
            .count()
    } else {
        0
    };
    let broad_ratio = broad_count as f32 / sustain.len().max(1) as f32;
    let attack_scale = if broad_ratio > 0.34 { 0.35 } else { 1.0 };

    for (idx, slot) in sustain.iter_mut().enumerate() {
        let current = notes.get(idx).copied().unwrap_or(0.0).clamp(0.0, 1.0);
        if current >= *slot {
            let attack = KEY_SUSTAIN_ATTACK * attack_scale;
            *slot = *slot * (1.0 - attack) + current * attack;
        } else {
            *slot = (*slot * KEY_SUSTAIN_RELEASE).max(current);
        }
        sustained.push(*slot);
    }
    sustained
}

fn keyboard_highlight_targets(notes: &[f32], note_count: usize) -> Vec<f32> {
    let raw: Vec<f32> = (0..note_count)
        .map(|idx| notes.get(idx).copied().unwrap_or(0.0).clamp(0.0, 1.0))
        .collect();
    let peak = raw.iter().copied().fold(0.0_f32, f32::max);
    if peak < KEY_HIGHLIGHT_MIN_PEAK {
        return vec![0.0; note_count];
    }

    let mut prominence = vec![0.0; note_count];
    for idx in 0..note_count {
        let value = raw[idx];
        let near = local_shoulder(&raw, idx);
        let bed = local_spectral_bed(&raw, idx);
        let relative_loudness = (value / peak).clamp(0.0, 1.0);
        prominence[idx] = (value - near - bed * 0.18).max(0.0) * relative_loudness.powf(2.0);
    }

    let prominence_peak = prominence.iter().copied().fold(0.0_f32, f32::max);
    if prominence_peak <= 1.0e-6 || prominence_peak < peak * 0.12 {
        return vec![0.0; note_count];
    }
    let floor = (prominence_peak * 0.08).max(0.006);
    let sustained_presence = ((peak - 0.06) / 0.28).clamp(0.0, 1.0).powf(0.7);
    prominence
        .into_iter()
        .zip(raw)
        .map(|(value, raw_value)| {
            let contrast = ((value - floor) / (prominence_peak - floor).max(1.0e-6))
                .clamp(0.0, 1.0)
                .powf(1.35);
            let loudness = (raw_value / peak).clamp(0.0, 1.0).powf(0.20);
            contrast * loudness * sustained_presence
        })
        .collect()
}

fn local_shoulder(values: &[f32], idx: usize) -> f32 {
    let mut shoulder = 0.0_f32;
    for offset in [-2_isize, -1, 1, 2] {
        let Some(neighbor_idx) = idx.checked_add_signed(offset) else {
            continue;
        };
        let Some(value) = values.get(neighbor_idx) else {
            continue;
        };
        let weight = if offset.abs() == 1 { 0.72 } else { 0.52 };
        shoulder = shoulder.max(value * weight);
    }
    shoulder
}

fn local_spectral_bed(values: &[f32], idx: usize) -> f32 {
    let mut weighted_sum = 0.0;
    let mut weight_sum = 0.0;
    for offset in -6_isize..=6 {
        let distance = offset.abs();
        if distance <= 2 {
            continue;
        }
        let Some(neighbor_idx) = idx.checked_add_signed(offset) else {
            continue;
        };
        let Some(value) = values.get(neighbor_idx) else {
            continue;
        };
        let weight = 1.0 / distance as f32;
        weighted_sum += value * weight;
        weight_sum += weight;
    }
    if weight_sum > 0.0 {
        weighted_sum / weight_sum
    } else {
        0.0
    }
}

fn conventional_key_rect(rect: egui::Rect, midi: u8, black: bool) -> Option<egui::Rect> {
    let pc = midi % 12;
    let octave_c = midi - pc;
    let (x0, white_w) = conventional_octave_geometry(rect, octave_c)?;
    let (left, right, bottom) = if black {
        let center = match pc {
            1 => 1.0,
            3 => 2.0,
            6 => 4.0,
            8 => 5.0,
            10 => 6.0,
            _ => return None,
        };
        let w = white_w * 0.56;
        (
            x0 + white_w * center - w * 0.5,
            x0 + white_w * center + w * 0.5,
            rect.top() + rect.height() * 0.64,
        )
    } else {
        let index = match pc {
            0 => 0,
            2 => 1,
            4 => 2,
            5 => 3,
            7 => 4,
            9 => 5,
            11 => 6,
            _ => return None,
        } as f32;
        (
            x0 + white_w * index,
            x0 + white_w * (index + 1.0),
            rect.bottom(),
        )
    };
    let left = left.max(rect.left()) + 0.25;
    let right = right.min(rect.right()) - 0.25;
    if right <= left {
        return None;
    }
    Some(egui::Rect::from_min_max(
        egui::pos2(left, rect.top()),
        egui::pos2(right, bottom),
    ))
}

fn conventional_octave_geometry(rect: egui::Rect, octave_c: u8) -> Option<(f32, f32)> {
    let c_x = spectrum_axis_x(rect, midi_to_hz(octave_c));
    let next_c_x = spectrum_axis_x(rect, midi_to_hz(octave_c.saturating_add(12)));
    let octave_w = next_c_x - c_x;
    if octave_w <= 1.0 {
        return None;
    }
    let white_w = octave_w / 7.0;
    Some((c_x - white_w * 0.5, white_w))
}

fn spectrum_axis_x(rect: egui::Rect, hz: f32) -> f32 {
    let min = SPECTRUM_AXIS_MIN_HZ;
    let max = SPECTRUM_VIEW_MAX_HZ;
    let t = (hz.clamp(min, max).log2() - min.log2()) / (max.log2() - min.log2());
    rect.left() + t.clamp(0.0, 1.0) * rect.width()
}

fn spectrum_axis_hz(rect: egui::Rect, x: f32) -> f32 {
    let min = SPECTRUM_AXIS_MIN_HZ;
    let max = SPECTRUM_VIEW_MAX_HZ;
    let t = ((x - rect.left()) / rect.width().max(1.0)).clamp(0.0, 1.0);
    2.0_f32.powf(min.log2() + t * (max.log2() - min.log2()))
}

fn spectrum_band_hz(index: usize, total: usize) -> f32 {
    if total <= 1 {
        return SPECTRUM_ANALYSIS_MIN_HZ;
    }
    let ratio = SPECTRUM_VIEW_MAX_HZ / SPECTRUM_ANALYSIS_MIN_HZ;
    let t = index as f32 / (total - 1) as f32;
    SPECTRUM_ANALYSIS_MIN_HZ * ratio.powf(t)
}

fn spectrum_band_hz_range(index: usize, total: usize) -> (f32, f32) {
    if total <= 1 {
        let half = 2.0_f32.powf(1.0 / 24.0);
        return (
            SPECTRUM_ANALYSIS_MIN_HZ / half,
            SPECTRUM_ANALYSIS_MIN_HZ * half,
        );
    }
    let center = spectrum_band_hz(index, total);
    let ratio = SPECTRUM_VIEW_MAX_HZ / SPECTRUM_ANALYSIS_MIN_HZ;
    let step = ratio.powf(1.0 / (total - 1) as f32);
    let edge_scale = step.sqrt();
    (center / edge_scale, center * edge_scale)
}

fn midi_to_hz(midi: u8) -> f32 {
    440.0 * 2.0_f32.powf((midi as f32 - 69.0) / 12.0)
}

fn note_label_for_hz(hz: f32) -> String {
    if !hz.is_finite() || hz <= 0.0 {
        return "--".to_string();
    }
    let midi_exact = 69.0 + 12.0 * (hz / 440.0).log2();
    let nearest = midi_exact.round() as i32;
    let cents = ((midi_exact - nearest as f32) * 100.0).round() as i32;
    let name = note_name_from_midi(nearest);
    if cents == 0 {
        name
    } else if cents > 0 {
        format!("{name} +{cents}c")
    } else {
        format!("{name} {cents}c")
    }
}

fn note_name_from_midi(midi: i32) -> String {
    const NAMES: [&str; 12] = [
        "C", "C#", "D", "D#", "E", "F", "F#", "G", "G#", "A", "A#", "B",
    ];
    let pitch_class = midi.rem_euclid(12) as usize;
    let octave = midi.div_euclid(12) - 1;
    format!("{}{}", NAMES[pitch_class], octave)
}

fn is_black_key(midi: u8) -> bool {
    matches!(midi % 12, 1 | 3 | 6 | 8 | 10)
}

fn key_color(midi: u8, value: f32) -> egui::Color32 {
    let pitch_class = midi % 12;
    let fifth_index = ((pitch_class as usize * 7) % 12) as f32;
    let hue = (fifth_index / 12.0 + 0.57) % 1.0;
    let saturation = 0.62 + value.clamp(0.0, 1.0) * 0.28;
    let brightness = 0.38 + value.clamp(0.0, 1.0) * 0.58;
    hsv_to_rgb(hue, saturation, brightness)
}

fn hsv_to_rgb(h: f32, s: f32, v: f32) -> egui::Color32 {
    let h = h.rem_euclid(1.0) * 6.0;
    let i = h.floor() as i32;
    let f = h - i as f32;
    let p = v * (1.0 - s);
    let q = v * (1.0 - s * f);
    let t = v * (1.0 - s * (1.0 - f));
    let (r, g, b) = match i.rem_euclid(6) {
        0 => (v, t, p),
        1 => (q, v, p),
        2 => (p, v, t),
        3 => (p, q, v),
        4 => (t, p, v),
        _ => (v, p, q),
    };
    egui::Color32::from_rgb(
        (r.clamp(0.0, 1.0) * 255.0) as u8,
        (g.clamp(0.0, 1.0) * 255.0) as u8,
        (b.clamp(0.0, 1.0) * 255.0) as u8,
    )
}

fn spectrum_color(index: usize, total: usize, value: f32) -> egui::Color32 {
    let t = if total <= 1 {
        0.0
    } else {
        index as f32 / (total - 1) as f32
    };
    let base = if t < 0.20 {
        lerp_color(
            egui::Color32::from_rgb(188, 58, 34),
            egui::Color32::from_rgb(236, 132, 34),
            t / 0.20,
        )
    } else if t < 0.52 {
        lerp_color(
            egui::Color32::from_rgb(255, 216, 28),
            egui::Color32::from_rgb(70, 232, 76),
            (t - 0.20) / 0.32,
        )
    } else if t < 0.78 {
        lerp_color(
            egui::Color32::from_rgb(38, 230, 190),
            egui::Color32::from_rgb(44, 138, 255),
            (t - 0.52) / 0.26,
        )
    } else {
        lerp_color(
            egui::Color32::from_rgb(44, 138, 255),
            egui::Color32::from_rgb(245, 82, 210),
            (t - 0.78) / 0.22,
        )
    };
    let alpha = (92.0 + 150.0 * value.clamp(0.0, 1.0)) as u8;
    color_with_alpha(
        brighten_color(base, 0.55 + value.clamp(0.0, 1.0) * 0.65),
        alpha,
    )
}

fn lerp_color(a: egui::Color32, b: egui::Color32, t: f32) -> egui::Color32 {
    let t = t.clamp(0.0, 1.0);
    let lerp = |av: u8, bv: u8| av as f32 + (bv as f32 - av as f32) * t;
    egui::Color32::from_rgb(
        lerp(a.r(), b.r()) as u8,
        lerp(a.g(), b.g()) as u8,
        lerp(a.b(), b.b()) as u8,
    )
}

fn brighten_color(color: egui::Color32, scale: f32) -> egui::Color32 {
    egui::Color32::from_rgb(
        ((color.r() as f32 * scale).min(255.0)) as u8,
        ((color.g() as f32 * scale).min(255.0)) as u8,
        ((color.b() as f32 * scale).min(255.0)) as u8,
    )
}

fn color_with_alpha(color: egui::Color32, alpha: u8) -> egui::Color32 {
    egui::Color32::from_rgba_unmultiplied(color.r(), color.g(), color.b(), alpha)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn window_range_centers_within_streaming_samples() {
        // 10 frames available, center at frame 6, radius 0.2s @ 10Hz → radius 2 frames.
        let (start, end, center) = spectrum_window_range(10, 10, 6, 0.2).unwrap();
        assert_eq!(start, 4);
        assert_eq!(end, 9);
        assert!((center - 0.2).abs() < 1.0e-9);
    }

    #[test]
    fn window_range_clamps_to_available_frames() {
        // center beyond available → clamps to last frame (index 9).
        let (start, end, center) = spectrum_window_range(10, 10, 99, 1.0).unwrap();
        assert_eq!(start, 0);
        assert_eq!(end, 10);
        // radius 0.05s floor → 1 frame min, but 1.0s * 10 = 10 frames radius, clamped by start=0.
        // center frame 9 - start 0 = 9 frames / 10Hz = 0.9s.
        assert!((center - 0.9).abs() < 1.0e-9);
    }

    #[test]
    fn window_range_rejects_empty() {
        assert!(spectrum_window_range(0, 48_000, 0, 1.0).is_none());
        assert!(spectrum_window_range(100, 0, 0, 1.0).is_none());
    }

    #[test]
    fn band_hz_range_is_monotonic_and_spans_audio() {
        let (lo0, hi0) = spectrum_band_hz_range(0, SPECTRUM_BANDS);
        let (lo_last, hi_last) = spectrum_band_hz_range(SPECTRUM_BANDS - 1, SPECTRUM_BANDS);
        assert!(lo0 < hi0);
        assert!(lo_last < hi_last);
        // 低域は 20Hz 付近、高域は 18kHz 付近。
        assert!(lo0 < 25.0);
        assert!(hi_last > 15_000.0);
        // バンド中心は単調増加。
        let mut prev = 0.0;
        for i in 0..SPECTRUM_BANDS {
            let center = spectrum_band_hz(i, SPECTRUM_BANDS);
            assert!(center > prev);
            prev = center;
        }
    }

    #[test]
    fn note_label_names_reference_pitches() {
        assert_eq!(note_label_for_hz(440.0), "A4");
        assert_eq!(note_label_for_hz(261.6256), "C4");
        assert_eq!(note_label_for_hz(0.0), "--");
    }

    #[test]
    fn keyboard_highlight_silence_is_flat() {
        let note_count = (SPECTRUM_NOTE_MAX_MIDI - SPECTRUM_NOTE_MIN_MIDI + 1) as usize;
        let targets = keyboard_highlight_targets(&vec![0.0; note_count], note_count);
        assert_eq!(targets.len(), note_count);
        assert!(targets.iter().all(|v| *v == 0.0));
    }

    #[test]
    fn black_key_classification_matches_semitones() {
        // C C# D D# E F F# G G# A A# B
        let expected = [
            false, true, false, true, false, false, true, false, true, false, true, false,
        ];
        for (pc, want) in expected.iter().enumerate() {
            assert_eq!(is_black_key(60 + pc as u8), *want, "pc={pc}");
        }
    }
}
