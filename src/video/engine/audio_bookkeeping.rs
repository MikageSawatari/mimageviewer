//! `AudioBookkeeping` — pump ringbuffer 残量 + audio_tx queued の会計。
//!
//! 設計ドキュメント [docs/video-engine-redesign.md] の「5. AudioBookkeeping の整理」
//! 節を実装。decoder pacing が「audio が枯渇しそうか」を判定する材料を提供する。
//!
//! ## 責務
//! - `pump_buf_secs`: AudioActor が pump 受信後に push、cpal callback で消費後に
//!   再 publish する「ringbuffer に残っている秒数」
//! - `tx_queued_secs`: decoder が audio_tx に積んだ frame の duration 合計。
//!   decoder send 直後に + 、pump recv 直後に - 。
//! - `total_buffer_secs() = pump_buf + tx_queued` が pacing 判断に使われる
//!
//! ## 不変条件
//! - 値は常に `>= 0.0` に clamp する (= CAS で `max(0.0)` する)。
//! - 全アクセスは atomic で完結。Mutex を取らない (RT 経路 = pump worker から呼ぶため)。
//! - 旧 AvClock の `audio_pump_buf_secs_bits` / `audio_tx_queued_secs_bits` を
//!   独立 struct に切り出して責務を明示しただけで、挙動は等価。

use std::sync::atomic::{AtomicU64, Ordering};

/// 動画再生中の音声バッファ会計。
///
/// `Default::default()` で全 0 初期化。
pub struct AudioBookkeeping {
    /// pump リングバッファの残量 (秒、f64 bits)。pump push / fill_output pop で
    /// 直近値を `set_pump_buf_secs` 経由で publish する。
    pump_buf_secs_bits: AtomicU64,
    /// audio_tx に積まれているフレーム合計時間 (秒、f64 bits)。decoder の send 後
    /// `add_tx_queued(+duration)`、pump の recv 後 `add_tx_queued(-duration)`。
    tx_queued_secs_bits: AtomicU64,
}

impl AudioBookkeeping {
    pub const fn new() -> Self {
        Self {
            pump_buf_secs_bits: AtomicU64::new(0),
            tx_queued_secs_bits: AtomicU64::new(0),
        }
    }

    /// pump 内ringbuffer 残量 (秒) を上書き publish。
    pub fn set_pump_buf_secs(&self, secs: f64) {
        let v = if secs.is_finite() && secs >= 0.0 { secs } else { 0.0 };
        self.pump_buf_secs_bits.store(v.to_bits(), Ordering::Release);
    }

    /// pump 内ringbuffer 残量を返す。
    pub fn pump_buf_secs(&self) -> f64 {
        f64::from_bits(self.pump_buf_secs_bits.load(Ordering::Acquire))
    }

    /// audio_tx queued 合計に `delta` を加算する (= decoder send 時に + 、pump recv 時に -)。
    /// 結果は `>= 0.0` に clamp する。
    pub fn add_tx_queued(&self, delta_secs: f64) {
        if !delta_secs.is_finite() {
            return;
        }
        let mut cur = self.tx_queued_secs_bits.load(Ordering::Relaxed);
        loop {
            let new_val = (f64::from_bits(cur) + delta_secs).max(0.0);
            match self.tx_queued_secs_bits.compare_exchange_weak(
                cur,
                new_val.to_bits(),
                Ordering::AcqRel,
                Ordering::Relaxed,
            ) {
                Ok(_) => return,
                Err(actual) => cur = actual,
            }
        }
    }

    /// audio_tx queued 合計を返す。
    pub fn tx_queued_secs(&self) -> f64 {
        f64::from_bits(self.tx_queued_secs_bits.load(Ordering::Acquire))
    }

    /// pump_buf + tx_queued の合計。decoder pacing が「audio safe lo を割っているか」
    /// を判定する材料。
    pub fn total_secs(&self) -> f64 {
        self.pump_buf_secs() + self.tx_queued_secs()
    }

    /// post-seek で pre-seek の会計を 0 リセット (= AvClock::notify_seek_completed の
    /// 旧挙動と同等)。
    pub fn reset(&self) {
        self.pump_buf_secs_bits.store(0, Ordering::Release);
        self.tx_queued_secs_bits.store(0, Ordering::Release);
    }
}

impl Default for AudioBookkeeping {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_to_zero() {
        let bk = AudioBookkeeping::new();
        assert_eq!(bk.pump_buf_secs(), 0.0);
        assert_eq!(bk.tx_queued_secs(), 0.0);
        assert_eq!(bk.total_secs(), 0.0);
    }

    #[test]
    fn set_pump_buf_persists() {
        let bk = AudioBookkeeping::new();
        bk.set_pump_buf_secs(0.5);
        assert_eq!(bk.pump_buf_secs(), 0.5);
        bk.set_pump_buf_secs(1.5);
        assert_eq!(bk.pump_buf_secs(), 1.5);
    }

    #[test]
    fn set_pump_buf_rejects_invalid() {
        let bk = AudioBookkeeping::new();
        bk.set_pump_buf_secs(0.3);
        bk.set_pump_buf_secs(f64::NAN);
        assert_eq!(bk.pump_buf_secs(), 0.0);
        bk.set_pump_buf_secs(0.7);
        bk.set_pump_buf_secs(-1.0);
        assert_eq!(bk.pump_buf_secs(), 0.0);
        bk.set_pump_buf_secs(0.5);
        bk.set_pump_buf_secs(f64::INFINITY);
        assert_eq!(bk.pump_buf_secs(), 0.0, "Infinity must be rejected too");
    }

    #[test]
    fn add_tx_queued_increments_and_decrements() {
        let bk = AudioBookkeeping::new();
        bk.add_tx_queued(0.020);
        bk.add_tx_queued(0.020);
        bk.add_tx_queued(0.020);
        assert!((bk.tx_queued_secs() - 0.060).abs() < 1e-9);
        bk.add_tx_queued(-0.020);
        assert!((bk.tx_queued_secs() - 0.040).abs() < 1e-9);
    }

    #[test]
    fn add_tx_queued_clamps_to_nonnegative() {
        let bk = AudioBookkeeping::new();
        bk.add_tx_queued(0.020);
        // Over-decrement: should clamp to 0
        bk.add_tx_queued(-1.0);
        assert_eq!(bk.tx_queued_secs(), 0.0);
    }

    #[test]
    fn add_tx_queued_ignores_invalid() {
        let bk = AudioBookkeeping::new();
        bk.add_tx_queued(0.020);
        bk.add_tx_queued(f64::NAN);
        bk.add_tx_queued(f64::INFINITY);
        // valid 加算のみ反映
        assert!((bk.tx_queued_secs() - 0.020).abs() < 1e-9);
    }

    #[test]
    fn total_sums_pump_and_tx() {
        let bk = AudioBookkeeping::new();
        bk.set_pump_buf_secs(1.0);
        bk.add_tx_queued(0.25);
        assert!((bk.total_secs() - 1.25).abs() < 1e-9);
    }

    #[test]
    fn reset_zeroes_everything() {
        let bk = AudioBookkeeping::new();
        bk.set_pump_buf_secs(2.0);
        bk.add_tx_queued(0.5);
        bk.reset();
        assert_eq!(bk.pump_buf_secs(), 0.0);
        assert_eq!(bk.tx_queued_secs(), 0.0);
        assert_eq!(bk.total_secs(), 0.0);
    }
}
