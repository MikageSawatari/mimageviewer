//! `MasterClock` — 動画再生の純粋な時計オブジェクト。
//!
//! 設計ドキュメント [docs/video-engine-redesign.md] の「1. MasterClock」節を実装。
//!
//! ## 責務
//! - 「現在の再生時刻 (秒)」の単一情報源として `now_secs()` を提供
//! - anchor (= 過去のある wall 時刻における動画 PTS) を保持し、`Audio` / `Wall`
//!   ソースなら wall 経過 × speed で extrapolation、`Frozen` なら anchor PTS を返す
//! - **状態を持たない**: playing/seeking/eof などは `EngineState` が別に管理
//!
//! ## 不変条件
//! - anchor 書き込みは **EngineActor のみ** (= `pub(super) fn set_anchor`)。
//!   AudioActor / DecoderActor / UI tick からは絶対に呼ばない。
//!   この制約は `pub(super)` で `video::engine` モジュール内に閉じることで型レベルに
//!   強制する。
//! - `now_secs()` の単調性は **caller (= EngineActor) の書き込み順** に依存。
//!   MasterClock 自身は単調性ガードを持たない (= 旧 `AvClock::set_audio_pts` の
//!   `pts_secs.max(self.now_secs())` のような暗黙ガードは廃止)。
//! - `speed` フィールドは将来の倍速再生に向けた予約。Phase 4 まで `1.0` 固定。
//!
//! ## 同期
//! 全フィールドを 1 つの atomic-shaped 構造 (`AtomicU128`) に格納したいが、
//! 安定 Rust では未提供。代わりに `Mutex<ClockAnchor>` で保護し、`now_secs()` は
//! Mutex を取って anchor を読み出した後で `Instant::now()` を計算する。
//! - Mutex のロック時間は `Instant::now()` 1 回 + 数算術 = ~100 ns 程度
//! - cpal RT callback も `now_secs()` を呼ばない (= EngineActor が events 経由で
//!   anchor を更新するだけ)。RT 経路に Mutex は触れない。
//!
//! ## anchor の publish 順序
//! `set_anchor` は内部で Mutex を取って anchor を全置換する。Mutex Release は
//! Rust の `Mutex` の memory ordering で SeqCst 相当の barrier を提供するため、
//! 後続の atomic state store (= EngineActor の `published_state.store(_, Release)`)
//! と組み合わせれば「anchor → state の publish 順」が保証される
//! (設計ドキュメント「State / Anchor の publish 順序」節)。

use std::sync::Mutex;
use std::time::Instant;

/// MasterClock の anchor 1 件。
///
/// `(pts_secs, wall_at_anchor)` のペアと、extrapolation の方法を決める `source` を持つ。
/// `speed` は再生速度倍率 (1.0 = 等速)。
#[derive(Clone, Copy, Debug)]
pub struct ClockAnchor {
    /// この anchor が示す動画 PTS (秒)。
    pub pts_secs: f64,
    /// この anchor が記録された wall 時刻 (Instant)。`source = Frozen` のときは
    /// 計算に使われないが、デバッグ目的で保持する。
    pub wall_at_anchor: Instant,
    /// 再生速度倍率。0.25..=4.0 程度を想定するが、ここでは clamp しない
    /// (TransportController 側で clamp する設計)。
    pub speed: f64,
    /// extrapolation の根拠。
    pub source: ClockSource,
}

/// anchor の根拠。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ClockSource {
    /// 音声 actor が報告した実 audio PTS が anchor。`now_secs()` は wall 経過 ×
    /// speed を加算して返す (= 通常再生時の挙動)。
    Audio,
    /// 音声無し / 音声出力起動失敗時の wall master。同様に extrapolate するが、
    /// audio は無視。
    Wall,
    /// 一時停止 / Loading / Buffering / Seeking / Eof 中の凍結状態。
    /// `now_secs()` は anchor PTS をそのまま返し、wall 経過は加算しない。
    Frozen,
}

impl ClockAnchor {
    /// `Frozen` anchor を作る。`pts` で凍結時刻を指定。
    pub fn frozen_at(pts_secs: f64) -> Self {
        Self {
            pts_secs,
            wall_at_anchor: Instant::now(),
            speed: 1.0,
            source: ClockSource::Frozen,
        }
    }

    /// 音声 master の anchor を作る。`pts` は最新 audio sample の PTS、
    /// `wall_now` はその callback で取った `Instant::now()`。
    pub fn audio(pts_secs: f64, wall_now: Instant) -> Self {
        Self {
            pts_secs,
            wall_at_anchor: wall_now,
            speed: 1.0,
            source: ClockSource::Audio,
        }
    }

    /// wall fallback の anchor を作る (= 音声無し動画用)。`pts` は最新表示済み
    /// 動画 frame の PTS、`wall_now` はその瞬間の `Instant::now()`。
    pub fn wall(pts_secs: f64, wall_now: Instant) -> Self {
        Self {
            pts_secs,
            wall_at_anchor: wall_now,
            speed: 1.0,
            source: ClockSource::Wall,
        }
    }

    /// speed を更新した版を返す (immutable で扱いやすくするため)。
    /// 現状未使用 (Phase 4 までは 1.0 固定)。
    #[allow(dead_code)]
    pub fn with_speed(mut self, speed: f64) -> Self {
        self.speed = speed;
        self
    }
}

/// マスタークロック本体。
pub struct MasterClock {
    inner: Mutex<ClockAnchor>,
}

impl MasterClock {
    /// 初期状態 = 0.0s で凍結。
    pub fn new() -> Self {
        Self {
            inner: Mutex::new(ClockAnchor::frozen_at(0.0)),
        }
    }

    /// 任意の anchor で初期化する (= 構築時に既知の resume 位置を与える等の用途)。
    pub fn with_anchor(anchor: ClockAnchor) -> Self {
        Self {
            inner: Mutex::new(anchor),
        }
    }

    /// 現在 anchor のスナップショットを取得 (デバッグ・ログ用)。
    pub fn anchor(&self) -> ClockAnchor {
        *self.inner.lock().unwrap()
    }

    /// **EngineActor 専用**: anchor を全置換する。
    ///
    /// 他のスレッド (decoder/audio/UI) からは呼ばないこと。最終目標は `pub(super)`
    /// で `video::engine` モジュール外に漏らさないことだが、現状は **`pub(crate)`**
    /// に緩めた状態を維持する:
    /// `AvClock` (= video::engine 外、`src/video/clock.rs`) が facade として
    /// MasterClock を所有し、旧 `set_audio_pts` 等から内部委譲する必要があるため。
    /// Phase 4 では AvClock を撤去せず薄い facade として残す方針に軌道修正したため
    /// (詳細は [docs/video-engine-redesign.md] の Phase 4 節)、本可視性も `pub(crate)`
    /// のまま。将来 AvClock が完全撤去できた段階で `pub(super)` に戻すこと。
    ///
    /// 入力 anchor の `pts_secs` / `speed` が NaN や負値だと `now_secs()` の戻り値が
    /// 非有限になりうる。caller を信頼し、debug build のみ assert で検出する。
    pub(crate) fn set_anchor(&self, anchor: ClockAnchor) {
        debug_assert!(
            anchor.pts_secs.is_finite(),
            "ClockAnchor.pts_secs must be finite, got {}",
            anchor.pts_secs
        );
        debug_assert!(
            anchor.speed.is_finite() && anchor.speed > 0.0,
            "ClockAnchor.speed must be finite and positive, got {}",
            anchor.speed
        );
        *self.inner.lock().unwrap() = anchor;
    }

    /// 現在の再生時刻 (秒) を返す。
    ///
    /// - `Audio` / `Wall`: `pts + (now - wall_at_anchor) * speed`
    /// - `Frozen`: `pts` (時間進行なし)
    ///
    /// 単調性は呼び出し側 (EngineActor) の書き込み順に依存する。連続して
    /// `now_secs()` を呼んだとき、`set_anchor` が間に挟まれば値が後退しうる。
    pub fn now_secs(&self) -> f64 {
        let anchor = *self.inner.lock().unwrap();
        match anchor.source {
            ClockSource::Frozen => anchor.pts_secs,
            ClockSource::Audio | ClockSource::Wall => {
                let elapsed = Instant::now()
                    .saturating_duration_since(anchor.wall_at_anchor)
                    .as_secs_f64();
                anchor.pts_secs + elapsed * anchor.speed
            }
        }
    }

    /// 現在の anchor source。`EngineActor` がデバッグ・ログで参照する想定。
    #[allow(dead_code)]
    pub fn source(&self) -> ClockSource {
        self.inner.lock().unwrap().source
    }
}

impl Default for MasterClock {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::thread::sleep;
    use std::time::Duration;

    #[test]
    fn frozen_clock_does_not_advance() {
        let clock = MasterClock::new();
        clock.set_anchor(ClockAnchor::frozen_at(12.5));
        assert!((clock.now_secs() - 12.5).abs() < 1e-9);
        sleep(Duration::from_millis(20));
        assert!((clock.now_secs() - 12.5).abs() < 1e-9);
    }

    #[test]
    fn audio_clock_extrapolates_with_wall() {
        let clock = MasterClock::new();
        let t0 = Instant::now();
        clock.set_anchor(ClockAnchor::audio(10.0, t0));
        sleep(Duration::from_millis(50));
        let now = clock.now_secs();
        // 50ms ± 10ms 程度は OS スケジューラの jitter で許容
        assert!(now >= 10.045 && now <= 10.150,
                "expected ~10.05s, got {now}");
    }

    #[test]
    fn wall_clock_extrapolates_like_audio() {
        let clock = MasterClock::new();
        let t0 = Instant::now();
        clock.set_anchor(ClockAnchor::wall(5.0, t0));
        sleep(Duration::from_millis(30));
        let now = clock.now_secs();
        assert!(now >= 5.025 && now <= 5.100,
                "expected ~5.03s, got {now}");
    }

    #[test]
    fn set_anchor_overwrites_previous() {
        let clock = MasterClock::new();
        clock.set_anchor(ClockAnchor::audio(100.0, Instant::now()));
        sleep(Duration::from_millis(10));
        // backward jump (post-seek) — 単調性ガードは無いので素直に後退する。
        // ガードは EngineActor の event handler 側に持つ設計 (= seek_epoch 判定)。
        clock.set_anchor(ClockAnchor::frozen_at(2.0));
        let now = clock.now_secs();
        assert!((now - 2.0).abs() < 1e-9);
    }

    #[test]
    fn anchor_snapshot_returns_current() {
        let t0 = Instant::now();
        let clock = MasterClock::new();
        clock.set_anchor(ClockAnchor::audio(7.5, t0));
        let a = clock.anchor();
        assert_eq!(a.source, ClockSource::Audio);
        assert!((a.pts_secs - 7.5).abs() < 1e-9);
        assert_eq!(a.speed, 1.0);
    }

    #[test]
    fn speed_multiplier_affects_extrapolation() {
        let clock = MasterClock::new();
        let t0 = Instant::now();
        // 2x 速度 → wall 50ms で 動画 100ms 進む想定
        clock.set_anchor(ClockAnchor::audio(0.0, t0).with_speed(2.0));
        sleep(Duration::from_millis(50));
        let now = clock.now_secs();
        assert!(now >= 0.090 && now <= 0.250,
                "expected ~0.10s @ 2x speed, got {now}");
    }

    #[test]
    fn default_anchor_is_zero_frozen() {
        let clock = MasterClock::default();
        assert_eq!(clock.now_secs(), 0.0);
        assert_eq!(clock.source(), ClockSource::Frozen);
    }

    #[test]
    fn with_anchor_initializes_with_given_state() {
        let t0 = Instant::now();
        let clock = MasterClock::with_anchor(ClockAnchor::audio(50.0, t0));
        sleep(Duration::from_millis(10));
        let now = clock.now_secs();
        assert!(now >= 50.005 && now <= 50.080,
                "expected ~50.01s, got {now}");
    }
}
