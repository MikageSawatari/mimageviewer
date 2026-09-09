//! A/V sync drift デバッグ用 atomic bundle。
//!
//! 動画の音声と映像のズレを記録するための共有 atomic 群を 1 つの構造体にまとめる。
//! audio.rs (cpal RT callback / pump スレッド) と present 経路 ([src/video/mod.rs] の
//! `presenter.present()` 成功直後) の両方が `Arc<AudioDiagnostics>` で同じインスタンスを
//! 参照する。
//!
//! ## RT-safe ポリシー
//!
//! - cpal の `fill_output` callback (= RT スレッド) からは **atomic 書き込みのみ**。
//!   `perf::event` や mutex lock 中の serialize は一切しない (= xrun の元)。
//! - JSONL emit は audio pump スレッドで 1Hz snapshot + edge poll する。
//! - present スレッドは drift を atomic に書くだけ。perf event はサンプリング (1Hz +
//!   閾値 edge)。
//!
//! ## 大ジャンプ専用 channel
//!
//! `audio_pts_jump_*` 系は **`|requested_delta_ms| > 5ms` または cap 乖離 (>1ms)** の
//! ときだけ書く。通常更新を毎 callback 上書きすると pump が読む前に Norm 直後の
//! 肝心な jump が消えうるため。pump は `audio_pts_jump_seq` の変化を poll するだけで
//! **大ジャンプを取り逃さない**。
//!
//! 許容範囲: pump が読む前に連続 jump が出た場合、最後の 1 件のみ残る (= 最新値で上書き)。
//! Norm 直後の存在確認が主目的なので許容。全件キャプチャしたい場合は将来 ring buffer
//! 化で拡張可。
//!
//! 詳細は `docs/video-architecture.md` の「A/V drift 計装」節を参照。

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::Instant;

static NEXT_AUDIO_STREAM_ID: AtomicU64 = AtomicU64::new(1);

fn atomic_fetch_max(target: &AtomicU64, value: u64) {
    let mut current = target.load(Ordering::Relaxed);
    while value > current {
        match target.compare_exchange_weak(current, value, Ordering::Relaxed, Ordering::Relaxed) {
            Ok(_) => break,
            Err(observed) => current = observed,
        }
    }
}

pub struct AudioDiagnostics {
    /// 起動時刻。`wall_ns_now()` の基準。
    started_at: Instant,

    /// Process-monotonic identity of the actual CPAL output stream. Zero means that stream
    /// construction has not succeeded and must never be presented as a live stream.
    pub audio_stream_id: AtomicU64,

    /// RT callback cadence and time spent around the one existing audio-buffer lock. These are
    /// updated only when perf diagnostics were enabled before the callback was installed.
    pub callback_count: AtomicU64,
    pub callback_latest_wall_ns: AtomicU64,
    pub callback_expected_period_ns: AtomicU64,
    pub callback_interval_latest_ns: AtomicU64,
    pub callback_interval_max_ns: AtomicU64,
    pub callback_late_max_ns: AtomicU64,
    pub callback_late_count: AtomicU64,
    pub buffer_lock_wait_max_ns: AtomicU64,
    pub fill_duration_max_ns: AtomicU64,
    pub callback_seek_serial: AtomicU64,
    pub callback_engine_state: AtomicU64,

    /// CPAL error callback writes only these atomics. Formatting and logging happen on audio-pump.
    pub device_error_count: AtomicU64,
    pub device_error_category: AtomicU64,
    pub device_error_wall_ns: AtomicU64,

    /// Rare RT-side events formerly logged directly from `fill_output`.
    pub stale_clear_seq: AtomicU64,
    pub stale_clear_pump_serial: AtomicU64,
    pub stale_clear_live_serial: AtomicU64,
    pub stale_clear_processed_secs_bits: AtomicU64,
    pub stale_clear_raw_pending_secs_bits: AtomicU64,
    pub pdc_change_seq: AtomicU64,
    pub pdc_change_delta_secs_bits: AtomicU64,
    pub pdc_change_pts_secs_bits: AtomicU64,

    /// 直近 present 時の drift (ms) を `f64::to_bits` で保持 (video_pts − master_clock)。
    /// **video pacing の健全性指標**で、ユーザー体感の音映像差ではない (= 値が小さくても
    /// audio が clock から乖離していれば実際にはズレが起きる)。
    /// 体感の音映像差は `av_offset_ms_bits` を見ること。
    pub av_drift_ms_bits: AtomicU64,

    /// 音声 callback の `set_audio_pts` 直前で書く「現在 drain 中の chunk の audible PTS」
    /// (= **実際にスピーカーから聞こえている音声の動画タイムライン上の位置**)。
    /// `f64::to_bits` で保持。
    /// Norm 操作で audio buffer 経由の big jump (= cap で拘束される現象) のとき、この値が
    /// `master_clock.now_secs()` から大きく乖離する。
    pub audio_audible_pts_bits: AtomicU64,
    /// `audio_audible_pts_bits` が直近 clear 以降に書かれており、audio/video offset
    /// を計算できるか。動画 only / cpal 起動失敗に加え、seek / buffer clear 直後も
    /// false になる。audio stream の active 判定には使わない。
    pub audio_audible_pts_valid: AtomicBool,

    /// `audio_audible_pts − master_clock.now_secs()` を ms で保持 (`f64::to_bits`)。
    /// callback で `set_audio_pts` 直前に更新する。
    /// + 値 = 音声が master clock より先行している (= 通常 ≈ 0、Norm 経路バグで >>0)。
    /// 「audio が clock から何 ms 先行しているか」のデバッグ指標。
    pub audio_lead_ms_bits: AtomicU64,

    /// **ユーザー体感の音映像差** (video_displayed_pts − audio_audible_pts) ms。
    /// `f64::to_bits` で保持。present 経路で書く。
    /// + 値 = 映像が音声より進んでいる、− 値 = 映像が音声より遅れている (= 普段の不一致報告)。
    /// audio inactive (動画 only / 音声起動失敗) または seek 直後など offset 未確定時は
    /// `f64::NAN`。
    pub av_offset_ms_bits: AtomicU64,

    /// callback 末尾で 0/1 切替。pump スレッド / overlay が読む。
    pub audio_underrun_active: AtomicBool,

    /// underrun begin / end をそれぞれ独立 seq + wall_ns で記録 (短時間に begin → end が
    /// 両方起きても片方落とさない)。
    pub audio_underrun_begin_seq: AtomicU64,
    pub audio_underrun_begin_wall_ns: AtomicU64,
    pub audio_underrun_end_seq: AtomicU64,
    pub audio_underrun_end_wall_ns: AtomicU64,

    /// 累積 silence サンプル数 (stereo interleaved 単位)。
    /// pump スレッドが 1Hz で前回値との差を取って silence_ms_last_sec を計算。
    pub audio_silence_samples_total: AtomicU64,

    /// audio_pts 更新の **大ジャンプ専用**チャネル。`f64::to_bits` で保持。
    pub audio_pts_jump_requested_bits: AtomicU64,
    pub audio_pts_jump_prev_now_bits: AtomicU64,
    pub audio_pts_jump_after_now_bits: AtomicU64,
    pub audio_pts_jump_wall_ns: AtomicU64,
    pub audio_pts_jump_seq: AtomicU64,
}

impl AudioDiagnostics {
    pub fn new(started_at: Instant) -> Self {
        Self {
            started_at,
            audio_stream_id: AtomicU64::new(0),
            callback_count: AtomicU64::new(0),
            callback_latest_wall_ns: AtomicU64::new(0),
            callback_expected_period_ns: AtomicU64::new(0),
            callback_interval_latest_ns: AtomicU64::new(0),
            callback_interval_max_ns: AtomicU64::new(0),
            callback_late_max_ns: AtomicU64::new(0),
            callback_late_count: AtomicU64::new(0),
            buffer_lock_wait_max_ns: AtomicU64::new(0),
            fill_duration_max_ns: AtomicU64::new(0),
            callback_seek_serial: AtomicU64::new(0),
            callback_engine_state: AtomicU64::new(0),
            device_error_count: AtomicU64::new(0),
            device_error_category: AtomicU64::new(0),
            device_error_wall_ns: AtomicU64::new(0),
            stale_clear_seq: AtomicU64::new(0),
            stale_clear_pump_serial: AtomicU64::new(0),
            stale_clear_live_serial: AtomicU64::new(0),
            stale_clear_processed_secs_bits: AtomicU64::new(0.0_f64.to_bits()),
            stale_clear_raw_pending_secs_bits: AtomicU64::new(0.0_f64.to_bits()),
            pdc_change_seq: AtomicU64::new(0),
            pdc_change_delta_secs_bits: AtomicU64::new(0.0_f64.to_bits()),
            pdc_change_pts_secs_bits: AtomicU64::new(0.0_f64.to_bits()),
            av_drift_ms_bits: AtomicU64::new(0.0_f64.to_bits()),
            audio_audible_pts_bits: AtomicU64::new(f64::NAN.to_bits()),
            audio_audible_pts_valid: AtomicBool::new(false),
            audio_lead_ms_bits: AtomicU64::new(0.0_f64.to_bits()),
            av_offset_ms_bits: AtomicU64::new(f64::NAN.to_bits()),
            audio_underrun_active: AtomicBool::new(false),
            audio_underrun_begin_seq: AtomicU64::new(0),
            audio_underrun_begin_wall_ns: AtomicU64::new(0),
            audio_underrun_end_seq: AtomicU64::new(0),
            audio_underrun_end_wall_ns: AtomicU64::new(0),
            audio_silence_samples_total: AtomicU64::new(0),
            audio_pts_jump_requested_bits: AtomicU64::new(0.0_f64.to_bits()),
            audio_pts_jump_prev_now_bits: AtomicU64::new(0.0_f64.to_bits()),
            audio_pts_jump_after_now_bits: AtomicU64::new(0.0_f64.to_bits()),
            audio_pts_jump_wall_ns: AtomicU64::new(0),
            audio_pts_jump_seq: AtomicU64::new(0),
        }
    }

    pub(crate) fn allocate_stream_id() -> u64 {
        NEXT_AUDIO_STREAM_ID.fetch_add(1, Ordering::Relaxed).max(1)
    }

    pub(crate) fn publish_stream_id(&self, id: u64) {
        debug_assert_ne!(id, 0);
        self.audio_stream_id.store(id, Ordering::Release);
    }

    #[cfg(test)]
    pub(crate) fn stream_id(&self) -> Option<u64> {
        match self.audio_stream_id.load(Ordering::Acquire) {
            0 => None,
            id => Some(id),
        }
    }

    pub(crate) fn record_callback_entry(
        &self,
        wall_ns: u64,
        interval_ns: Option<u64>,
        expected_period_ns: u64,
        seek_serial: u64,
        engine_state: u8,
    ) {
        self.callback_count.fetch_add(1, Ordering::Relaxed);
        self.callback_latest_wall_ns
            .store(wall_ns, Ordering::Relaxed);
        self.callback_expected_period_ns
            .store(expected_period_ns, Ordering::Relaxed);
        self.callback_seek_serial
            .store(seek_serial, Ordering::Relaxed);
        self.callback_engine_state
            .store(engine_state as u64, Ordering::Relaxed);
        if let Some(interval_ns) = interval_ns {
            self.callback_interval_latest_ns
                .store(interval_ns, Ordering::Relaxed);
            atomic_fetch_max(&self.callback_interval_max_ns, interval_ns);
            atomic_fetch_max(
                &self.callback_late_max_ns,
                interval_ns.saturating_sub(expected_period_ns),
            );
            if interval_ns > expected_period_ns {
                self.callback_late_count.fetch_add(1, Ordering::Relaxed);
            }
        }
    }

    pub(crate) fn record_buffer_lock_wait(&self, ns: u64) {
        atomic_fetch_max(&self.buffer_lock_wait_max_ns, ns);
    }

    pub(crate) fn record_fill_duration(&self, ns: u64) {
        atomic_fetch_max(&self.fill_duration_max_ns, ns);
    }

    pub(crate) fn take_interval_maxima_ns(&self) -> (u64, u64, u64, u64) {
        (
            self.callback_interval_max_ns.swap(0, Ordering::AcqRel),
            self.callback_late_max_ns.swap(0, Ordering::AcqRel),
            self.buffer_lock_wait_max_ns.swap(0, Ordering::AcqRel),
            self.fill_duration_max_ns.swap(0, Ordering::AcqRel),
        )
    }

    pub(crate) fn record_device_error(&self, category: u64) {
        self.device_error_category
            .store(category, Ordering::Relaxed);
        self.device_error_wall_ns
            .store(self.wall_ns_now(), Ordering::Release);
        self.device_error_count.fetch_add(1, Ordering::Release);
    }

    pub(crate) fn record_stale_clear(
        &self,
        pump_serial: u64,
        live_serial: u64,
        processed_secs: f64,
        raw_pending_secs: f64,
    ) {
        self.stale_clear_pump_serial
            .store(pump_serial, Ordering::Relaxed);
        self.stale_clear_live_serial
            .store(live_serial, Ordering::Relaxed);
        self.stale_clear_processed_secs_bits
            .store(processed_secs.to_bits(), Ordering::Relaxed);
        self.stale_clear_raw_pending_secs_bits
            .store(raw_pending_secs.to_bits(), Ordering::Relaxed);
        self.stale_clear_seq.fetch_add(1, Ordering::Release);
    }

    pub(crate) fn record_pdc_change(&self, delta_secs: f64, pts_secs: f64) {
        self.pdc_change_delta_secs_bits
            .store(delta_secs.to_bits(), Ordering::Relaxed);
        self.pdc_change_pts_secs_bits
            .store(pts_secs.to_bits(), Ordering::Relaxed);
        self.pdc_change_seq.fetch_add(1, Ordering::Release);
    }

    /// 起動時刻からの ns 経過を u64 で返す。`u64::MAX` で clamp (= 約 584 年、実用上問題なし)。
    pub fn wall_ns_now(&self) -> u64 {
        let nanos = self.started_at.elapsed().as_nanos();
        if nanos > u64::MAX as u128 {
            u64::MAX
        } else {
            nanos as u64
        }
    }

    /// 直近 present の drift を読む (overlay 表示用)。
    pub fn load_av_drift_ms(&self) -> f32 {
        f64::from_bits(self.av_drift_ms_bits.load(Ordering::Acquire)) as f32
    }

    /// underrun 状態を読む (overlay 表示用)。
    pub fn load_underrun_active(&self) -> bool {
        self.audio_underrun_active.load(Ordering::Acquire)
    }

    /// 直近の audio audible PTS (= drain 中の chunk の最新 PTS)。
    /// `audio_audible_pts_valid` が false なら `None` (= 音声 inactive または
    /// seek / buffer clear 直後で offset 未確定)。
    pub fn load_audio_audible_pts(&self) -> Option<f64> {
        if !self.audio_audible_pts_valid.load(Ordering::Acquire) {
            return None;
        }
        let v = f64::from_bits(self.audio_audible_pts_bits.load(Ordering::Acquire));
        if v.is_finite() { Some(v) } else { None }
    }

    /// 「audio が master clock より何 ms 先行しているか」(callback 直近値)。
    /// 通常 ≈ 0、Norm 経路バグで >>0 になる。
    pub fn load_audio_lead_ms(&self) -> f32 {
        f64::from_bits(self.audio_lead_ms_bits.load(Ordering::Acquire)) as f32
    }

    /// 直近 present 時のユーザー体感音映像差 (video − audio、ms)。
    /// audio inactive または offset 未確定時は `None`。
    pub fn load_av_offset_ms(&self) -> Option<f32> {
        let v = f64::from_bits(self.av_offset_ms_bits.load(Ordering::Acquire));
        if v.is_finite() { Some(v as f32) } else { None }
    }

    /// 大ジャンプ閾値判定 (pure 関数、unit test 容易)。
    /// `|requested_delta_ms| > 5.0` または `(requested_delta_ms - applied_delta_ms).abs() > 1.0`
    /// のときに jump として記録すべき。
    pub fn should_record_pts_jump(requested_delta_ms: f64, applied_delta_ms: f64) -> bool {
        requested_delta_ms.abs() > 5.0 || (requested_delta_ms - applied_delta_ms).abs() > 1.0
    }

    /// audio buffer の clear (seek / Norm / fast-swap / shutdown 等) で呼ぶ。
    /// `audio_audible_pts_valid=false` にして overlay / analyzer 側で旧値を参照させない。
    /// `av_offset_ms` も NaN にして「present までに新 audio が届くまで体感ズレは未確定」
    /// を表現する (Codex P2 ① 反映)。
    ///
    /// 次に audio callback が `set_audio_pts` を呼ぶと再び `audio_audible_pts_valid=true` に
    /// なり、新しい値で計測が再開する。
    pub fn clear_audio_position(&self) {
        // valid=false を **先に**書く: present 経路は valid → bits の順で読むので、
        // 逆順で書くと「valid=true なのに bits は新しい NaN」の中間状態が見える。
        self.audio_audible_pts_valid.store(false, Ordering::Release);
        self.audio_audible_pts_bits
            .store(f64::NAN.to_bits(), Ordering::Release);
        self.av_offset_ms_bits
            .store(f64::NAN.to_bits(), Ordering::Release);
        self.audio_lead_ms_bits
            .store(0.0_f64.to_bits(), Ordering::Release);
    }
}

/// `NativeFullscreenPresentStats::overlay_snapshot()` に渡す軽量値型。
/// `Source` 全体を渡さないことで結合度を下げる。
#[derive(Clone, Copy, Debug, Default)]
pub struct OverlayDiagnostics {
    /// video pacing health (= video_pts − master_clock、近 0 = 健全)
    pub av_drift_ms: f32,
    /// 体感の音映像差 (video − audio、ms、None = audio inactive or offset pending)。
    /// 数分再生で気づく「音と映像のズレ」はこの値が変わる。
    pub av_offset_ms: Option<f32>,
    /// audio stream が clock source として active か。`av_offset_ms` は seek / buffer clear
    /// 直後に一時 None になるため、HUD の lead / underrun 表示可否はこの値で判定する。
    pub audio_active: bool,
    /// audio が master clock より何 ms 先行しているか (callback 直近値、デバッグ用)。
    pub audio_lead_ms: f32,
    pub audio_underrun_active: bool,
}

impl OverlayDiagnostics {
    pub fn from_diagnostics(diag: &AudioDiagnostics, audio_active: bool) -> Self {
        Self {
            av_drift_ms: diag.load_av_drift_ms(),
            av_offset_ms: diag.load_av_offset_ms(),
            audio_active,
            audio_lead_ms: diag.load_audio_lead_ms(),
            audio_underrun_active: diag.load_underrun_active(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn wall_ns_now_is_monotonic() {
        let diag = AudioDiagnostics::new(Instant::now());
        let a = diag.wall_ns_now();
        std::thread::sleep(Duration::from_millis(1));
        let b = diag.wall_ns_now();
        assert!(b >= a, "wall_ns_now must be monotonic: a={a}, b={b}");
        assert!(
            b > a,
            "wall_ns_now must advance after a sleep: a={a}, b={b}"
        );
    }

    #[test]
    fn should_record_pts_jump_threshold() {
        // 通常更新 (delta < 5ms かつ cap が効いてない) は記録しない
        assert!(!AudioDiagnostics::should_record_pts_jump(2.0, 2.0));
        assert!(!AudioDiagnostics::should_record_pts_jump(-3.0, -3.0));
        assert!(!AudioDiagnostics::should_record_pts_jump(0.0, 0.0));

        // |requested_delta_ms| > 5ms は記録
        assert!(AudioDiagnostics::should_record_pts_jump(5.5, 5.5));
        assert!(AudioDiagnostics::should_record_pts_jump(-10.0, -10.0));
        assert!(AudioDiagnostics::should_record_pts_jump(100.0, 100.0));

        // cap 乖離 (>1ms) は記録 (delta が小さくても)
        assert!(AudioDiagnostics::should_record_pts_jump(3.0, 1.5));
        assert!(AudioDiagnostics::should_record_pts_jump(2.0, 4.0));

        // 境界: ちょうど 1ms 乖離は記録しない (= 厳密 > 1.0)
        assert!(!AudioDiagnostics::should_record_pts_jump(2.0, 1.0));
        // ちょうど 5ms は記録しない (= 厳密 > 5.0)
        assert!(!AudioDiagnostics::should_record_pts_jump(5.0, 5.0));
    }

    #[test]
    fn load_av_drift_ms_round_trip() {
        let diag = AudioDiagnostics::new(Instant::now());
        diag.av_drift_ms_bits
            .store((12.34_f64).to_bits(), Ordering::Release);
        let v = diag.load_av_drift_ms();
        assert!((v - 12.34).abs() < 0.01, "round trip failed: got {v}");
    }

    #[test]
    fn overlay_diagnostics_from_reads_atomics() {
        let diag = AudioDiagnostics::new(Instant::now());
        diag.av_drift_ms_bits
            .store((-7.5_f64).to_bits(), Ordering::Release);
        diag.audio_audible_pts_bits
            .store((1.25_f64).to_bits(), Ordering::Release);
        diag.audio_audible_pts_valid.store(true, Ordering::Release);
        diag.audio_underrun_active.store(true, Ordering::Release);
        let view = OverlayDiagnostics::from_diagnostics(&diag, true);
        assert!((view.av_drift_ms - (-7.5)).abs() < 0.01);
        assert!(view.audio_active);
        assert!(view.audio_underrun_active);
    }

    #[test]
    fn clear_audio_position_invalidates_audio_metrics() {
        // Codex P2 ① 反映: clear_buffer 後に旧 audible_pts が残ると、次の present で
        // 偽の巨大 av_offset が出る。clear_audio_position() で valid=false にして
        // overlay / analyzer が「未確定」として扱えるようにする。
        let diag = AudioDiagnostics::new(Instant::now());
        // 一旦 audio active にする
        diag.audio_audible_pts_bits
            .store((42.5_f64).to_bits(), Ordering::Release);
        diag.audio_audible_pts_valid.store(true, Ordering::Release);
        diag.av_offset_ms_bits
            .store((123.4_f64).to_bits(), Ordering::Release);
        diag.audio_lead_ms_bits
            .store((5128.0_f64).to_bits(), Ordering::Release);
        assert!(diag.load_audio_audible_pts().is_some());
        assert!(diag.load_av_offset_ms().is_some());

        // clear で全部 invalidate
        diag.clear_audio_position();

        assert!(
            diag.load_audio_audible_pts().is_none(),
            "audio_audible_pts must be None after clear"
        );
        assert!(
            diag.load_av_offset_ms().is_none(),
            "av_offset_ms must be None after clear"
        );
        assert_eq!(
            diag.load_audio_lead_ms(),
            0.0,
            "audio_lead_ms must reset to 0 after clear"
        );
        // overlay 側も offset は None として観測するが、audio stream の active 判定は
        // caller が clock から明示的に渡す。
        let view = OverlayDiagnostics::from_diagnostics(&diag, true);
        assert!(view.av_offset_ms.is_none());
        assert!(view.audio_active);
    }

    #[test]
    fn clear_audio_position_publish_order_no_torn_read() {
        // valid=false を先に書くことで「valid=true で旧 bits」の中間状態が見えないか確認。
        // この test は thread race ではなく invariant を pin する目的 (= 関数定義の順序が
        // 変わると次の test が失敗するという回帰検出)。
        let diag = AudioDiagnostics::new(Instant::now());
        diag.audio_audible_pts_bits
            .store((100.0_f64).to_bits(), Ordering::Release);
        diag.audio_audible_pts_valid.store(true, Ordering::Release);
        diag.clear_audio_position();
        // 古い bits は NaN に上書きされている、かつ valid=false なので load は None。
        assert!(diag.load_audio_audible_pts().is_none());
        let raw_bits = diag.audio_audible_pts_bits.load(Ordering::Acquire);
        let raw = f64::from_bits(raw_bits);
        assert!(raw.is_nan(), "bits should be NaN after clear, got {raw}");
    }

    #[test]
    fn stream_ids_are_nonzero_and_process_monotonic() {
        let first = AudioDiagnostics::new(Instant::now());
        let second = AudioDiagnostics::new(Instant::now());
        assert_eq!(first.stream_id(), None);
        let first_id = AudioDiagnostics::allocate_stream_id();
        let second_id = AudioDiagnostics::allocate_stream_id();
        assert_ne!(first_id, 0);
        assert!(second_id > first_id);
        assert_eq!(first.stream_id(), None);
        assert_eq!(second.stream_id(), None);
        first.publish_stream_id(first_id);
        second.publish_stream_id(second_id);
        assert_eq!(first.stream_id(), Some(first_id));
        assert_eq!(second.stream_id(), Some(second_id));
    }

    #[test]
    fn callback_metrics_exclude_first_interval_and_exchange_maxima() {
        let diag = AudioDiagnostics::new(Instant::now());
        diag.record_callback_entry(10, None, 4, 7, 2);
        assert_eq!(diag.callback_count.load(Ordering::Acquire), 1);
        assert_eq!(diag.callback_interval_latest_ns.load(Ordering::Acquire), 0);

        diag.record_callback_entry(22, Some(12), 10, 8, 3);
        diag.record_callback_entry(31, Some(9), 10, 9, 4);
        diag.record_buffer_lock_wait(6);
        diag.record_buffer_lock_wait(4);
        diag.record_fill_duration(15);
        diag.record_fill_duration(11);

        assert_eq!(diag.callback_count.load(Ordering::Acquire), 3);
        assert_eq!(diag.callback_interval_latest_ns.load(Ordering::Acquire), 9);
        assert_eq!(diag.callback_seek_serial.load(Ordering::Acquire), 9);
        assert_eq!(diag.callback_engine_state.load(Ordering::Acquire), 4);
        assert_eq!(diag.callback_late_count.load(Ordering::Acquire), 1);
        assert_eq!(diag.take_interval_maxima_ns(), (12, 2, 6, 15));
        assert_eq!(diag.take_interval_maxima_ns(), (0, 0, 0, 0));
    }

    #[test]
    fn device_and_rare_rt_events_preserve_counts_and_latest_payload() {
        let diag = AudioDiagnostics::new(Instant::now());
        diag.record_device_error(1);
        diag.record_device_error(2);
        assert_eq!(diag.device_error_count.load(Ordering::Acquire), 2);
        assert_eq!(diag.device_error_category.load(Ordering::Acquire), 2);

        diag.record_stale_clear(3, 4, 1.25, 2.5);
        diag.record_stale_clear(5, 6, 3.25, 4.5);
        assert_eq!(diag.stale_clear_seq.load(Ordering::Acquire), 2);
        assert_eq!(diag.stale_clear_pump_serial.load(Ordering::Acquire), 5);
        assert_eq!(diag.stale_clear_live_serial.load(Ordering::Acquire), 6);
        assert_eq!(
            f64::from_bits(
                diag.stale_clear_raw_pending_secs_bits
                    .load(Ordering::Acquire)
            ),
            4.5
        );

        diag.record_pdc_change(-0.012, 7.0);
        assert_eq!(diag.pdc_change_seq.load(Ordering::Acquire), 1);
        assert_eq!(
            f64::from_bits(diag.pdc_change_delta_secs_bits.load(Ordering::Acquire)),
            -0.012
        );
    }
}
