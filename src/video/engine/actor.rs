//! `EngineActor` — 動画再生エンジンの内部調停 actor (skeleton)。
//!
//! 設計ドキュメント [docs/video-engine-redesign.md] の「4. EngineActor」節を実装。
//!
//! ## Phase 1c の範囲
//! このファイルは **型と handler signature の skeleton のみ**。
//! - 実際の thread spawn / select / run loop は Phase 3 で実装
//! - decoder/audio/transport との配線も Phase 3 で
//!
//! 本 Phase で確定するもの:
//! - `EngineActor` 構造体のフィールド (clock / state / latch / epoch / context)
//! - command / event の dispatch 関数 signature
//! - 状態遷移 helper (`transition_to_playing` 等) で **anchor → state の publish 順**
//!   を型レベルに強制する
//!
//! ## 不変条件
//! - `current_seek_epoch` のインクリメントは `handle_seek_request` の **1 箇所のみ**
//! - `set_anchor` は EngineActor 経由でしか呼ばない (= MasterClock の `pub(super)` で
//!   静的に強制済み)
//! - state 遷移は必ず `transition_to_*` helper を経由 (= anchor → state の順)

use std::sync::atomic::{AtomicU8, Ordering};
use std::time::Instant;

use super::clock::{ClockAnchor, MasterClock};
use super::state::{
    AudioEvent, DecoderEvent, EngineState, ReadinessLatch, SeekEpoch,
};

/// 外部 (TransportController) から EngineActor への命令。
#[derive(Debug, Clone)]
pub enum TransportCommand {
    Play,
    Pause,
    TogglePlay,
    SeekAbsolute { target_secs: f64 },
    SeekRelative { delta_secs: f64 },
    SetSpeed { speed: f64 },
    SetVolume { volume: f64 },
    SetMuted { muted: bool },
    SetLoopEnabled { enabled: bool },
    Shutdown,
}

/// VideoEngine 構築時の options (= `VideoPlayer::open(path, opts)` の opts)。
/// resume seek を atomic open path に取り込むため、`open` に渡す。
#[derive(Debug, Clone)]
pub struct OpenOptions {
    pub initial_volume: f64,
    pub autoplay: bool,
    /// 過去再生位置からの再開。`None` なら 0 から。
    pub resume_secs: Option<f64>,
    pub loop_enabled: bool,
    pub hw_decode: bool,
}

impl Default for OpenOptions {
    fn default() -> Self {
        Self {
            initial_volume: 0.6,
            autoplay: true,
            resume_secs: None,
            loop_enabled: false,
            hw_decode: false,
        }
    }
}

/// `EngineActor` の内部 context。Phase 1c では struct skeleton のみ。
///
/// 実際の thread / channel 配線は Phase 3 で行う。
#[allow(dead_code)]
pub struct EngineActor {
    /// マスタークロック。anchor 書き手はこの actor のみ。
    clock: MasterClock,

    /// 現在の state。`published_state` (atomic、外部 reader 用) と内部の
    /// match 処理で使う複製を持つ。
    /// **重要**: `set_anchor` → `published_state.store(_, Release)` の順に書く
    /// (= 「state / anchor の publish 順序」設計に従う)。
    state: EngineState,

    /// 外部 reader が `Acquire` で読む state code (= EngineState の discriminant)。
    /// Phase 1c では `EngineState::Idle` の discriminant 0 で初期化。
    published_state: AtomicU8,

    /// 動画 metadata。`InfoReceived` で初期化される。
    duration_secs: Option<f64>,
    has_audio: bool,

    /// 現在の seek 世代。`handle_seek_request` でのみ +1 する。
    /// `SeekCompleted` / `enter_buffering` では進めない。
    current_seek_epoch: SeekEpoch,

    /// readiness latch (Buffering → Playing 遷移用)。
    latch: ReadinessLatch,

    /// AudioRendered の monotonic guard 用、最後に受け入れた anchor PTS と epoch。
    last_audio_pts: f64,
    last_audio_epoch: SeekEpoch,

    /// 構築時に渡された options。autoplay 判定や resume 履歴に使う。
    opts: OpenOptions,
}

/// EngineState を `AtomicU8` で publish するための discriminant code。
/// `published_state` の load/store はこの値で行う。
#[allow(dead_code)]
mod state_code {
    pub const IDLE: u8 = 0;
    pub const LOADING: u8 = 1;
    pub const BUFFERING: u8 = 2;
    pub const PLAYING: u8 = 3;
    pub const PAUSED: u8 = 4;
    pub const SEEKING: u8 = 5;
    pub const EOF: u8 = 6;
}

#[allow(dead_code)]
impl EngineActor {
    /// 新規 actor を構築 (Idle 状態)。実 spawn は Phase 3 で `run` を別 thread で
    /// 走らせる想定。
    pub fn new(opts: OpenOptions) -> Self {
        let clock = MasterClock::with_anchor(ClockAnchor::frozen_at(
            opts.resume_secs.unwrap_or(0.0),
        ));
        Self {
            clock,
            state: EngineState::Idle,
            published_state: AtomicU8::new(state_code::IDLE),
            duration_secs: None,
            has_audio: false,
            current_seek_epoch: 0,
            latch: ReadinessLatch::new(0),
            last_audio_pts: f64::NEG_INFINITY,
            last_audio_epoch: 0,
            opts,
        }
    }

    /// 外部から `Acquire` で state code を読む public な entry point。
    /// MasterClock を保持する `Arc<MasterClock>` と一緒に外部公開する想定。
    pub fn published_state_code(&self) -> u8 {
        self.published_state.load(Ordering::Acquire)
    }

    /// 現在 state への参照 (= EngineActor 内部からのみ使う、Phase 1c では公開不要だが
    /// テスト都合で `pub(super)`)。
    pub(super) fn state(&self) -> EngineState {
        self.state
    }

    /// MasterClock への参照 (= Phase 3 で TransportController が clone して
    /// `position_secs()` を提供する目的)。
    pub fn clock(&self) -> &MasterClock {
        &self.clock
    }

    // ──────────────────────────────────────────────────────────────
    // 状態遷移 helper (= anchor → state の publish 順を強制)
    // ──────────────────────────────────────────────────────────────

    /// `Playing` への遷移。anchor を **先に** 書いてから state を Release で publish。
    fn transition_to_playing(&mut self, anchor: ClockAnchor) {
        debug_assert!(
            !matches!(anchor.source, super::clock::ClockSource::Frozen),
            "transition_to_playing requires non-Frozen anchor"
        );
        self.clock.set_anchor(anchor);
        self.state = EngineState::Playing;
        self.published_state
            .store(state_code::PLAYING, Ordering::Release);
    }

    /// `Paused` への遷移 (= 凍結 anchor)。
    fn transition_to_paused(&mut self, pts: f64) {
        self.clock.set_anchor(ClockAnchor::frozen_at(pts));
        self.state = EngineState::Paused;
        self.published_state
            .store(state_code::PAUSED, Ordering::Release);
    }

    /// `Buffering` への遷移。**epoch は進めない** (= 既に handle_seek_request で進めた値、
    /// または 0 = 初回 open 用)。`pts` は anchor の凍結位置。
    ///
    /// **重要**: latch は `current_seek_epoch` でリセットする。BufferStarved 等の
    /// 非 seek 経路で再 buffering する場合に、前回 ready 済みの latch が残ったまま
    /// 即 Playing に戻る race を防ぐ (Codex Phase 1c P1 反映)。
    fn transition_to_buffering(&mut self, pts: f64) {
        self.latch = ReadinessLatch::new(self.current_seek_epoch);
        self.clock.set_anchor(ClockAnchor::frozen_at(pts));
        self.state = EngineState::Buffering;
        self.published_state
            .store(state_code::BUFFERING, Ordering::Release);
    }

    /// `Seeking` への遷移。target で凍結。**epoch は handle_seek_request で進める**。
    fn transition_to_seeking(&mut self, target_secs: f64) {
        self.clock.set_anchor(ClockAnchor::frozen_at(target_secs));
        self.state = EngineState::Seeking { target_secs };
        self.published_state
            .store(state_code::SEEKING, Ordering::Release);
    }

    /// `Loading` への遷移 (= 0 で凍結 or resume 値で凍結)。
    fn transition_to_loading(&mut self, pts: f64) {
        self.clock.set_anchor(ClockAnchor::frozen_at(pts));
        self.state = EngineState::Loading;
        self.published_state
            .store(state_code::LOADING, Ordering::Release);
    }

    /// `Eof` への遷移 (= duration で凍結)。
    fn transition_to_eof(&mut self, duration: f64) {
        self.clock.set_anchor(ClockAnchor::frozen_at(duration));
        self.state = EngineState::Eof;
        self.published_state
            .store(state_code::EOF, Ordering::Release);
    }

    // ──────────────────────────────────────────────────────────────
    // command / event handlers (Phase 1c では skeleton のみ。実装は Phase 3)
    // ──────────────────────────────────────────────────────────────

    /// command 処理 (Phase 3 で実装)。
    pub(super) fn apply_command(&mut self, _cmd: TransportCommand) {
        // TODO(Phase 3): cmd を分岐して handle_* に dispatch
    }

    /// epoch++ + latch reset + Seeking 遷移 + decoder への SeekTo 命令。
    /// **epoch++ はこの関数の中だけ** で行う (Codex v4 P1)。
    pub(super) fn handle_seek_request(&mut self, target_secs: f64) {
        self.current_seek_epoch = self.current_seek_epoch.saturating_add(1);
        self.latch = ReadinessLatch::new(self.current_seek_epoch);
        self.transition_to_seeking(target_secs);
        // TODO(Phase 3): decoder に SeekTo command を送る
    }

    /// decoder event 処理 (Phase 3 で実装)。
    pub(super) fn handle_decoder_event(&mut self, _ev: DecoderEvent) {
        // TODO(Phase 3): InfoReceived / SeekCompleted / FirstFrameReady /
        // EofReached / Failed を処理。stale (ev.epoch < current_seek_epoch) は捨てる。
    }

    /// audio event 処理 (Phase 3 で実装)。
    pub(super) fn handle_audio_event(&mut self, _ev: AudioEvent) {
        // TODO(Phase 3): AudioRendered (monotonic guard で stale 捨てる) /
        // BufferReady / BufferStarved / AudioInactive を処理。
    }

    /// readiness latch を再評価し、揃っていれば Playing/Paused に遷移する。
    /// `handle_decoder_event(FirstFrameReady)` / `handle_audio_event(BufferReady)` /
    /// `handle_audio_event(AudioInactive)` の各ハンドラ末尾から呼ぶ。
    pub(super) fn try_transition_from_buffering(&mut self) {
        if self.state != EngineState::Buffering {
            return;
        }
        if !self.latch.is_ready(self.has_audio) {
            return;
        }
        // anchor source の選択: audio あり → Audio anchor、なし → Wall anchor。
        // is_ready が true なら Option は必ず Some (= 上記の改訂 is_ready で保証、
        // Codex Phase 1c P2 反映)。
        let anchor = if self.has_audio {
            let (pts, wall) = self.latch.audio_anchor.expect("is_ready guarantees audio_anchor");
            ClockAnchor::audio(pts, wall)
        } else {
            let pts = self.latch.first_frame_pts.expect("is_ready guarantees first_frame_pts");
            ClockAnchor::wall(pts, Instant::now())
        };
        if self.opts.autoplay {
            self.transition_to_playing(anchor);
        } else {
            self.transition_to_paused(anchor.pts_secs);
        }
    }

    /// engine の run loop (Phase 3 で実装)。
    /// command_rx / decoder_event_rx / audio_event_rx を select! で待ち受ける想定。
    pub fn run(self) {
        // TODO(Phase 3): loop { select! { ... } } を実装
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use super::super::clock::ClockSource;

    fn fresh_actor() -> EngineActor {
        EngineActor::new(OpenOptions::default())
    }

    #[test]
    fn new_actor_is_idle_with_resume_zero() {
        let a = fresh_actor();
        assert_eq!(a.state, EngineState::Idle);
        assert_eq!(a.published_state_code(), state_code::IDLE);
        assert_eq!(a.current_seek_epoch, 0);
        assert!((a.clock().now_secs() - 0.0).abs() < 1e-9);
        assert_eq!(a.clock().anchor().source, ClockSource::Frozen);
    }

    #[test]
    fn new_actor_with_resume_initializes_clock() {
        let opts = OpenOptions {
            resume_secs: Some(42.5),
            ..Default::default()
        };
        let a = EngineActor::new(opts);
        assert!((a.clock().now_secs() - 42.5).abs() < 1e-9);
    }

    #[test]
    fn handle_seek_request_advances_epoch_once() {
        let mut a = fresh_actor();
        assert_eq!(a.current_seek_epoch, 0);
        a.handle_seek_request(10.0);
        assert_eq!(a.current_seek_epoch, 1);
        assert_eq!(a.state, EngineState::Seeking { target_secs: 10.0 });
        assert_eq!(a.latch.epoch, 1);
        // anchor は target で frozen
        assert!((a.clock().now_secs() - 10.0).abs() < 1e-9);

        a.handle_seek_request(20.0);
        assert_eq!(a.current_seek_epoch, 2);
        assert_eq!(a.latch.epoch, 2);
        assert_eq!(a.state, EngineState::Seeking { target_secs: 20.0 });
    }

    #[test]
    fn transition_to_playing_publishes_state_code() {
        let mut a = fresh_actor();
        let anchor = ClockAnchor::audio(5.0, Instant::now());
        a.transition_to_playing(anchor);
        assert_eq!(a.state, EngineState::Playing);
        assert_eq!(a.published_state_code(), state_code::PLAYING);
        assert_eq!(a.clock().anchor().source, ClockSource::Audio);
    }

    #[test]
    fn transition_to_paused_freezes_clock() {
        let mut a = fresh_actor();
        a.transition_to_paused(7.5);
        assert_eq!(a.state, EngineState::Paused);
        assert_eq!(a.published_state_code(), state_code::PAUSED);
        assert!((a.clock().now_secs() - 7.5).abs() < 1e-9);
        assert_eq!(a.clock().anchor().source, ClockSource::Frozen);
    }

    #[test]
    fn transition_to_buffering_does_not_advance_epoch() {
        let mut a = fresh_actor();
        a.handle_seek_request(3.0); // epoch=1
        a.transition_to_buffering(3.5);
        assert_eq!(a.current_seek_epoch, 1, "Buffering must not advance epoch");
        assert_eq!(a.state, EngineState::Buffering);
        assert_eq!(a.published_state_code(), state_code::BUFFERING);
    }

    #[test]
    fn try_transition_buffering_to_playing_video_only() {
        let mut a = fresh_actor();
        a.has_audio = false;
        a.transition_to_buffering(0.0);

        // FirstFrameReady を観測した状態を作る
        a.latch.first_frame = true;
        a.latch.first_frame_pts = Some(0.0);

        a.try_transition_from_buffering();
        assert_eq!(a.state, EngineState::Playing);
        assert_eq!(a.clock().anchor().source, ClockSource::Wall);
    }

    #[test]
    fn try_transition_buffering_to_playing_with_audio() {
        let mut a = fresh_actor();
        a.has_audio = true;
        a.transition_to_buffering(0.0);

        a.latch.first_frame = true;
        a.latch.first_frame_pts = Some(0.0);
        a.latch.buffer_ready = true;
        a.latch.audio_anchor = Some((0.05, Instant::now()));

        a.try_transition_from_buffering();
        assert_eq!(a.state, EngineState::Playing);
        assert_eq!(a.clock().anchor().source, ClockSource::Audio);
    }

    #[test]
    fn try_transition_paused_when_autoplay_disabled() {
        let mut a = EngineActor::new(OpenOptions {
            autoplay: false,
            ..Default::default()
        });
        a.has_audio = false;
        a.transition_to_buffering(2.0);
        a.latch.first_frame = true;
        a.latch.first_frame_pts = Some(2.0);
        a.try_transition_from_buffering();
        assert_eq!(a.state, EngineState::Paused);
        assert_eq!(a.clock().anchor().source, ClockSource::Frozen);
    }

    #[test]
    fn try_transition_no_op_when_latch_incomplete() {
        let mut a = fresh_actor();
        a.has_audio = true;
        a.transition_to_buffering(0.0);
        a.latch.first_frame = true;
        a.latch.first_frame_pts = Some(0.0);
        // buffer_ready = false → not ready
        a.try_transition_from_buffering();
        assert_eq!(a.state, EngineState::Buffering, "should remain Buffering");
    }

    #[test]
    fn re_entering_buffering_resets_latch() {
        // BufferStarved 等で Playing → Buffering に再入場するシナリオ。
        // 前回 ready した latch が残ると即 Playing に bounce-back する race を防ぐ。
        let mut a = fresh_actor();
        a.has_audio = false;
        a.transition_to_buffering(0.0);
        a.latch.first_frame = true;
        a.latch.first_frame_pts = Some(0.0);
        a.try_transition_from_buffering();
        assert_eq!(a.state, EngineState::Playing);

        // 再 Buffering 入場 — latch がクリアされ、即 Playing には戻らない
        a.transition_to_buffering(1.0);
        assert!(!a.latch.first_frame);
        assert!(!a.latch.buffer_ready);
        a.try_transition_from_buffering();
        assert_eq!(a.state, EngineState::Buffering, "must wait for fresh first_frame");
    }

    #[test]
    fn is_ready_requires_option_payloads() {
        // first_frame=true でも first_frame_pts=None なら ready とは見なさない
        let mut l = ReadinessLatch::new(0);
        l.first_frame = true;
        l.first_frame_pts = None;
        assert!(!l.is_ready(false), "first_frame_pts=None blocks readiness");
        l.first_frame_pts = Some(0.0);
        assert!(l.is_ready(false));

        // has_audio=true で buffer_ready=true でも audio_anchor=None なら blocked
        l.buffer_ready = true;
        l.audio_anchor = None;
        assert!(!l.is_ready(true), "audio_anchor=None blocks readiness with audio");
        l.audio_anchor = Some((0.05, std::time::Instant::now()));
        assert!(l.is_ready(true));
    }

    #[test]
    fn try_transition_no_op_when_not_buffering() {
        let mut a = fresh_actor();
        a.transition_to_paused(0.0);
        a.latch.first_frame = true;
        a.latch.first_frame_pts = Some(0.0);
        a.try_transition_from_buffering();
        // Paused 状態のままで、Playing には行かない
        assert_eq!(a.state, EngineState::Paused);
    }
}
