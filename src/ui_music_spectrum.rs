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
//! 描画 (`draw`) のピクセル/カラー計算はラボ実装を字面どおり移植している (ラボが機能の正本、
//! §2.1「再利用する」原則)。egui 依存があるため music-core には置けず本体側モジュールとして持つ。

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, PoisonError, RwLock, RwLockReadGuard, RwLockWriteGuard};
use std::time::{Duration, Instant};

use music_core::{
    SPECTRUM_BAND_COUNT, SPECTRUM_BAND_MAX_MIDI, SPECTRUM_BAND_MIN_MIDI, SPECTRUM_NOTE_MAX_MIDI,
    SPECTRUM_NOTE_MIN_MIDI, SpectrumAnalysis, SpectrumAnalyzer,
};

// ── レイアウト・解析定数 (ラボと同値) ──
/// スペクトラムのバンド数 (E0-C#10、MIDI 半音ごと)。
const SPECTRUM_BANDS: usize = SPECTRUM_BAND_COUNT;
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
// ── 鍵盤の倍音サリエンス / 縦グラデ演出パラメータ (実機で視覚チューニングする想定) ──
/// Layer ② 倍音サリエンス加点の強さ。center = raw × (1 + α·harmonic_support)。加点のみ (乗数≥1)
/// なので生より暗くならない。0 で加点無効。
const KEY_HARMONIC_BOOST: f32 = 0.9;
/// Layer ③ 隣接ピークゲート: 局所最大でない鍵の残存率 (0=完全に消す, 1=ゲート無効)。
const KEY_PEAK_GATE_FLOOR: f32 = 0.4;
/// Layer ④ 偶数/奇数倍音サポートの EMA 平滑係数 (縦グラデのチラつき防止、大きいほど即応)。
const KEY_HARMONIC_SMOOTH: f32 = 0.45;
/// Layer ④ 縦グラデで倍音の伸びをどこまで許すか (上下端 = center × factor.clamp(0, これ))。
const KEY_GRADIENT_SPREAD: f32 = 1.0;
/// 偶数倍音の半音オフセット (2,4,6,8倍)。オクターブ (2/4/8倍) は整数半音でぴったり乗る。
const EVEN_HARMONIC_OFFSETS: [usize; 4] = [12, 24, 31, 36];
/// 偶数倍音の寄与重み (高次ほど小さく = 1/n)。
const EVEN_HARMONIC_WEIGHTS: [f32; 4] = [0.5, 0.25, 0.166_7, 0.125];
/// 奇数倍音の半音オフセット (3,5,7倍)。平均律からズレるので近傍 bin の max で拾う。
const ODD_HARMONIC_OFFSETS: [usize; 3] = [19, 28, 34];
/// 奇数倍音の寄与重み (高次ほど小さく = 1/n)。
const ODD_HARMONIC_WEIGHTS: [f32; 3] = [0.333_3, 0.2, 0.142_9];
/// 黒鍵の幅 / 高さ (白鍵に対する比)。実物のピアノ (幅~0.56 / 高さ~0.64) よりやや大きめにして、
/// 白鍵が面積で目立ちすぎるのを抑え、縦グラデ (上=奇数 / 下=偶数) の表示余地も確保する。
const KEY_BLACK_WIDTH_RATIO: f32 = 0.70;
const KEY_BLACK_HEIGHT_RATIO: f32 = 0.68;
const KEYBOARD_DISPLAY_MIN_MIDI: u8 = 12; // C0
const KEYBOARD_DISPLAY_MAX_MIDI: u8 = 143; // B10, 18kHz 軸で右端はクリップされる。
const SPECTRUM_ANALYSIS_MIN_HZ: f32 = 20.0;
const SPECTRUM_AXIS_MIN_HZ: f32 = SPECTRUM_ANALYSIS_MIN_HZ;
const SPECTRUM_VIEW_MAX_HZ: f32 = 18_000.0;

/// スペクトラム更新のリクエスト間隔。再生中はフレーム毎に届くが、この間隔で throttle して
/// 過剰リクエストを抑える (1 フレーム 1 リクエストが上限)。
const SPECTRUM_REFRESH_INTERVAL: Duration = Duration::from_millis(16);

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
    /// 縦グラデ用: 偶数倍音サポート (鍵の下方向の伸び) を平滑保持。
    note_even: Vec<f32>,
    /// 縦グラデ用: 奇数倍音サポート (鍵の上方向の伸び) を平滑保持。
    note_odd: Vec<f32>,
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
            trail: Vec::new(),
            prev_bands: Vec::new(),
            onsets: Vec::new(),
            note_sustain: Vec::new(),
            note_trail: Vec::new(),
            note_even: Vec::new(),
            note_odd: Vec::new(),
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
        self.trail.clear();
        self.prev_bands.clear();
        self.onsets.clear();
        self.note_sustain.clear();
        self.note_trail.clear();
        self.note_even.clear();
        self.note_odd.clear();
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
                &mut self.note_even,
                &mut self.note_odd,
            );
            // まだデコード中 (窓が取れない) 間はプロット領域に「解析中」を出し、無反応に見せない。
            // デコード完了後にバンドが空 = 無音区間なので、その場合はラベルを出さない。
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
            &mut self.note_even,
            &mut self.note_odd,
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
    let center_frame =
        (request.center_secs.max(0.0) * pcm.sample_rate.max(1) as f64).round() as usize;
    // デコード済みプレフィックスから窓をコピー (progressive: 未デコード領域を中心に指すと None)。
    match pcm.copy_window(center_frame, SPECTRUM_SNAPSHOT_RADIUS_SECS) {
        Some((window, local_center)) => {
            analyzer.analyze_moving_window(&window, pcm.sample_rate, local_center)
        }
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

#[allow(clippy::too_many_arguments)]
fn draw_pitch_keyboard(
    painter: &egui::Painter,
    rect: egui::Rect,
    notes: &[f32],
    note_sustain: &mut Vec<f32>,
    note_trail: &mut Vec<f32>,
    note_even: &mut Vec<f32>,
    note_odd: &mut Vec<f32>,
) {
    // 鍵盤矩形でクリップし、均等幅で描いた鍵の軸外はみ出しを表示上でカットする。
    let painter = painter.with_clip_rect(rect);
    painter.rect_filled(rect, 0.0, egui::Color32::BLACK);
    let note_count = (SPECTRUM_NOTE_MAX_MIDI - SPECTRUM_NOTE_MIN_MIDI + 1) as usize;
    for slot in [
        &mut *note_sustain,
        &mut *note_trail,
        &mut *note_even,
        &mut *note_odd,
    ] {
        if slot.len() != note_count {
            slot.clear();
            slot.resize(note_count, 0.0);
        }
    }
    let sustained_notes = update_keyboard_sustain(notes, note_sustain);
    let visuals = compute_keyboard_visuals(&sustained_notes, note_count);
    for idx in 0..note_count {
        // center (基音) は attack 即時 / release 減衰。倍音サポートは EMA で平滑してチラつきを抑える。
        note_trail[idx] = (note_trail[idx] * KEY_HIGHLIGHT_DECAY).max(visuals.center[idx]);
        note_even[idx] =
            note_even[idx] * (1.0 - KEY_HARMONIC_SMOOTH) + visuals.even[idx] * KEY_HARMONIC_SMOOTH;
        note_odd[idx] =
            note_odd[idx] * (1.0 - KEY_HARMONIC_SMOOTH) + visuals.odd[idx] * KEY_HARMONIC_SMOOTH;
    }

    for c_midi in (KEYBOARD_DISPLAY_MIN_MIDI..=144_u8).step_by(12) {
        let x = spectrum_axis_x_unclamped(rect, midi_to_hz(c_midi));
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

    // 白鍵を先に、黒鍵を後に描いて z 順 (黒鍵が上) を保つ。
    for want_black in [false, true] {
        for midi in KEYBOARD_DISPLAY_MIN_MIDI..=KEYBOARD_DISPLAY_MAX_MIDI {
            if is_black_key(midi) != want_black {
                continue;
            }
            let Some(key_rect) = conventional_key_rect(rect, midi, want_black) else {
                continue;
            };
            let corner = if want_black { 1.0 } else { 0.0 };
            let real_key = (SPECTRUM_NOTE_MIN_MIDI..=SPECTRUM_NOTE_MAX_MIDI).contains(&midi);
            if real_key {
                // Layer ④ 縦グラデ: 中央=基音 / 上端=奇数倍音 / 下端=偶数倍音。色相は key_color が
                // ピッチクラスで決めるので維持され、明るさ (濃さ) だけが倍音で変わる。
                let idx = (midi - SPECTRUM_NOTE_MIN_MIDI) as usize;
                let center_val = note_trail[idx];
                let even_factor = note_even[idx].clamp(0.0, KEY_GRADIENT_SPREAD);
                let odd_factor = note_odd[idx].clamp(0.0, KEY_GRADIENT_SPREAD);
                let mid_color = key_fill(midi, center_val, want_black);
                let top_color = key_fill(midi, center_val * odd_factor, want_black);
                let bottom_color = key_fill(midi, center_val * even_factor, want_black);
                fill_key_gradient(&painter, key_rect, top_color, mid_color, bottom_color);
            } else {
                painter.rect_filled(key_rect, corner, unlit_key_base(want_black, false));
            }
            let stroke = if want_black {
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
}

/// 消灯時の鍵ベース色 (黒/白 × real/非表示レンジ)。
fn unlit_key_base(black: bool, real: bool) -> egui::Color32 {
    match (black, real) {
        (false, true) => egui::Color32::from_rgb(208, 214, 219),
        (false, false) => egui::Color32::from_rgb(58, 64, 70),
        (true, true) => egui::Color32::from_rgb(13, 16, 19),
        (true, false) => egui::Color32::from_rgb(34, 39, 44),
    }
}

/// 鍵の点灯色。色相 (ピッチクラス) は `key_color` が保持し、`value` で消灯ベースから blend する。
fn key_fill(midi: u8, value: f32, black: bool) -> egui::Color32 {
    let base = unlit_key_base(black, true);
    let active = key_color(midi, value);
    let blend = (value * if black { 0.96 } else { 0.94 }).clamp(0.0, 1.0);
    lerp_color(base, active, blend)
}

/// 鍵を縦グラデ (上端 → 中央 → 下端) の頂点カラー付き mesh で塗る。中央が最も明るい基音アンカー。
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

/// 鍵盤 1 フレーム分の描画量。`center` が中央 (基音) の明るさ、`even`/`odd` が縦グラデの
/// 下 / 上方向の伸び (倍音サポート)。
struct KeyboardVisuals {
    center: Vec<f32>,
    even: Vec<f32>,
    odd: Vec<f32>,
}

/// 知覚補正済み notes から、鍵盤の中央明るさ (Layer ② 加点のみ倍音サリエンス + Layer ③ 隣接
/// ピークゲート) と、偶数 / 奇数倍音サポートを求める。減点は一切せず、加点は乗数≥1 なので生より
/// 暗くならない (オクターブ重ねの実音を消さない / 高音域を沈めないため)。
fn compute_keyboard_visuals(notes: &[f32], note_count: usize) -> KeyboardVisuals {
    let flat = || KeyboardVisuals {
        center: vec![0.0; note_count],
        even: vec![0.0; note_count],
        odd: vec![0.0; note_count],
    };
    let raw: Vec<f32> = (0..note_count)
        .map(|idx| notes.get(idx).copied().unwrap_or(0.0).clamp(0.0, 1.0))
        .collect();
    let peak = raw.iter().copied().fold(0.0_f32, f32::max);
    if peak < KEY_HIGHLIGHT_MIN_PEAK {
        return flat();
    }

    let mut even_energy = vec![0.0_f32; note_count];
    let mut odd_energy = vec![0.0_f32; note_count];
    for idx in 0..note_count {
        let (even, odd) = harmonic_energies(&raw, idx);
        even_energy[idx] = even;
        odd_energy[idx] = odd;
    }

    // Layer ② 加点のみ (乗数≥1): 基音に倍音支持があるほど明るくする。積 (HPS) ではなく和。
    let mut boosted = vec![0.0_f32; note_count];
    for idx in 0..note_count {
        let support = ((even_energy[idx] + odd_energy[idx]) / (raw[idx] + 1.0e-4)).clamp(0.0, 1.0);
        boosted[idx] = raw[idx] * (1.0 + KEY_HARMONIC_BOOST * support);
    }

    // Layer ③ 隣接ピークゲート: 局所最大でない鍵を減衰し、隣が一斉に光る (漏れ / ビブラート由来)
    // のを抑える。完全に 0 にはせず floor を残してチラつき / 不自然さを防ぐ。
    let mut gated = vec![0.0_f32; note_count];
    for idx in 0..note_count {
        let left = if idx > 0 { boosted[idx - 1] } else { 0.0 };
        let right = boosted.get(idx + 1).copied().unwrap_or(0.0);
        let is_local_max = boosted[idx] >= left && boosted[idx] >= right;
        gated[idx] = if is_local_max {
            boosted[idx]
        } else {
            boosted[idx] * KEY_PEAK_GATE_FLOOR
        };
    }

    let gated_peak = gated.iter().copied().fold(0.0_f32, f32::max).max(1.0e-6);
    // 全体ラウドネスゲート: 静かな箇所で鍵盤が光りっぱなしにならないよう全体を抑える。
    let presence = ((peak - KEY_HIGHLIGHT_MIN_PEAK) / 0.28)
        .clamp(0.0, 1.0)
        .powf(0.7);

    let center = gated
        .iter()
        .map(|value| ((value / gated_peak) * presence).clamp(0.0, 1.0))
        .collect();
    // 縦グラデの伸び factor: 各鍵の倍音エネルギーを基音に対する相対量 (0..1) にする。
    let even = (0..note_count)
        .map(|idx| (even_energy[idx] / (raw[idx] + 1.0e-4)).clamp(0.0, 1.0))
        .collect();
    let odd = (0..note_count)
        .map(|idx| (odd_energy[idx] / (raw[idx] + 1.0e-4)).clamp(0.0, 1.0))
        .collect();
    KeyboardVisuals { center, even, odd }
}

/// note index `idx` の偶数 / 奇数倍音の重み付きエネルギーを、知覚補正済み `notes` から求める。
/// オクターブ倍音 (2/4/8倍) は整数半音に乗るが、奇数倍音や 6倍は平均律からズレるので近傍 bin の
/// max で拾う。高次ほど寄与は小さい (重みは 1/n)。
fn harmonic_energies(notes: &[f32], idx: usize) -> (f32, f32) {
    let sample = |offset: usize| -> f32 {
        let center = idx + offset;
        let mut best = notes.get(center).copied().unwrap_or(0.0);
        if center >= 1 {
            best = best.max(notes.get(center - 1).copied().unwrap_or(0.0));
        }
        best = best.max(notes.get(center + 1).copied().unwrap_or(0.0));
        best.clamp(0.0, 1.0)
    };
    let mut even = 0.0_f32;
    for (offset, weight) in EVEN_HARMONIC_OFFSETS.iter().zip(EVEN_HARMONIC_WEIGHTS) {
        even += weight * sample(*offset);
    }
    let mut odd = 0.0_f32;
    for (offset, weight) in ODD_HARMONIC_OFFSETS.iter().zip(ODD_HARMONIC_WEIGHTS) {
        odd += weight * sample(*offset);
    }
    (even, odd)
}

fn conventional_key_rect(rect: egui::Rect, midi: u8, black: bool) -> Option<egui::Rect> {
    let pc = midi % 12;
    let octave_c = midi - pc;
    let (x0, white_w) = conventional_octave_geometry(rect, octave_c)?;
    let (left, right, bottom) = if black {
        // 黒鍵を白鍵境界の真上に置くと中央の白鍵 (D / G・A) の露出上部が狭くなる。実物ピアノ
        // のように黒鍵を外側へずらし、各グループ内の白鍵の露出上部幅を均等にする (= 見える面積が
        // 揃う)。シフト量は黒鍵幅 r から導出: CDE (黒2) は C#/D# を ±r/6、FGAB (黒3) は外側の
        // F#/A# を ±r/4、中央 G# は据え置き。単位は white_w (center は後で white_w 倍される)。
        let r = KEY_BLACK_WIDTH_RATIO;
        let center = match pc {
            1 => 1.0 - r / 6.0,  // C# 左(外側)へ
            3 => 2.0 + r / 6.0,  // D# 右(外側)へ
            6 => 4.0 - r / 4.0,  // F# 左(外側)へ
            8 => 5.0,            // G# 中央のまま
            10 => 6.0 + r / 4.0, // A# 右(外側)へ
            _ => return None,
        };
        let w = white_w * r;
        (
            x0 + white_w * center - w * 0.5,
            x0 + white_w * center + w * 0.5,
            rect.top() + rect.height() * KEY_BLACK_HEIGHT_RATIO,
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
    // 鍵は自然な (横クランプしない) 幅のまま返す。端のはみ出しは呼び出し側の painter クリップで
    // カットする (幅を切り詰めると枠線が表示端に残って縦線化するため)。
    let left = left + 0.25;
    let right = right - 0.25;
    if right <= left {
        return None;
    }
    // 完全に表示外の鍵は描かない (無駄描画の削減、可視分はクリップ任せ)。
    if right < rect.left() || left > rect.right() {
        return None;
    }
    Some(egui::Rect::from_min_max(
        egui::pos2(left, rect.top()),
        egui::pos2(right, bottom),
    ))
}

fn conventional_octave_geometry(rect: egui::Rect, octave_c: u8) -> Option<(f32, f32)> {
    // クランプしない log 座標を使う。これで全オクターブが均等幅になり、軸下限 (20Hz) 未満の最低
    // オクターブだけ横方向に圧縮されて鍵が小さくなる現象が無くなる (端は painter クリップで切る)。
    let c_x = spectrum_axis_x_unclamped(rect, midi_to_hz(octave_c));
    let next_c_x = spectrum_axis_x_unclamped(rect, midi_to_hz(octave_c.saturating_add(12)));
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

/// `spectrum_axis_x` のクランプなし版 (鍵盤ジオメトリ用)。hz が軸レンジ外だと左端より左 / 右端より
/// 右の座標を返す。均等幅で鍵を並べ、表示端は呼び出し側の painter クリップでカットする前提。
fn spectrum_axis_x_unclamped(rect: egui::Rect, hz: f32) -> f32 {
    let min = SPECTRUM_AXIS_MIN_HZ;
    let max = SPECTRUM_VIEW_MAX_HZ;
    let t = (hz.max(1.0).log2() - min.log2()) / (max.log2() - min.log2());
    rect.left() + t * rect.width()
}

fn spectrum_axis_hz(rect: egui::Rect, x: f32) -> f32 {
    let min = SPECTRUM_AXIS_MIN_HZ;
    let max = SPECTRUM_VIEW_MAX_HZ;
    let t = ((x - rect.left()) / rect.width().max(1.0)).clamp(0.0, 1.0);
    2.0_f32.powf(min.log2() + t * (max.log2() - min.log2()))
}

#[cfg(test)]
fn spectrum_band_hz(index: usize, total: usize) -> f32 {
    spectrum_band_midi(index, total).map_or(SPECTRUM_ANALYSIS_MIN_HZ, midi_to_hz)
}

fn spectrum_band_hz_range(index: usize, total: usize) -> (f32, f32) {
    let Some(midi) = spectrum_band_midi(index, total) else {
        let half = 2.0_f32.powf(1.0 / 24.0);
        return (
            SPECTRUM_ANALYSIS_MIN_HZ / half,
            SPECTRUM_ANALYSIS_MIN_HZ * half,
        );
    };
    let center = midi_to_hz(midi);
    let half_step = 2.0_f32.powf(1.0 / 24.0);
    (center / half_step, center * half_step)
}

fn spectrum_band_midi(index: usize, total: usize) -> Option<u8> {
    if total == 0 || index >= total {
        return None;
    }
    let midi = SPECTRUM_BAND_MIN_MIDI as usize + index;
    (midi <= SPECTRUM_BAND_MAX_MIDI as usize).then_some(midi as u8)
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
    fn keyboard_visuals_silence_is_flat() {
        let note_count = (SPECTRUM_NOTE_MAX_MIDI - SPECTRUM_NOTE_MIN_MIDI + 1) as usize;
        let visuals = compute_keyboard_visuals(&vec![0.0; note_count], note_count);
        assert_eq!(visuals.center.len(), note_count);
        assert!(visuals.center.iter().all(|v| *v == 0.0));
        assert!(visuals.even.iter().all(|v| *v == 0.0));
        assert!(visuals.odd.iter().all(|v| *v == 0.0));
    }

    #[test]
    fn keyboard_visuals_detect_even_harmonics() {
        // 基音 + 2倍音 (オクターブ, +12) + 4倍音 (+24) の偶数倍音構成 → even サポートが odd を上回り、
        // 加点で基音の center が点灯する。
        let note_count = (SPECTRUM_NOTE_MAX_MIDI - SPECTRUM_NOTE_MIN_MIDI + 1) as usize;
        let mut notes = vec![0.0_f32; note_count];
        let fundamental = 24usize; // +24 が範囲内に収まる位置。
        notes[fundamental] = 0.8;
        notes[fundamental + 12] = 0.5;
        notes[fundamental + 24] = 0.3;
        let visuals = compute_keyboard_visuals(&notes, note_count);
        assert!(visuals.center[fundamental] > 0.0);
        assert!(
            visuals.even[fundamental] > visuals.odd[fundamental],
            "even {} should exceed odd {}",
            visuals.even[fundamental],
            visuals.odd[fundamental]
        );
    }

    #[test]
    fn keyboard_peak_gate_attenuates_adjacent_key() {
        // 隣接した 2 鍵のうち弱い方 (局所最大でない) は floor まで減衰する。
        let note_count = (SPECTRUM_NOTE_MAX_MIDI - SPECTRUM_NOTE_MIN_MIDI + 1) as usize;
        let mut notes = vec![0.0_f32; note_count];
        notes[40] = 0.9;
        notes[41] = 0.7; // 局所最大ではない (左隣が強い)。
        let visuals = compute_keyboard_visuals(&notes, note_count);
        assert!(
            visuals.center[40] > visuals.center[41],
            "local max {} should exceed gated neighbor {}",
            visuals.center[40],
            visuals.center[41]
        );
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
