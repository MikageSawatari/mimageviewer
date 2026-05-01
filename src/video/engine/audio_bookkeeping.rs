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
    /// pump 後段 **processed** queue の残量 (秒、f64 bits、post-VST audible)。
    /// `set_pump_buf_secs` 経由で publish。EQ latency 指標。
    pump_buf_secs_bits: AtomicU64,
    /// pump 前段 **raw_pending** queue の残量 (秒、f64 bits、pre-VST)。
    /// `set_raw_pending_secs` 経由で publish (= Codex 助言、2026-05-01)。
    /// raw → processed 変換は VST process_block の数 ms 程度なので、decoder pacing は
    /// raw_pending 内容も「再生可能 audio」として扱う (= `total_secs` に含める)。
    raw_pending_secs_bits: AtomicU64,
    /// audio_tx に積まれているフレーム合計時間 (秒、f64 bits)。decoder の send 後
    /// `add_tx_queued(+duration)`、pump の recv 後 `add_tx_queued(-duration)`。
    tx_queued_secs_bits: AtomicU64,
    /// VST3 プラグインチェーン全体の構造的遅延 (= PDC latency、秒、f64 bits)。
    /// `audio-pump` push 時に最新値が publish される。
    ///
    /// **重要**: `pump_buf_secs` には**含めない**。decoder pacing は actual buffer 残量で
    /// `audio_escape` 判定し、先読み許可量だけ `PACE_LEAD + pdc_latency` を使う設計
    /// (= Codex 助言、2026-05-01)。
    /// これがないと、AudioBuffer が空で cpal が underrun している瞬間でも、
    /// 「pdc 分のバッファあり」に見えて補充が発動しない退行を起こす。
    vst3_pdc_latency_secs_bits: AtomicU64,
}

impl AudioBookkeeping {
    pub const fn new() -> Self {
        Self {
            pump_buf_secs_bits: AtomicU64::new(0),
            raw_pending_secs_bits: AtomicU64::new(0),
            tx_queued_secs_bits: AtomicU64::new(0),
            vst3_pdc_latency_secs_bits: AtomicU64::new(0),
        }
    }

    /// pump 後段 (= processed) ringbuffer 残量 (秒) を上書き publish。
    pub fn set_pump_buf_secs(&self, secs: f64) {
        let v = if secs.is_finite() && secs >= 0.0 {
            secs
        } else {
            0.0
        };
        self.pump_buf_secs_bits
            .store(v.to_bits(), Ordering::Release);
    }

    /// pump 後段 (= processed) ringbuffer 残量を返す。
    pub fn pump_buf_secs(&self) -> f64 {
        f64::from_bits(self.pump_buf_secs_bits.load(Ordering::Acquire))
    }

    /// pump 前段 (= raw_pending) queue 残量 (秒) を上書き publish。
    pub fn set_raw_pending_secs(&self, secs: f64) {
        let v = if secs.is_finite() && secs >= 0.0 {
            secs
        } else {
            0.0
        };
        self.raw_pending_secs_bits
            .store(v.to_bits(), Ordering::Release);
    }

    /// pump 前段 (= raw_pending) queue 残量を返す。
    pub fn raw_pending_secs(&self) -> f64 {
        f64::from_bits(self.raw_pending_secs_bits.load(Ordering::Acquire))
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

    /// audio_tx queued 合計を **0 に強制リセット** (Codex P2、2026-05-01):
    /// pump の seek staleness cleanup から呼ばれ、旧 seek 世代の tx_queued が
    /// `total_audio_buffer_secs()` (= playable) に残るのを防ぐ。
    /// 本リセット後に旧世代 frame が pump に届いても `add_tx_queued(-duration)` は
    /// `max(0.0)` で clamp されるので 0 を割らない。
    pub fn zero_tx_queued(&self) {
        self.tx_queued_secs_bits.store(0, Ordering::Release);
    }

    /// audio_tx queued 合計を返す。
    pub fn tx_queued_secs(&self) -> f64 {
        f64::from_bits(self.tx_queued_secs_bits.load(Ordering::Acquire))
    }

    /// **pacing_audio_secs** = `pump_buf_secs` (= post-VST processed) + `tx_queued_secs`
    /// (= decoder→pump 間の bounded 供給、cap ≒ 0.7 秒)。decoder pacing が
    /// `in_audio_escape` 判定で参照する値。
    ///
    /// **厳密には playable ではない**: tx_queued は pre-VST/pre-pump なので cpal が
    /// 今すぐ鳴らせる audio ではない。「cpal-ready playable + 短い予測補助」という
    /// 折衷値。tx_queued は cap=0.7 秒に縛られるため暴走 supply 誤認のリスクは小さく、
    /// 旧コード (= 1 段 buffer 時代) からの互換性のためにここに含める。
    ///
    /// **raw_pending は含めない** (= Codex 助言、2026-05-01 改訂):
    /// raw_pending は **pre-VST** で cap=30 秒。VST process_block が遅い/詰まる、PDC trim
    /// で drop される場合、実際の playable buffer は 0 でも raw は満杯になる。raw を
    /// 含めると decoder pacing が「音声あり」と誤判断 → video が pacing 無視で burst →
    /// 結果的に audio が underrun したまま動画だけ進む退行。
    ///
    /// **actual buffer のみ** (= PDC latency は含まない、`vst3_pdc_latency_secs` で別取得)。
    ///
    /// 詳細な supply 状態は [`raw_pending_secs`](Self::raw_pending_secs) や
    /// [`supply_secs`](Self::supply_secs) で別途取得可。
    pub fn total_secs(&self) -> f64 {
        self.pump_buf_secs() + self.tx_queued_secs()
    }

    /// raw_pending + tx_queued の合計 (= **pre-VST supply のみ**、診断用)。
    /// decoder pacing は本値を「playable」とは見なさず、starvation 復旧の予兆等の
    /// 判断材料として参照する。
    pub fn supply_secs(&self) -> f64 {
        self.raw_pending_secs() + self.tx_queued_secs()
    }

    /// VST3 PDC latency (秒) を上書き publish。pump push 時に呼ばれる。
    pub fn set_vst3_pdc_latency_secs(&self, secs: f64) {
        let v = if secs.is_finite() && secs >= 0.0 {
            secs
        } else {
            0.0
        };
        self.vst3_pdc_latency_secs_bits
            .store(v.to_bits(), Ordering::Release);
    }

    /// VST3 PDC latency (秒) を返す。decoder pacing が先読み許可量計算に使う。
    pub fn vst3_pdc_latency_secs(&self) -> f64 {
        f64::from_bits(self.vst3_pdc_latency_secs_bits.load(Ordering::Acquire))
    }

    /// post-seek で pre-seek の会計を 0 リセット (= AvClock::notify_seek_completed の
    /// 旧挙動と同等)。PDC latency は次の pump push で再 publish されるので reset しない。
    pub fn reset(&self) {
        self.pump_buf_secs_bits.store(0, Ordering::Release);
        self.raw_pending_secs_bits.store(0, Ordering::Release);
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
    fn total_excludes_raw_pending() {
        // Codex 助言 (2026-05-01 改訂): raw_pending は pre-VST なので playable とは
        // 見なさない。total_secs は processed + tx_queued のみ。
        let bk = AudioBookkeeping::new();
        bk.set_pump_buf_secs(0.3); // post-VST processed
        bk.set_raw_pending_secs(5.0); // pre-VST raw queue (=含めない)
        bk.add_tx_queued(0.5); // audio_tx queued (= 含める、small decoder supply)
        // total = 0.3 + 0.5 = 0.8 (raw_pending は除外)
        assert!((bk.total_secs() - 0.8).abs() < 1e-9);
        // supply_secs は raw + tx
        assert!((bk.supply_secs() - 5.5).abs() < 1e-9);
    }

    #[test]
    fn raw_pending_set_persists_and_resets() {
        let bk = AudioBookkeeping::new();
        bk.set_raw_pending_secs(7.5);
        assert!((bk.raw_pending_secs() - 7.5).abs() < 1e-9);
        bk.reset();
        assert_eq!(bk.raw_pending_secs(), 0.0);
    }

    #[test]
    fn zero_tx_queued_clears_and_clamps_subsequent_negative_delta() {
        // Codex P2 (2026-05-01): seek staleness cleanup で tx_queued を 0 化。
        // その後旧世代 frame が pump に届いて -duration が来ても 0 を割らない。
        let bk = AudioBookkeeping::new();
        bk.add_tx_queued(0.5);
        bk.set_pump_buf_secs(0.3);
        bk.set_raw_pending_secs(2.0);
        // staleness cleanup 相当: tx_queued を 0 化 (= raw / pump_buf は別途 clear 想定)
        bk.zero_tx_queued();
        assert_eq!(bk.tx_queued_secs(), 0.0);
        // pump_buf / raw_pending は touched でない
        assert!((bk.pump_buf_secs() - 0.3).abs() < 1e-9);
        assert!((bk.raw_pending_secs() - 2.0).abs() < 1e-9);
        // 旧世代 frame の subtract: clamp で 0 に張り付く
        bk.add_tx_queued(-0.020);
        bk.add_tx_queued(-0.023);
        assert_eq!(bk.tx_queued_secs(), 0.0);
        // 新世代 frame の add は通常通り
        bk.add_tx_queued(0.040);
        assert!((bk.tx_queued_secs() - 0.040).abs() < 1e-9);
    }

    #[test]
    fn reset_zeroes_everything() {
        let bk = AudioBookkeeping::new();
        bk.set_pump_buf_secs(2.0);
        bk.set_raw_pending_secs(10.0);
        bk.add_tx_queued(0.5);
        bk.reset();
        assert_eq!(bk.pump_buf_secs(), 0.0);
        assert_eq!(bk.raw_pending_secs(), 0.0);
        assert_eq!(bk.tx_queued_secs(), 0.0);
        assert_eq!(bk.total_secs(), 0.0);
    }
}
