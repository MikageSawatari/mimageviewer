//! 音楽ビュー下段の MIDI-semitone spectrum アナライザ + ピッチ鍵盤描画 (Inc 4)。
//!
//! ラボ (`tools/music_lab`) の `draw_spectrum` / `draw_pitch_keyboard` + spectrum worker を
//! 本体へ移植したもの。設計はラボと同じ:
//!
//! - `music-core::SpectrumAnalyzer` (多解像度 FFT、書き換えず再利用) を **常駐ワーカースレッド**
//!   が所有し、UI から `MusicSpectrumRequest` を受けて `analyze_moving_window` を回す。
//! - 解析窓は再生位置周辺 **±1 秒** の PCM。この窓幅 (~96k サンプル) は cpal ring buffer
//!   (`src/video/audio.rs` の `AudioBuffer.processed`、約 100ms 分) では全く足りないため、
//!   ラボと同じく **展開済み PCM を playhead 周辺でスライス** する
//!   (`docs/music-integration-plan.md` 案A、§11 の「ring buffer tap の口」に決着)。
//! - PCM は解析ワーカー (`app.rs` の `run_music_analysis`) がデコードした 48kHz interleaved
//!   stereo f32 を `Arc<MusicPcm>` で保持したもの。**progressive**: `MusicPcm` は追記式共有バッファ
//!   (`RwLock<Vec<f32>>`) で、ワーカーがデコード開始前に空の `Arc` を UI へ渡し、以降差分を末尾
//!   `append` する。UI スレッドは `Arc` を渡すだけで、窓の切り出し (`copy_window`) はワーカー側で
//!   行う。これで全尺デコード完了を待たず、再生位置がデコード済み範囲にあれば下段グラフが出る
//!   (§5.6、長尺ファイルの出現遅延対策)。`RwLock` は解析 read と窓コピー read を並行させ spectrum を
//!   固まらせないため (詳細は `MusicPcm` の doc)。
//!
//! 鍵盤の輝度・低域整理・配置・配色と Bass ▼ の判断、スペクトラムの配色は `music-core` にあり、
//! 音楽ビデオ書き出しと共有する。このモジュールには、その結果を egui の描画 API へ渡す処理だけを置く。

use crate::music_note_color::{MusicNoteColorScheme, note_color};

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, PoisonError, RwLock, RwLockReadGuard, RwLockWriteGuard};
use std::time::{Duration, Instant};

use music_core::bass::{BassEstimate, pitch_frame_at_secs, select_bass};
#[cfg(test)]
use music_core::pitch::HARMNET_HOP_SECS;
use music_core::pitch::{
    HARMNET_HOP_SAMPLES, HARMNET_LONG_WINDOW_SAMPLES, HARMNET_PITCH_COUNT, HarmNetTracker,
};
use music_core::{
    BassMarkerPlacement, KeyboardAnimationState, KeyboardBassInput, KeyboardCenterInput,
    KeyboardFrame, KeyboardLayoutCache, KeyboardRect, MivSpectrumState, SPECTRUM_BAND_COUNT,
    SPECTRUM_NOTE_MIN_MIDI, SpectrumAnalysis, SpectrumAnalyzer, SpectrumVisualRect,
    VisualizationColor, brighten_color, key_fill_with_color, layout_bass_markers_with_input_into,
    layout_keyboard_cached, prepare_keyboard_frame_with_input_into, spectrum_axis_hz,
    spectrum_color, unlit_key_base, visit_miv_spectrum_primitives, with_alpha,
};
#[cfg(test)]
use music_core::{SPECTRUM_BAND_MIN_MIDI, spectrum_band_hz, spectrum_band_hz_range};

// ── レイアウト・解析定数 (ラボと同値) ──
/// スペクトラムのバンド数 (E0-C#10、MIDI 半音ごと)。
const SPECTRUM_BANDS: usize = SPECTRUM_BAND_COUNT;
/// 下段ピッチ鍵盤の高さ (px)。
const SPECTRUM_KEYBOARD_H: f32 = 34.0;
/// スペクトラムプロットと鍵盤の間の余白 (px)。
const SPECTRUM_PANEL_GAP: f32 = 8.0;
/// 解析窓の半径 (秒)。再生位置 ±この秒数を切り出して FFT に食わせる。
const SPECTRUM_SNAPSHOT_RADIUS_SECS: f64 = 1.0;
const BASS_MARKER_ALPHA_MIN: f32 = 80.0;
const BASS_MARKER_ALPHA_MAX: f32 = 255.0;

/// スペクトラム更新のリクエスト間隔。再生中はフレーム毎に届くが、この間隔で throttle して
/// 過剰リクエストを抑える (1 フレーム 1 リクエストが上限)。
const SPECTRUM_REFRESH_INTERVAL: Duration = Duration::from_millis(16);
const PITCH_LOOKAHEAD_FRAMES: i64 = 8;
const WARMUP_FRAMES: i64 = 58;
const MAX_ADVANCE_FRAMES: i64 = WARMUP_FRAMES;

/// 音楽ビューの再生位置周辺スペクトラム用に、解析ワーカーがデコードした PCM を保持する
/// 共有バッファ。48kHz interleaved stereo f32。再生エンジン (`VideoPlayer`) とは独立した、
/// 時刻でインデクスできる並行コピー (再生バッファそのものではない)。
///
/// **progressive**: 解析ワーカーはデコード開始前にこの (空の) バッファを `Arc` で UI へ渡し、
/// 以降デコード差分を末尾 `append` する。spectrum worker は再生位置 ±1 秒窓だけを読み取ってコピー
/// し FFT に渡すので、全尺デコード完了を待たず、再生位置がデコード済み範囲にあれば下段グラフを描ける
/// (docs/music-integration-plan.md §5.6)。`Vec` を丸ごと clone しないので常駐は 1 曲分 1×。
///
/// `samples` を **`RwLock`** で包む理由: timeline の partial/最終確定解析 (`with_prefix` が
/// `analyze_stereo_timeline` を回す = **長い読み取り**) と、spectrum worker の窓コピー
/// (`copy_window` = **短い読み取り**) は共に read なので **同時に走れる**。`Mutex` だと解析中に
/// spectrum が固まる (実機 FB 2026-07-07) が、`RwLock` なら長い解析 read と並行して spectrum read が
/// 進むので固まらない。追記 (`append` = write) だけが短時間 read を排他する (追記は解析と同じデコード
/// スレッド上で逐次実行なので、解析中に write は来ない)。UI スレッドは `is_complete` の atomic 読みと
/// `Arc` clone しかせず lock を取らない。
pub struct MusicPcm {
    /// interleaved stereo f32 サンプル (`[-1.0, 1.0]`)。デコード進行に合わせて末尾追記される。
    samples: RwLock<Vec<f32>>,
    /// サンプルレート (Hz)。`audio_decode` の出力なので通常 48000。デコード中は不変。
    pub sample_rate: u32,
    /// デコード完了フラグ (true = これ以上 `append` されない)。ローディング表示の抑制に使う。
    complete: AtomicBool,
}

impl MusicPcm {
    /// 空の共有 PCM を作る。`reserve_frames` (フレーム数) 分だけ容量を **best-effort で**先取りし、
    /// 追記中の再確保 (長尺では数百 MB 級 memcpy → latency スパイク) を避ける。先取りは `try_reserve`
    /// なので、巨大 / bogus な reserve でも abort せず、失敗時は空容量で始める (`append` が incremental
    /// に `try_reserve` して確保失敗を Err で返す)。
    pub fn with_capacity(sample_rate: u32, reserve_frames: usize) -> Self {
        let mut samples: Vec<f32> = Vec::new();
        let _ = samples.try_reserve(reserve_frames.saturating_mul(2));
        Self {
            samples: RwLock::new(samples),
            sample_rate,
            complete: AtomicBool::new(false),
        }
    }

    /// 中毒 (poison) を無視して read/write ロックする。窓コピー / 追記 / プレフィックス解析の
    /// いずれも `Vec` を壊さないので、稀な panic 後も残データで続行してよい。
    fn read_samples(&self) -> RwLockReadGuard<'_, Vec<f32>> {
        self.samples.read().unwrap_or_else(PoisonError::into_inner)
    }
    fn write_samples(&self) -> RwLockWriteGuard<'_, Vec<f32>> {
        self.samples.write().unwrap_or_else(PoisonError::into_inner)
    }

    /// デコード済みサンプル (interleaved stereo f32) を末尾追記する (解析ワーカー専用、write ロック)。
    /// 長尺で GB 級に膨らんでも abort させないよう `try_reserve` で確保し、失敗時は `TryReserveError`
    /// を返す (呼び出し側がデコードを打ち切る。旧 `append_resampled` の try_reserve 方針を共有バッファ
    /// でも維持)。
    pub fn append(&self, delta: &[f32]) -> Result<(), std::collections::TryReserveError> {
        if delta.is_empty() {
            return Ok(());
        }
        let mut guard = self.write_samples();
        guard.try_reserve(delta.len())?;
        guard.extend_from_slice(delta);
        Ok(())
    }

    /// これ以上追記しないことを表明する (デコード完了 / 打ち切り時)。
    pub fn mark_complete(&self) {
        self.complete.store(true, Ordering::Release);
    }

    /// デコードが完了しているか。
    pub fn is_complete(&self) -> bool {
        self.complete.load(Ordering::Acquire)
    }

    /// これまでにデコード済みのフレーム数 (= interleaved サンプル数 / 2)。partial のマイルストーン
    /// 判定に使う (read ロック)。
    pub fn decoded_frames(&self) -> usize {
        self.read_samples().len() / 2
    }

    /// 現在のデコード済みプレフィックス全体を **read ロック**下で `f` に渡す。timeline の progressive
    /// 先出し / 最終確定解析で使う。`f` (= `analyze_stereo_timeline`) の実行中も read ロックなので、
    /// spectrum worker の窓コピー (同じく read) は **並行して進める** (固まらない)。追記 (write) だけは
    /// この read が終わるまで待つが、追記は同じデコードスレッド上で逐次実行なので競合しない。
    pub fn with_prefix<R>(&self, f: impl FnOnce(&[f32], u32) -> R) -> R {
        let guard = self.read_samples();
        f(&guard, self.sample_rate)
    }

    /// 再生位置 `center_frame` ±`radius_secs` の窓を **read ロック**下でコピーして返す (spectrum
    /// worker 用)。デコード済み範囲に窓が取れなければ `None` (= まだ描けない)。窓は最大でも ~96k
    /// フレーム (2 秒幅) なのでコピーは 1MB 未満、read ロック保持は sub-ms。read なので `with_prefix`
    /// の長い解析 read とも並行して進める (固まらない)。
    ///
    /// progressive 特有: デコード**未完了**で、再生位置が窓半径を超えてデコード済み先端より先を指す
    /// (= 窓がデコード済み領域と全く重ならない。seek forward で未デコード領域へ飛んだ等) 場合は、
    /// `spectrum_window_range` の末尾クランプによる **stale な末尾窓**を返さず `None` にして「解析中」
    /// を維持する。窓が一部でもデコード済み領域に重なる (再生が先端に追いつきかけ) 場合や、デコード
    /// 完了後は従来どおりクランプに委ねる (末尾 ±1s の spectrum は妥当)。
    fn copy_window(&self, center_frame: usize, radius_secs: f64) -> Option<(Vec<f32>, f64)> {
        let guard = self.read_samples();
        let available_frames = guard.len() / 2;
        if !self.is_complete() {
            // spectrum_window_range と同じ半径換算で「重なりなし」を判定する。
            let radius_frames = (radius_secs.max(0.05) * self.sample_rate.max(1) as f64)
                .round()
                .max(1.0) as usize;
            if center_frame.saturating_sub(radius_frames) >= available_frames {
                return None;
            }
        }
        let (start_frame, end_frame, local_center) = spectrum_window_range(
            available_frames,
            self.sample_rate,
            center_frame,
            radius_secs,
        )?;
        Some((guard[start_frame * 2..end_frame * 2].to_vec(), local_center))
    }

    /// Copy interleaved stereo [start, end), padding before zero and after confirmed EOF.
    /// During decoding only the available prefix is returned; its length identifies the frontier.
    /// This short read is used only by the spectrum worker, never the UI thread.
    fn copy_range(&self, start_frame: i64, end_frame: i64) -> Vec<f32> {
        let guard = self.read_samples();
        let available = (guard.len() / 2) as i64;
        let end = if self.is_complete() {
            end_frame
        } else {
            end_frame.min(available)
        };
        if end <= start_frame {
            return Vec::new();
        }
        let mut samples = vec![0.0; (end - start_frame) as usize * 2];
        let copy_start = start_frame.max(0);
        let copy_end = end.min(available);
        if copy_start < copy_end {
            let offset = (copy_start - start_frame) as usize * 2;
            let count = (copy_end - copy_start) as usize * 2;
            samples[offset..offset + count]
                .copy_from_slice(&guard[copy_start as usize * 2..copy_end as usize * 2]);
        }
        samples
    }
}

struct MusicSpectrumRequest {
    /// 全尺 PCM を `Arc` で共有 (UI スレッドは refcount +1 のみ、窓切り出しはワーカー側)。
    pcm: Arc<MusicPcm>,
    /// 解析中心 = 現在の再生位置 (秒)。
    center_secs: f64,
}

struct MusicSpectrumResult {
    center_secs: f64,
    analysis: SpectrumAnalysis,
}

/// 音楽ビュー下段スペクトラムの状態 + 常駐ワーカーのハンドル。
///
/// `TimelineTextureCache` (`ui_music_timeline`) と同じく、ワーカー配線と描画状態を 1 つの
/// 構造体に閉じて `App` を薄く保つ。
pub struct MusicSpectrumState {
    tx: Option<mpsc::Sender<MusicSpectrumRequest>>,
    rx: Option<mpsc::Receiver<MusicSpectrumResult>>,
    cancel: Option<Arc<AtomicBool>>,
    /// 送信済みで結果待ちのリクエストが in-flight か (常に高々 1 件)。
    pending: bool,
    last_request: Option<Instant>,
    /// 直近リクエストした中心位置 (秒)。一時停止中は変化時のみ再リクエストする。
    last_center: f64,
    // ── 描画状態 (draw で毎フレーム減衰更新) ──
    bands: Vec<f32>,
    notes: Vec<f32>,
    pitch_probabilities: Option<Vec<f32>>,
    note_min_midi: u8,
    spectrum_visual: MivSpectrumState,
    keyboard: KeyboardAnimationState,
    keyboard_frame: KeyboardFrame,
    keyboard_layout: KeyboardLayoutCache,
    bass_markers: Vec<BassMarkerPlacement>,
    /// 直近 `update` で PCM (`Arc<MusicPcm>`) が渡っていたか。false = まだ解析ワーカー起動待ち。
    source_present: bool,
    /// 直近 `update` で PCM のデコードが完了していたか。バンドが空でこれが false の間は
    /// 「解析中」表示を出す (progressive で下段グラフが出るまで無反応に見せない)。
    source_complete: bool,
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
            pitch_probabilities: None,
            note_min_midi: SPECTRUM_NOTE_MIN_MIDI,
            spectrum_visual: MivSpectrumState::default(),
            keyboard: KeyboardAnimationState::default(),
            keyboard_frame: KeyboardFrame::default(),
            keyboard_layout: KeyboardLayoutCache::default(),
            bass_markers: Vec::new(),
            source_present: false,
            source_complete: false,
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
        self.pitch_probabilities = None;
        self.note_min_midi = SPECTRUM_NOTE_MIN_MIDI;
        self.spectrum_visual.clear();
        self.keyboard.clear();
        self.keyboard_layout.clear();
        self.bass_markers.clear();
        self.pending = false;
        self.last_request = None;
        self.last_center = f64::NEG_INFINITY;
        self.source_present = false;
        self.source_complete = false;
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
        let (result_tx, result_rx) = mpsc::channel::<MusicSpectrumResult>();
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
    /// `pcm` が None (まだ解析ワーカー起動待ち) の間は何もリクエストしない。progressive では
    /// `pcm` はデコード開始前から Some (中身は追記中) なので、再生位置がデコード済み範囲に入り
    /// 次第 spectrum worker が窓を返す。まだ窓が取れない (未デコード領域) 間はバンド空 = 鍵盤
    /// ベースライン + 「解析中」表示。再生中 or 結果待ちの間は軽い間隔で repaint を要求
    /// (デコード進行中の repaint は `poll_music_analysis` の 50ms tick が駆動する)。
    pub fn update(
        &mut self,
        ctx: &egui::Context,
        pcm: Option<&Arc<MusicPcm>>,
        center_secs: f64,
        playing: bool,
    ) {
        self.poll();

        self.source_present = pcm.is_some();
        self.source_complete = pcm.is_some_and(|p| p.is_complete());

        let Some(pcm) = pcm else {
            return;
        };
        self.ensure_worker();

        let due = self
            .last_request
            .is_none_or(|t| t.elapsed() >= SPECTRUM_REFRESH_INTERVAL);
        let center_changed = (self.last_center - center_secs).abs() > 1.0e-4;
        let pitch_missing = self.pitch_probabilities.is_none();
        let want = self.bands.is_empty() || playing || center_changed || pitch_missing;
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

        if playing || self.pending || pitch_missing {
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
                Ok(result) => {
                    latest = Some(result);
                    self.pending = false;
                }
                Err(mpsc::TryRecvError::Empty) => break,
                Err(mpsc::TryRecvError::Disconnected) => {
                    self.disconnect_worker();
                    break;
                }
            }
        }
        if let Some(result) = latest {
            let analysis = result.analysis;
            self.spectrum_visual
                .adopt_analysis_at(&analysis.bands, result.center_secs);
            self.note_min_midi = analysis.note_min_midi;
            self.bands = analysis.bands;
            self.notes = analysis.notes;
            self.pitch_probabilities = analysis.pitch_probabilities;
        }
    }

    /// 下段スペクトラム + ピッチ鍵盤を `rect` に描く。ラボの `draw_spectrum` を本体向けに
    /// 移植したもの (描画状態 trail/onset/note は `self` に持つ)。
    pub fn draw(
        &mut self,
        ui: &egui::Ui,
        rect: egui::Rect,
        norm_gain_db: f32,
        scheme: MusicNoteColorScheme,
    ) {
        let response = ui.interact(
            rect,
            ui.scope_id().with("music_spectrum_panel"),
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
                self.note_min_midi,
                self.pitch_probabilities.as_deref(),
                &mut self.keyboard,
                &mut self.keyboard_frame,
                &mut self.keyboard_layout,
                norm_gain_db,
                scheme,
            );
            // まだデコード中 (窓が取れない) 間はプロット領域に「解析中」を出し、無反応に見せない。
            // デコード完了後にバンドが空 = 無音区間なので、その場合はラベルを出さない。
            draw_bass_marker(
                &painter,
                keyboard_rect,
                self.note_min_midi,
                &mut self.keyboard,
                &self.keyboard_frame,
                &mut self.keyboard_layout,
                &mut self.bass_markers,
                select_bass(self.pitch_probabilities.as_deref().unwrap_or(&[])),
                scheme,
            );
            if self.source_present && !self.source_complete {
                painter.text(
                    plot.center(),
                    egui::Align2::CENTER_CENTER,
                    "スペクトラム解析中…",
                    egui::FontId::proportional(13.0),
                    egui::Color32::from_rgba_unmultiplied(180, 200, 220, 170),
                );
            }
            return;
        }
        let visual = self.spectrum_visual.frame();
        visit_miv_spectrum_primitives(visual, spectrum_visual_rect(plot), |primitive| {
            let mut color = spectrum_color(
                primitive.band_index,
                primitive.band_count,
                primitive.color_value,
            );
            if primitive.brightness_scale != 1.0 {
                color = brighten_color(color, primitive.brightness_scale);
            }
            if let Some(alpha) = primitive.alpha_override {
                color = with_alpha(color, alpha);
            }
            painter.rect_filled(
                egui::Rect::from_min_max(
                    egui::pos2(primitive.rect.left, primitive.rect.top),
                    egui::pos2(primitive.rect.right, primitive.rect.bottom),
                ),
                primitive.corner_radius_px,
                egui_color(color),
            );
        });

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
            self.note_min_midi,
            self.pitch_probabilities.as_deref(),
            &mut self.keyboard,
            &mut self.keyboard_frame,
            &mut self.keyboard_layout,
            norm_gain_db,
            scheme,
        );
        // 鍵盤と同じ確率から最低音を選ぶ (残光は保持、明るさ=確率)。
        draw_bass_marker(
            &painter,
            keyboard_rect,
            self.note_min_midi,
            &mut self.keyboard,
            &self.keyboard_frame,
            &mut self.keyboard_layout,
            &mut self.bass_markers,
            select_bass(self.pitch_probabilities.as_deref().unwrap_or(&[])),
            scheme,
        );
    }
}

fn run_music_spectrum_worker(
    request_rx: mpsc::Receiver<MusicSpectrumRequest>,
    result_tx: mpsc::Sender<MusicSpectrumResult>,
    cancel: Arc<AtomicBool>,
) {
    let mut analyzer = SpectrumAnalyzer::new(SPECTRUM_BANDS);
    let mut pitch = SpectrumPitchTracker::new();
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
        let mut analysis = compute_spectrum(&mut analyzer, &request);
        analysis.pitch_probabilities = pitch.analyze(&request, &cancel);
        if cancel.load(Ordering::Relaxed) {
            break;
        }
        let result = MusicSpectrumResult {
            center_secs: request.center_secs,
            analysis,
        };
        if result_tx.send(result).is_err() {
            break;
        }
    }
}

/// Published only after a frame is computed. Retaining the Arc also prevents pointer reuse.
struct PitchTrackerCursor {
    pcm: Arc<MusicPcm>,
    first_frame: i64,
    next_frame: i64,
}

#[derive(Debug, PartialEq, Eq)]
enum PitchDecision {
    Reuse,
    Advance(i64),
    Restart(i64),
}

fn decide(cursor: Option<&PitchTrackerCursor>, pcm: &Arc<MusicPcm>, k_end: i64) -> PitchDecision {
    let from = (k_end - WARMUP_FRAMES).max(1);
    if let Some(cursor) = cursor
        && Arc::ptr_eq(&cursor.pcm, pcm)
        && cursor.first_frame <= from
    {
        let distance = k_end - (cursor.next_frame - 1);
        if distance == 0 {
            return PitchDecision::Reuse;
        }
        if (1..=MAX_ADVANCE_FRAMES).contains(&distance) {
            return PitchDecision::Advance(distance);
        }
    }
    PitchDecision::Restart(from)
}

fn pitch_end_frame(center_secs: f64) -> i64 {
    pitch_frame_at_secs(center_secs) + PITCH_LOOKAHEAD_FRAMES
}

struct SpectrumPitchTracker {
    tracker: HarmNetTracker,
    cursor: Option<PitchTrackerCursor>,
}

impl SpectrumPitchTracker {
    fn new() -> Self {
        Self {
            tracker: HarmNetTracker::new(48_000),
            cursor: None,
        }
    }

    fn analyze(&mut self, request: &MusicSpectrumRequest, cancel: &AtomicBool) -> Option<Vec<f32>> {
        if request.pcm.sample_rate != 48_000 {
            return Some(vec![0.0; HARMNET_PITCH_COUNT]);
        }
        let k_end = pitch_end_frame(request.center_secs);
        let mut next_frame = match decide(self.cursor.as_ref(), &request.pcm, k_end) {
            PitchDecision::Reuse => return Some(self.tracker.probabilities(3).to_vec()),
            PitchDecision::Advance(_) => {
                self.cursor
                    .as_ref()
                    .expect("valid computed cursor")
                    .next_frame
            }
            PitchDecision::Restart(from) => {
                self.tracker.reset();
                self.cursor = None;
                from
            }
        };
        let hop = HARMNET_HOP_SAMPLES as i64;
        let start = next_frame * hop - HARMNET_LONG_WINDOW_SAMPLES as i64;
        let stereo = request.pcm.copy_range(start, k_end * hop);
        // Convert after releasing the read lock; sanitize as the upstream stereo frontend does.
        let mono: Vec<f32> = stereo
            .chunks_exact(2)
            .map(|frame| {
                let left = if frame[0].is_finite() { frame[0] } else { 0.0 };
                let right = if frame[1].is_finite() { frame[1] } else { 0.0 };
                0.5 * left + 0.5 * right
            })
            .collect();
        while next_frame <= k_end {
            if cancel.load(Ordering::Relaxed) {
                return None;
            }
            let end = (next_frame * hop - start) as usize;
            if end > mono.len() {
                return None;
            }
            self.tracker
                .push_frame(&mono[end - HARMNET_LONG_WINDOW_SAMPLES..end]);
            next_frame += 1;
            // The cursor describes actual inference state, never an uncomputed restart origin.
            if let Some(cursor) = self.cursor.as_mut() {
                cursor.next_frame = next_frame;
            } else {
                self.cursor = Some(PitchTrackerCursor {
                    pcm: Arc::clone(&request.pcm),
                    first_frame: next_frame - 1,
                    next_frame,
                });
            }
        }
        Some(self.tracker.probabilities(3).to_vec())
    }
}

fn compute_spectrum(
    analyzer: &mut SpectrumAnalyzer,
    request: &MusicSpectrumRequest,
) -> SpectrumAnalysis {
    let pcm = &request.pcm;
    let center_frame =
        (request.center_secs.max(0.0) * pcm.sample_rate.max(1) as f64).round() as usize;
    // デコード済みプレフィックスから窓をコピー (progressive: 未デコード領域を中心に指すと None)。
    match pcm.copy_window(center_frame, SPECTRUM_SNAPSHOT_RADIUS_SECS) {
        Some((window, local_center)) => {
            analyzer.analyze_moving_window(&window, pcm.sample_rate, local_center)
        }
        None => SpectrumAnalysis {
            bands: Vec::new(),
            notes: Vec::new(),
            pitch_probabilities: None,
            note_min_midi: SPECTRUM_NOTE_MIN_MIDI,
        },
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
    let hz = spectrum_axis_hz(spectrum_visual_rect(plot), x);
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

#[allow(clippy::too_many_arguments)]
fn draw_pitch_keyboard(
    painter: &egui::Painter,
    rect: egui::Rect,
    notes: &[f32],
    note_min_midi: u8,
    pitch_probabilities: Option<&[f32]>,
    keyboard: &mut KeyboardAnimationState,
    keyboard_frame: &mut KeyboardFrame,
    keyboard_layout: &mut KeyboardLayoutCache,
    norm_gain_db: f32,
    scheme: MusicNoteColorScheme,
) {
    // 鍵盤矩形でクリップし、均等幅で描いた鍵の軸外はみ出しを表示上でカットする。
    let painter = painter.with_clip_rect(rect);
    painter.rect_filled(rect, 0.0, egui::Color32::BLACK);
    prepare_keyboard_frame_with_input_into(
        notes,
        note_min_midi,
        norm_gain_db,
        KeyboardCenterInput::FundamentalProbabilities(pitch_probabilities),
        keyboard,
        keyboard_frame,
    );
    let layout = layout_keyboard_cached(
        keyboard_frame,
        KeyboardRect::new(rect.left(), rect.top(), rect.right(), rect.bottom()),
        keyboard_layout,
    );

    for &x in &layout.octave_guides_x {
        painter.line_segment(
            [egui::pos2(x, rect.top()), egui::pos2(x, rect.bottom())],
            egui::Stroke::new(
                0.8,
                egui::Color32::from_rgba_unmultiplied(120, 134, 148, 82),
            ),
        );
    }

    for &key in &layout.keys {
        let key_rect = egui::Rect::from_min_max(
            egui::pos2(key.rect.left, key.rect.top),
            egui::pos2(key.rect.right, key.rect.bottom),
        );
        let corner = if key.black { 1.0 } else { 0.0 };
        if key.real_key {
            let center_val = key.levels.center;
            let mid_color = key_fill(key.midi, center_val, key.black, scheme);
            let top_color = key_fill(key.midi, center_val * key.levels.odd, key.black, scheme);
            let bottom_color = key_fill(key.midi, center_val * key.levels.even, key.black, scheme);
            fill_key_gradient(&painter, key_rect, top_color, mid_color, bottom_color);
        } else {
            painter.rect_filled(
                key_rect,
                corner,
                egui_color(unlit_key_base(key.black, false)),
            );
        }
        let stroke = if key.black {
            egui::Stroke::new(
                0.75,
                egui::Color32::from_rgba_unmultiplied(255, 255, 255, 80),
            )
        } else {
            egui::Stroke::new(0.8, egui::Color32::from_rgba_unmultiplied(0, 0, 0, 155))
        };
        painter.rect_stroke(key_rect, corner, stroke, egui::StrokeKind::Inside);
    }
}

/// 共通選択関数が返す MIDI 鍵へ直接 ▼ を描く。倍音でオクターブを選び直さない。
fn draw_bass_marker(
    painter: &egui::Painter,
    keyboard_rect: egui::Rect,
    note_min_midi: u8,
    keyboard: &mut KeyboardAnimationState,
    keyboard_frame: &KeyboardFrame,
    keyboard_layout: &mut KeyboardLayoutCache,
    markers: &mut Vec<BassMarkerPlacement>,
    bass_marker: BassEstimate,
    scheme: MusicNoteColorScheme,
) {
    let rect = KeyboardRect::new(
        keyboard_rect.left(),
        keyboard_rect.top(),
        keyboard_rect.right(),
        keyboard_rect.bottom(),
    );
    let layout = layout_keyboard_cached(keyboard_frame, rect, keyboard_layout);
    layout_bass_markers_with_input_into(
        &[],
        note_min_midi,
        rect,
        keyboard,
        KeyboardBassInput::AbsoluteMidi(bass_marker),
        Some(layout),
        markers,
    );
    for marker in markers.iter().copied() {
        let m = marker.intensity.clamp(0.0, 1.0);
        let base = note_color(60 + marker.midi % 12, 0.75 + 0.25 * m, scheme);
        let alpha =
            (BASS_MARKER_ALPHA_MIN + (BASS_MARKER_ALPHA_MAX - BASS_MARKER_ALPHA_MIN) * m) as u8;
        let color = egui::Color32::from_rgba_unmultiplied(base.r(), base.g(), base.b(), alpha);
        let triangle = marker
            .triangle
            .into_iter()
            .map(|point| egui::pos2(point.x, point.y))
            .collect();
        painter.add(egui::Shape::convex_polygon(
            triangle,
            color,
            egui::Stroke::NONE,
        ));
    }
}

/// 鍵を縦グラデ (上端 → 中央 → 下端) の頂点カラー付き mesh で塗る。中央が最も明るい基音アンカー。
fn key_fill(midi: u8, value: f32, black: bool, scheme: MusicNoteColorScheme) -> egui::Color32 {
    let active = note_color(midi, value, scheme);
    egui_color(key_fill_with_color(
        value,
        black,
        VisualizationColor::from_rgb(active.r(), active.g(), active.b()),
    ))
}

fn fill_key_gradient(
    painter: &egui::Painter,
    key_rect: egui::Rect,
    top_color: egui::Color32,
    mid_color: egui::Color32,
    bottom_color: egui::Color32,
) {
    let mut mesh = egui::epaint::Mesh::default();
    let (l, r) = (key_rect.left(), key_rect.right());
    let (t, m, b) = (key_rect.top(), key_rect.center().y, key_rect.bottom());
    mesh.colored_vertex(egui::pos2(l, t), top_color); // 0 上端左
    mesh.colored_vertex(egui::pos2(r, t), top_color); // 1 上端右
    mesh.colored_vertex(egui::pos2(l, m), mid_color); // 2 中央左
    mesh.colored_vertex(egui::pos2(r, m), mid_color); // 3 中央右
    mesh.colored_vertex(egui::pos2(l, b), bottom_color); // 4 下端左
    mesh.colored_vertex(egui::pos2(r, b), bottom_color); // 5 下端右
    mesh.add_triangle(0, 1, 3);
    mesh.add_triangle(0, 3, 2);
    mesh.add_triangle(2, 3, 5);
    mesh.add_triangle(2, 5, 4);
    painter.add(egui::Shape::mesh(mesh));
}

fn spectrum_visual_rect(rect: egui::Rect) -> SpectrumVisualRect {
    SpectrumVisualRect::new(rect.left(), rect.top(), rect.right(), rect.bottom())
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

fn egui_color(color: VisualizationColor) -> egui::Color32 {
    egui::Color32::from_rgba_unmultiplied(color.r, color.g, color.b, color.a)
}

#[cfg(test)]
mod tests {
    #[test]
    fn bass_marker_draws_selected_absolute_midi_and_keeps_only_afterglow() {
        use super::*;
        for midi in [21, 33, 40, 55] {
            let ctx = egui::Context::default();
            let mut keyboard = KeyboardAnimationState::default();
            let frame = KeyboardFrame::default();
            let mut layout = KeyboardLayoutCache::default();
            let mut markers = Vec::new();
            let mut probabilities = vec![0.0; 88];
            probabilities[(midi - 21) as usize] = 0.5;
            probabilities[(midi + 12 - 21) as usize] = 0.99;
            let selected = select_bass(&probabilities);
            assert_eq!(selected.midi, midi);
            let rect = egui::Rect::from_min_size(egui::pos2(0.0, 50.0), egui::vec2(1200.0, 34.0));
            let _ = ctx
                .run_pass(Default::default(), |ctx| {
                    draw_bass_marker(
                        &ctx.layer_painter(egui::LayerId::background()),
                        rect,
                        SPECTRUM_NOTE_MIN_MIDI,
                        &mut keyboard,
                        &frame,
                        &mut layout,
                        &mut markers,
                        selected,
                        MusicNoteColorScheme::default(),
                    );
                })
                .drop_without_applying_deltas();
            assert_eq!(markers.len(), 1);
            assert_eq!(markers[0].midi, midi);
            let old = markers[0].intensity;
            let _ = ctx
                .run_pass(Default::default(), |ctx| {
                    draw_bass_marker(
                        &ctx.layer_painter(egui::LayerId::background()),
                        rect,
                        SPECTRUM_NOTE_MIN_MIDI,
                        &mut keyboard,
                        &frame,
                        &mut layout,
                        &mut markers,
                        BassEstimate::default(),
                        MusicNoteColorScheme::default(),
                    );
                })
                .drop_without_applying_deltas();
            assert_eq!(markers.len(), 1);
            assert_eq!(markers[0].midi, midi);
            assert!((markers[0].intensity - old * 0.92).abs() < 1e-6);
        }
    }

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
    fn music_pcm_progressive_append_grows_and_windows() {
        // progressive: 空バッファは窓が取れない → None (下段は「解析中」表示のまま)。
        let pcm = MusicPcm::with_capacity(10, 0);
        assert_eq!(pcm.decoded_frames(), 0);
        assert!(!pcm.is_complete());
        assert!(pcm.copy_window(0, 1.0).is_none());

        // 10Hz で 10 フレーム (interleaved stereo = 20 サンプル) 追記。
        let delta: Vec<f32> = (0..20).map(|i| i as f32).collect();
        pcm.append(&delta).unwrap();
        assert_eq!(pcm.decoded_frames(), 10);

        // frame 6 中心 / radius 0.2s @10Hz → [4,9) の窓 (spectrum_window_range と一致)。
        let (window, center) = pcm
            .copy_window(6, 0.2)
            .expect("window available after append");
        assert_eq!(window.len(), (9 - 4) * 2);
        // 窓先頭は frame 4 → interleaved index 8。
        assert_eq!(window[0], 8.0);
        assert!((center - 0.2).abs() < 1.0e-9);

        // さらに追記するとデコード済みフレームが伸びる (progressive)。
        pcm.append(&delta).unwrap();
        assert_eq!(pcm.decoded_frames(), 20);

        pcm.mark_complete();
        assert!(pcm.is_complete());
    }

    #[test]
    fn music_pcm_ahead_of_frontier_returns_none_until_complete() {
        // 10Hz、10 フレームだけデコード済み (先端 = frame 10)。
        let pcm = MusicPcm::with_capacity(10, 0);
        pcm.append(&(0..20).map(|i| i as f32).collect::<Vec<f32>>())
            .unwrap();

        // 未完了で、再生位置が先端より窓半径 (0.2s @10Hz = 2 フレーム) を超えて先 (frame 50) →
        // 窓がデコード済み領域と重ならないので stale 末尾窓を返さず None (「解析中」維持)。
        assert!(pcm.copy_window(50, 0.2).is_none());
        // 先端付近 (frame 11、半径内で重なる) はクランプされた窓を返す (追いつきかけの許容)。
        assert!(pcm.copy_window(11, 0.2).is_some());

        // デコード完了後は seek forward でも従来どおり末尾クランプ窓を返す (末尾 spectrum は妥当)。
        pcm.mark_complete();
        assert!(pcm.copy_window(50, 0.2).is_some());
    }

    #[test]
    fn music_pcm_with_prefix_sees_current_samples() {
        let pcm = MusicPcm::with_capacity(48_000, 4);
        pcm.append(&[1.0, 2.0, 3.0, 4.0]).unwrap(); // 2 frames
        let (len, rate) = pcm.with_prefix(|prefix, rate| (prefix.len(), rate));
        assert_eq!(len, 4);
        assert_eq!(rate, 48_000);
    }

    #[test]
    fn music_pcm_range_pads_only_known_boundaries() {
        let pcm = MusicPcm::with_capacity(48_000, 0);
        pcm.append(&[1.0, 2.0, 3.0, 4.0]).unwrap();
        assert_eq!(
            pcm.copy_range(-2, 4),
            vec![0.0, 0.0, 0.0, 0.0, 1.0, 2.0, 3.0, 4.0]
        );
        assert!(pcm.copy_range(3, 5).is_empty());
        assert_eq!(pcm.copy_range(-3, -1), vec![0.0; 4]);
        assert!(pcm.copy_range(2, 1).is_empty());
        pcm.mark_complete();
        assert_eq!(
            pcm.copy_range(-1, 4),
            vec![0.0, 0.0, 1.0, 2.0, 3.0, 4.0, 0.0, 0.0, 0.0, 0.0]
        );
        assert_eq!(pcm.copy_range(3, 5), vec![0.0; 4]);
    }

    #[test]
    fn pitch_decide_continuity_and_pcm_identity() {
        let pcm = Arc::new(MusicPcm::with_capacity(48_000, 0));
        let cursor = PitchTrackerCursor {
            pcm: Arc::clone(&pcm),
            first_frame: 1,
            next_frame: 101,
        };
        assert_eq!(decide(None, &pcm, 200), PitchDecision::Restart(142));
        assert_eq!(decide(None, &pcm, 9), PitchDecision::Restart(1));
        assert_eq!(decide(Some(&cursor), &pcm, 100), PitchDecision::Reuse);
        assert_eq!(decide(Some(&cursor), &pcm, 101), PitchDecision::Advance(1));
        assert_eq!(decide(Some(&cursor), &pcm, 158), PitchDecision::Advance(58));
        assert_eq!(
            decide(Some(&cursor), &pcm, 159),
            PitchDecision::Restart(101)
        );
        assert_eq!(decide(Some(&cursor), &pcm, 99), PitchDecision::Restart(41));
        let parked_pcm = Arc::clone(&pcm);
        assert_eq!(
            decide(Some(&cursor), &parked_pcm, 100),
            PitchDecision::Reuse
        );
        let other = Arc::new(MusicPcm::with_capacity(48_000, 0));
        assert_eq!(
            decide(Some(&cursor), &other, 100),
            PitchDecision::Restart(42)
        );
        let partial = PitchTrackerCursor {
            pcm: Arc::clone(&pcm),
            first_frame: 142,
            next_frame: 151,
        };
        assert_eq!(
            decide(Some(&partial), &pcm, 150),
            PitchDecision::Restart(92)
        );
        assert_eq!(
            decide(Some(&partial), &pcm, 151),
            PitchDecision::Restart(93)
        );
        assert_eq!(
            decide(Some(&partial), &pcm, 200),
            PitchDecision::Advance(50)
        );
    }

    #[test]
    fn pitch_display_frame_uses_peak_delay_and_age_eight() {
        assert_eq!(pitch_end_frame(0.0), 9);
        assert_eq!(pitch_end_frame(0.2), 19);
        assert_eq!(pitch_end_frame(0.209), 19);
        assert_eq!(pitch_end_frame(0.211), 20);
        assert_eq!(pitch_end_frame(-1.0), 9);
        assert_eq!(pitch_end_frame(f64::NAN), 9);
    }

    fn tone_pcm(frames: usize, complete: bool) -> Arc<MusicPcm> {
        let pcm = Arc::new(MusicPcm::with_capacity(48_000, frames));
        let mut samples = Vec::with_capacity(frames * 2);
        for n in 0..frames {
            let value: f64 = (1..=8)
                .map(|h| {
                    0.1 / h as f64
                        * (std::f64::consts::TAU * 220.0 * h as f64 * n as f64 / 48_000.0).sin()
                })
                .sum();
            samples.extend([value as f32, value as f32]);
        }
        pcm.append(&samples).unwrap();
        if complete {
            pcm.mark_complete();
        }
        pcm
    }

    #[test]
    fn spectrum_worker_publishes_pitch_switches_pcm_and_exits_on_cancel() {
        let (request_tx, request_rx) = mpsc::channel();
        let (result_tx, result_rx) = mpsc::channel();
        let (finished_tx, finished_rx) = mpsc::channel();
        let cancel = Arc::new(AtomicBool::new(false));
        let worker_cancel = Arc::clone(&cancel);
        let worker = std::thread::spawn(move || {
            run_music_spectrum_worker(request_rx, result_tx, worker_cancel);
            finished_tx.send(()).unwrap();
        });
        let tone = tone_pcm(3 * 48_000, true);
        let silence = Arc::new(MusicPcm::with_capacity(48_000, 3 * 48_000));
        silence.append(&vec![0.0; 3 * 48_000 * 2]).unwrap();
        silence.mark_complete();
        let request = |pcm: &Arc<MusicPcm>| MusicSpectrumRequest {
            pcm: Arc::clone(pcm),
            center_secs: 1.42,
        };
        let receive = || {
            result_rx
                .recv_timeout(Duration::from_secs(20))
                .unwrap()
                .analysis
        };

        request_tx.send(request(&tone)).unwrap();
        let analysis = receive();
        assert!(!analysis.bands.is_empty());
        let tone_probabilities = analysis
            .pitch_probabilities
            .expect("worker must publish pitch");
        assert_eq!(tone_probabilities.len(), 88);
        assert!(tone_probabilities.iter().all(|p| p.is_finite()));
        assert!(tone_probabilities[(57 - SPECTRUM_NOTE_MIN_MIDI) as usize] >= 0.5);

        // Same position, different PCM: the previous tone must not be reused.
        request_tx.send(request(&silence)).unwrap();
        assert_eq!(receive().pitch_probabilities, Some(vec![0.0; 88]));
        request_tx.send(request(&tone)).unwrap();
        assert_eq!(receive().pitch_probabilities, Some(tone_probabilities));

        let streaming = tone_pcm(HARMNET_HOP_SAMPLES, false);
        request_tx.send(request(&streaming)).unwrap();
        assert!(receive().pitch_probabilities.is_none());
        streaming.mark_complete();
        request_tx.send(request(&streaming)).unwrap();
        assert_eq!(receive().pitch_probabilities, Some(vec![0.0; 88]));

        // Keep the request sender alive and wake recv after cancellation, verifying
        // the cancel path rather than relying on request-channel disconnection.
        // The worker may also observe cancellation at the top of its loop and
        // disconnect before the wake-up send; both must exit without a result.
        cancel.store(true, Ordering::Relaxed);
        let _ = request_tx.send(request(&tone));
        finished_rx.recv_timeout(Duration::from_secs(20)).unwrap();
        worker.join().unwrap();
        assert!(matches!(
            result_rx.recv_timeout(Duration::from_secs(1)),
            Err(mpsc::RecvTimeoutError::Disconnected)
        ));
    }

    #[test]
    fn pitch_frontier_keeps_cursor_and_resumes_after_append() {
        let pcm = tone_pcm(20 * HARMNET_HOP_SAMPLES + 7, false);
        let request = MusicSpectrumRequest {
            pcm: Arc::clone(&pcm),
            center_secs: 0.5,
        };
        let mut pitch = SpectrumPitchTracker::new();
        let cancel = AtomicBool::new(false);
        assert!(pitch.analyze(&request, &cancel).is_none());
        assert_eq!(pitch.cursor.as_ref().unwrap().next_frame, 21);
        pcm.append(&vec![0.0; 20 * HARMNET_HOP_SAMPLES * 2])
            .unwrap();
        let probabilities = pitch.analyze(&request, &cancel).unwrap();
        assert_eq!(probabilities.len(), 88);
        assert_eq!(
            pitch.cursor.as_ref().unwrap().next_frame,
            pitch_end_frame(0.5) + 1
        );
        assert_eq!(pitch.analyze(&request, &cancel), Some(probabilities));
    }

    #[test]
    fn pitch_far_undecoded_request_does_not_advance_cursor() {
        let pcm = tone_pcm(HARMNET_HOP_SAMPLES, false);
        let request = MusicSpectrumRequest {
            pcm,
            center_secs: 10.0,
        };
        let mut pitch = SpectrumPitchTracker::new();
        assert!(pitch.analyze(&request, &AtomicBool::new(false)).is_none());
        assert!(pitch.cursor.is_none(), "no frame computed means no cursor");
    }

    #[test]
    fn pitch_partial_restart_then_backward_seek_uses_requested_warmup() {
        for (computed_frames, missing_last_sample) in [0, 1, 9]
            .into_iter()
            .flat_map(|count| [false, true].map(|missing| (count, missing)))
        {
            let new_end = 141 + computed_frames;
            let pcm = tone_pcm(
                new_end * HARMNET_HOP_SAMPLES - usize::from(missing_last_sample),
                false,
            );
            let mut pitch = SpectrumPitchTracker::new();
            let cancel = AtomicBool::new(false);
            let ahead = MusicSpectrumRequest {
                pcm: Arc::clone(&pcm),
                center_secs: 3.82,
            };
            assert_eq!(pitch_end_frame(ahead.center_secs), 200);
            assert!(pitch.analyze(&ahead, &cancel).is_none());
            let pushed = computed_frames.saturating_sub(usize::from(missing_last_sample));
            if pushed == 0 {
                assert!(pitch.cursor.is_none());
            } else {
                let cursor = pitch.cursor.as_ref().unwrap();
                assert_eq!(cursor.first_frame, 142);
                assert_eq!(cursor.next_frame, (142 + pushed) as i64);
            }

            // Return to the decoded frontier, which still needs its own full warmup.
            let back = MusicSpectrumRequest {
                pcm: Arc::clone(&pcm),
                center_secs: (new_end - 9) as f64 * HARMNET_HOP_SECS,
            };
            assert_eq!(pitch_end_frame(back.center_secs), new_end as i64);
            assert!(
                !compute_spectrum(&mut SpectrumAnalyzer::new(SPECTRUM_BANDS), &back)
                    .bands
                    .is_empty()
            );
            let mut probabilities = pitch.analyze(&back, &cancel);
            if missing_last_sample {
                assert!(
                    probabilities.is_none(),
                    "undecoded lookahead must not return phantom Reuse"
                );
                assert_eq!(pitch.cursor.as_ref().unwrap().next_frame, new_end as i64);
                pcm.append(&[0.1, 0.1]).unwrap();
                probabilities = pitch.analyze(&back, &cancel);
            }
            let probabilities = probabilities.expect("decoded lookahead is available");
            assert_eq!(
                pitch.cursor.as_ref().unwrap().first_frame,
                new_end as i64 - WARMUP_FRAMES
            );
            assert!(probabilities[(57 - SPECTRUM_NOTE_MIN_MIDI) as usize] >= 0.5);
            let mut fresh = SpectrumPitchTracker::new();
            assert_eq!(Some(probabilities), fresh.analyze(&back, &cancel));
        }
    }

    #[test]
    fn pitch_zero_frame_restart_discards_previous_computed_cursor() {
        let pcm = tone_pcm(141 * HARMNET_HOP_SAMPLES, false);
        let mut pitch = SpectrumPitchTracker::new();
        let cancel = AtomicBool::new(false);
        assert!(
            pitch
                .analyze(
                    &MusicSpectrumRequest {
                        pcm: Arc::clone(&pcm),
                        center_secs: 0.4
                    },
                    &cancel
                )
                .is_some()
        );
        assert!(pitch.cursor.is_some());
        assert!(
            pitch
                .analyze(
                    &MusicSpectrumRequest {
                        pcm,
                        center_secs: 3.82
                    },
                    &cancel
                )
                .is_none()
        );
        assert!(
            pitch.cursor.is_none(),
            "restart clears the old sequence even when no new frame is ready"
        );
    }

    #[test]
    fn pitch_completed_eof_is_available_for_short_and_fractional_files() {
        for frames in [1, 4_321, 48_123] {
            let pcm = tone_pcm(frames, true);
            let mut pitch = SpectrumPitchTracker::new();
            for center_secs in [0.0, (frames - 1) as f64 / 48_000.0] {
                let request = MusicSpectrumRequest {
                    pcm: Arc::clone(&pcm),
                    center_secs,
                };
                assert_eq!(
                    pitch
                        .analyze(&request, &AtomicBool::new(false))
                        .unwrap()
                        .len(),
                    88,
                    "frames={frames}"
                );
            }
        }
    }

    #[test]
    fn pitch_unsupported_sample_rate_is_valid_zero() {
        let mut pitch = SpectrumPitchTracker::new();
        let request = MusicSpectrumRequest {
            pcm: Arc::new(MusicPcm::with_capacity(44_100, 0)),
            center_secs: 0.0,
        };
        assert_eq!(
            pitch.analyze(&request, &AtomicBool::new(false)),
            Some(vec![0.0; 88])
        );
    }

    #[test]
    fn paused_update_reissues_missing_pitch_after_append_or_completion() {
        for complete_before_update in [false, true] {
            let pcm = tone_pcm(HARMNET_HOP_SAMPLES, false);
            let (request_tx, request_rx) = mpsc::channel();
            let (result_tx, result_rx) = mpsc::channel();
            let mut state = MusicSpectrumState::default();
            state.tx = Some(request_tx);
            state.rx = Some(result_rx);
            state.pending = true;
            state.last_center = 0.0;
            let mut analyzer = SpectrumAnalyzer::new(SPECTRUM_BANDS);
            let mut pitch = SpectrumPitchTracker::new();
            let initial = MusicSpectrumRequest {
                pcm: Arc::clone(&pcm),
                center_secs: 0.0,
            };
            let cancel = AtomicBool::new(false);
            let mut analysis = compute_spectrum(&mut analyzer, &initial);
            analysis.pitch_probabilities = pitch.analyze(&initial, &cancel);
            assert!(!analysis.bands.is_empty());
            assert!(analysis.pitch_probabilities.is_none());
            result_tx
                .send(MusicSpectrumResult {
                    center_secs: 0.0,
                    analysis,
                })
                .unwrap();
            if complete_before_update {
                // Critical ordering: complete before poll/update observes the saved None result.
                pcm.mark_complete();
            } else {
                pcm.append(&vec![0.0; 10 * HARMNET_HOP_SAMPLES * 2])
                    .unwrap();
            }
            let ctx = egui::Context::default();
            state.update(&ctx, Some(&pcm), 0.0, false);
            let request = request_rx
                .try_recv()
                .expect("paused fixed position must reissue None pitch");
            assert!(state.pending);
            let mut analysis = compute_spectrum(&mut analyzer, &request);
            analysis.pitch_probabilities = pitch.analyze(&request, &cancel);
            assert!(analysis.pitch_probabilities.is_some());
            result_tx
                .send(MusicSpectrumResult {
                    center_secs: 0.0,
                    analysis,
                })
                .unwrap();
            state.last_request = None;
            state.update(&ctx, Some(&pcm), 0.0, false);
            assert!(!state.pending);
            assert!(
                request_rx.try_recv().is_err(),
                "valid Some stops paused reissues"
            );
            state.clear();
            assert!(state.pitch_probabilities.is_none());
        }
    }

    #[test]
    fn paused_missing_pitch_reserves_repaint_even_when_throttled() {
        let pcm = tone_pcm(HARMNET_HOP_SAMPLES, false);
        let (request_tx, request_rx) = mpsc::channel();
        let mut state = MusicSpectrumState::default();
        state.tx = Some(request_tx);
        let (result_tx, result_rx) = mpsc::channel();
        state.rx = Some(result_rx);
        result_tx
            .send(MusicSpectrumResult {
                center_secs: 0.0,
                analysis: SpectrumAnalysis {
                    bands: vec![0.0; SPECTRUM_BANDS],
                    ..Default::default()
                },
            })
            .unwrap();
        state.poll();
        state.last_center = 0.0;
        let ctx = egui::Context::default();
        // Consume Context's initial immediate repaint before checking our delayed request.
        for _ in 0..3 {
            let _ = ctx
                .run_pass(egui::RawInput::default(), |_| {})
                .drop_without_applying_deltas();
        }
        let output =
            crate::test_ui::headless_paint_output(ctx.run_pass(egui::RawInput::default(), |_| {
                state.last_request = Some(Instant::now());
                state.update(&ctx, Some(&pcm), 0.0, false);
            }));
        assert!(
            request_rx.try_recv().is_err(),
            "throttle should skip sending"
        );
        assert!(
            output.viewport_output[&egui::ViewportId::ROOT].repaint_delay
                <= SPECTRUM_REFRESH_INTERVAL
        );

        output.drop_without_applying_deltas();
    }

    #[test]
    fn paused_same_time_result_adoption_updates_graph_and_draw_is_read_only() {
        let mut state = MusicSpectrumState::default();
        let (result_tx, result_rx) = mpsc::channel();
        state.rx = Some(result_rx);
        let send = |value, note, pitch_probabilities| {
            result_tx
                .send(MusicSpectrumResult {
                    center_secs: 0.4,
                    analysis: SpectrumAnalysis {
                        bands: vec![value; SPECTRUM_BANDS],
                        notes: vec![note; 88],
                        note_min_midi: SPECTRUM_NOTE_MIN_MIDI,
                        pitch_probabilities,
                    },
                })
                .unwrap();
        };
        send(0.8, 0.2, None);
        state.poll();
        assert!(state.pitch_probabilities.is_none());
        assert_eq!(state.spectrum_visual.frame().bands[0].value, 0.8);

        // Drain only the latest completed result, even when its time is unchanged.
        send(0.4, 0.3, None);
        send(0.1, 0.6, Some(vec![0.9; 88]));
        state.poll();
        assert_eq!(state.bands, vec![0.1; SPECTRUM_BANDS]);
        assert_eq!(state.notes, vec![0.6; 88]);
        assert_eq!(state.pitch_probabilities, Some(vec![0.9; 88]));
        let adopted = state.spectrum_visual.frame().clone();
        assert_eq!(adopted.bands[0].value, 0.1);
        assert_eq!(adopted.bands[0].trail, 0.8 * 0.994);

        state.poll();
        assert_eq!(state.spectrum_visual.frame(), &adopted);
        let ctx = egui::Context::default();
        for _ in 0..2 {
            let _ = ctx
                .run_pass(Default::default(), |ctx| {
                    ctx.root_ui(|root_ui| {
                        egui::CentralPanel::default().show(root_ui, |ui| {
                            state.draw(
                                ui,
                                egui::Rect::from_min_size(
                                    egui::pos2(0.0, 0.0),
                                    egui::vec2(1200.0, 180.0),
                                ),
                                0.0,
                                MusicNoteColorScheme::default(),
                            );
                        })
                    });
                })
                .drop_without_applying_deltas();
            assert_eq!(state.spectrum_visual.frame(), &adopted);
        }
        state.clear();
        assert!(state.spectrum_visual.frame().bands.is_empty());
    }

    #[test]
    fn band_hz_range_is_monotonic_and_spans_audio() {
        let (lo0, hi0) = spectrum_band_hz_range(0, SPECTRUM_BANDS);
        let (lo_last, hi_last) = spectrum_band_hz_range(SPECTRUM_BANDS - 1, SPECTRUM_BANDS);
        assert!(lo0 < hi0);
        assert!(lo_last < hi_last);
        // 低域は E0 の下端 (20Hz 付近)、高域は C#10 の上端 (18kHz 付近)。
        assert!(lo0 < 25.0);
        assert!(hi_last > 15_000.0);
        // バンド中心は MIDI 半音ごとに単調増加。
        let mut prev = 0.0;
        for i in 0..SPECTRUM_BANDS {
            let center = spectrum_band_hz(i, SPECTRUM_BANDS);
            assert!(center > prev);
            prev = center;
        }
        assert!(
            (spectrum_band_hz(69 - SPECTRUM_BAND_MIN_MIDI as usize, SPECTRUM_BANDS) - 440.0).abs()
                < 0.01
        );
    }

    #[test]
    fn note_label_names_reference_pitches() {
        assert_eq!(note_label_for_hz(440.0), "A4");
        assert_eq!(note_label_for_hz(261.6256), "C4");
        assert_eq!(note_label_for_hz(0.0), "--");
    }

    #[test]
    fn keyboard_visuals_harmonic_tone_highlights_fundamental() {
        let pcm = tone_pcm(48_000, true);
        let request = MusicSpectrumRequest {
            pcm,
            center_secs: 0.8,
        };
        let mut analyzer = SpectrumAnalyzer::new(SPECTRUM_BANDS);
        let mut pitch = SpectrumPitchTracker::new();
        let analysis = compute_spectrum(&mut analyzer, &request);
        let probabilities = pitch.analyze(&request, &AtomicBool::new(false)).unwrap();
        let mut state = KeyboardAnimationState::default();
        let mut frame = KeyboardFrame::default();
        for _ in 0..30 {
            prepare_keyboard_frame_with_input_into(
                &analysis.notes,
                SPECTRUM_NOTE_MIN_MIDI,
                0.0,
                KeyboardCenterInput::FundamentalProbabilities(Some(&probabilities)),
                &mut state,
                &mut frame,
            );
        }
        let fundamental = (57 - SPECTRUM_NOTE_MIN_MIDI) as usize;
        let brightest = frame
            .levels
            .iter()
            .enumerate()
            .max_by(|a, b| a.1.center.total_cmp(&b.1.center))
            .unwrap()
            .0;
        assert_eq!(brightest, fundamental);
        assert!(frame.levels[fundamental].center > 0.5);
        for idx in [fundamental + 1, fundamental + 12] {
            assert!(frame.levels[idx].center < frame.levels[fundamental].center * 0.3);
        }
    }

    /// Pure worker benchmark; no application/window launch. Run with --release --ignored --nocapture.
    #[test]
    #[ignore = "release worker timing benchmark"]
    fn harmnet_worker_benchmark() {
        let pcm = tone_pcm(48_000 * 10, true);
        let mut analyzer = SpectrumAnalyzer::new(SPECTRUM_BANDS);
        let mut pitch = SpectrumPitchTracker::new();
        let cancel = AtomicBool::new(false);
        let mut request = MusicSpectrumRequest {
            pcm,
            center_secs: 3.0,
        };
        let start = Instant::now();
        std::hint::black_box(pitch.analyze(&request, &cancel));
        println!(
            "worker warmup (PCM copy + 59 frames, from=end-58 inclusive): {:.3} ms",
            start.elapsed().as_secs_f64() * 1000.0
        );
        std::hint::black_box(compute_spectrum(&mut analyzer, &request));
        let start = Instant::now();
        request.center_secs += HARMNET_HOP_SECS;
        std::hint::black_box(compute_spectrum(&mut analyzer, &request));
        std::hint::black_box(pitch.analyze(&request, &cancel));
        println!(
            "worker normal request (both PCM copies + spectrum FFT + one pitch frame): {:.3} ms",
            start.elapsed().as_secs_f64() * 1000.0
        );
        let start = Instant::now();
        for _ in 0..100 {
            request.center_secs += HARMNET_HOP_SECS;
            std::hint::black_box(compute_spectrum(&mut analyzer, &request));
            std::hint::black_box(pitch.analyze(&request, &cancel));
        }
        println!(
            "worker normal request mean (100): {:.3} ms",
            start.elapsed().as_secs_f64() * 10.0
        );
    }
}
