//! `EngineActor` — 動画再生エンジンの内部調停 actor。
//!
//! 設計ドキュメント [docs/video-engine-redesign.md] の「4. EngineActor」節を実装。
//!
//! ## Phase 3a の範囲
//! - `apply_command` (TransportController からの命令) の full 実装
//! - `handle_decoder_event` / `handle_audio_event` の full 実装
//! - `try_transition_from_buffering` を全 readiness event 経路から呼ぶ
//! - 状態遷移は必ず helper 経由 (= anchor → state の Release publish 順)
//! - epoch++ は `handle_seek_request` の **1 箇所のみ**
//!
//! ## 不変条件
//! - `current_seek_epoch` のインクリメントは `handle_seek_request` の **1 箇所のみ**
//! - `set_anchor` は EngineActor 経由でしか呼ばない (= MasterClock の `pub(crate)` で
//!   facade 期間中暫定、Phase 4 で `pub(super)` に戻す)
//! - state 遷移は必ず `transition_to_*` helper を経由 (= anchor → state の順)
//! - audio bookkeeping や decoder への seek 命令などの **副作用** は本 Phase 3a では
//!   stub (= TODO Phase 3b で channel 配線を行うときに発火する)

use std::sync::Arc;
use std::sync::atomic::{AtomicU8, AtomicU64, Ordering};
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
    /// Phase 3d で `Arc<AtomicU8>` に変更: decoder thread が pacing loop 内で
    /// state を観察して preroll モードに入るため、actor 外から共有読み取りが必要。
    /// 書き込みは EngineActor 経由のみ。
    published_state: Arc<AtomicU8>,

    /// 動画 metadata。`InfoReceived` で初期化される。
    duration_secs: Option<f64>,
    has_audio: bool,

    /// 現在の seek 世代。`handle_seek_request` でのみ +1 する。
    /// `SeekCompleted` / `enter_buffering` では進めない。
    ///
    /// `Arc<AtomicU64>` で `AvClock` と **同一インスタンスを共有** する
    /// (= 旧版は AvClock と別カウンタを持っていたが、Codex P2 で「呼び出し順を
    /// 間違えると二重 ++」バグが発生したため、構造的に統合)。
    /// `handle_seek_request` は **adaptive** に動作する:
    ///   - 共有カウンタが `last_observed_serial` より進んでいる
    ///     (= `clock.request_seek` で既に bump 済) → 値を観測するだけで bump しない
    ///   - 進んでいない (= 内部経路、loop replay 等) → fetch_add で bump し、
    ///     `clock.request_seek` 経由で SeekRequest を decoder に publish する
    seek_serial: Arc<AtomicU64>,
    /// `clock.request_seek` の publish 用。内部 seek (loop replay / EOF replay /
    /// resume) では `seek_serial` を bump した後、ここ経由で decoder に SeekRequest を
    /// 流す必要がある。
    av_clock: Arc<crate::video::clock::AvClock>,
    /// 直近の `handle_seek_request` 呼び出しで観測したカウンタ値。次回呼び出しが
    /// 「外部 path (= 既に bump 済)」か「内部 path (= 自分が bump すべき)」を判定する。
    last_observed_serial: SeekEpoch,

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
/// `pub` (Phase 3d で decoder.rs から `PLAYING` 比較に使う、Phase 4 で
/// `from_code` ヘルパに移管予定)。
pub mod state_code {
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
    /// 新規 actor を構築 (Idle 状態)。
    ///
    /// `seek_serial` は `AvClock` と共有する `Arc<AtomicU64>`。`av_clock` は内部 seek
    /// (loop replay / EOF replay / resume) で SeekRequest を decoder に publish する
    /// 経路として使う。両方とも `VideoPlayer::open` で組み立てた同じインスタンスへの
    /// `Arc::clone` を渡す。
    pub fn new(
        opts: OpenOptions,
        seek_serial: Arc<AtomicU64>,
        av_clock: Arc<crate::video::clock::AvClock>,
    ) -> Self {
        let clock = MasterClock::with_anchor(ClockAnchor::frozen_at(
            opts.resume_secs.unwrap_or(0.0),
        ));
        let initial_serial = seek_serial.load(Ordering::Acquire);
        Self {
            clock,
            state: EngineState::Idle,
            published_state: Arc::new(AtomicU8::new(state_code::IDLE)),
            duration_secs: None,
            has_audio: false,
            seek_serial,
            av_clock,
            last_observed_serial: initial_serial,
            latch: ReadinessLatch::new(initial_serial),
            last_audio_pts: f64::NEG_INFINITY,
            last_audio_epoch: initial_serial,
            opts,
        }
    }

    /// 外部から `Acquire` で state code を読む public な entry point。
    /// MasterClock を保持する `Arc<MasterClock>` と一緒に外部公開する想定。
    pub fn published_state_code(&self) -> u8 {
        self.published_state.load(Ordering::Acquire)
    }

    /// `published_state` の `Arc<AtomicU8>` を clone で返す (Phase 3d)。
    /// decoder thread が pacing loop 内で state を観察するために使う。
    /// 書き込みは EngineActor 経由のみ (= decoder/audio から store しない)。
    pub fn published_state_handle(&self) -> Arc<AtomicU8> {
        Arc::clone(&self.published_state)
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

    /// 現在の seek 世代を返す。VideoPlayer::tick が「FirstFrameReady を新世代で
    /// 再発火するか」判定するために観察する用途 (Phase 3c)。
    /// 共有 `Arc<AtomicU64>` を `Acquire` で読む。
    pub fn current_seek_epoch(&self) -> SeekEpoch {
        self.seek_serial.load(Ordering::Acquire)
    }

    // ──────────────────────────────────────────────────────────────
    // 状態遷移 helper (= anchor → state の publish 順を強制)
    // ──────────────────────────────────────────────────────────────

    /// `Playing` への遷移。anchor を **先に** 書いてから state を Release で publish。
    /// monotonic guard は anchor の PTS / 現 epoch でリセット (= Playing 入場直後の
    /// AudioRendered がこの anchor 周辺の小さい pts でも受け入れられるように)。
    fn transition_to_playing(&mut self, anchor: ClockAnchor) {
        debug_assert!(
            !matches!(anchor.source, super::clock::ClockSource::Frozen),
            "transition_to_playing requires non-Frozen anchor"
        );
        self.last_audio_pts = anchor.pts_secs;
        self.last_audio_epoch = self.current_seek_epoch();
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
    /// 即 Playing に戻る race を防ぐ。
    fn transition_to_buffering(&mut self, pts: f64) {
        self.latch = ReadinessLatch::new(self.current_seek_epoch());
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
    // command / event handlers (Phase 3a で full 実装)
    // ──────────────────────────────────────────────────────────────

    /// 構築直後の最初の遷移。`Idle → Loading` を実行する (= decoder spawn 直後に呼ぶ)。
    /// resume_secs が指定されていれば anchor を resume 値で凍結する。
    /// `pub` (= video::engine 外から呼べる) にしているのは、VideoPlayer::open が
    /// Phase 3b で本関数を直接呼ぶため。Phase 3d でカプセル化する予定。
    pub fn begin_loading(&mut self) {
        debug_assert_eq!(self.state, EngineState::Idle);
        let initial_pts = self.opts.resume_secs.unwrap_or(0.0);
        self.transition_to_loading(initial_pts);
    }

    /// command 処理。`select!` の cmd lane から呼ぶ。
    /// `pub` (Phase 3e で VideoPlayer の seek 系から Play を発行する用、Phase 4 で
    /// `pub(super)` に戻す)。
    pub fn apply_command(&mut self, cmd: TransportCommand) {
        match cmd {
            TransportCommand::Play => self.handle_play(),
            TransportCommand::Pause => self.handle_pause(),
            TransportCommand::TogglePlay => {
                if matches!(self.state, EngineState::Playing) {
                    self.handle_pause();
                } else {
                    self.handle_play();
                }
            }
            TransportCommand::SeekAbsolute { target_secs } => {
                self.handle_seek_request(target_secs);
            }
            TransportCommand::SeekRelative { delta_secs } => {
                let target = (self.clock.now_secs() + delta_secs).max(0.0);
                self.handle_seek_request(target);
            }
            TransportCommand::SetSpeed { speed } => {
                // Phase 3a: speed は anchor に伝搬しない (= 1.0 固定の前提)。
                // 倍率変更は Phase 4+ で MasterClock への anchor.with_speed() 経由実装予定。
                debug_assert!(
                    speed > 0.0 && speed.is_finite(),
                    "speed must be finite positive, got {speed}"
                );
                let _ = speed;
            }
            TransportCommand::SetVolume { volume: _ }
            | TransportCommand::SetMuted { muted: _ }
            | TransportCommand::SetLoopEnabled { enabled: _ } => {
                // Phase 3a: volume/muted/loop_enabled は EngineActor 状態には影響しない
                // (= TransportController 側で atomic に保持し、audio actor が直接読む)。
                // Phase 3b で配線したときに本 handler は no-op で良い。
            }
            TransportCommand::Shutdown => {
                // run loop 側で扱う (= apply_command の呼び出し元が break する)
            }
        }
    }

    /// `Play` 命令の処理。state によって挙動が変わる:
    /// - Paused → Playing (anchor を audio/wall で再開)
    /// - Eof → Seeking{0} (周回再開、autoplay=true を強制)
    /// - Loading/Buffering/Seeking → autoplay フラグを true に変更 (= 遷移先で Playing)
    /// - Playing → no-op
    fn handle_play(&mut self) {
        // 全ての非 Idle/非 Playing パスで autoplay=true を保証 (= Pause→Seek→READY の
        // 後 Paused に戻ってしまう、EOF Play で seek 後 Paused に戻る、等の防止)。
        self.opts.autoplay = true;
        match self.state {
            EngineState::Paused => {
                // 一時停止中の anchor PTS を起点に再開。
                let pts = self.clock.anchor().pts_secs;
                let anchor = if self.has_audio {
                    ClockAnchor::audio(pts, Instant::now())
                } else {
                    ClockAnchor::wall(pts, Instant::now())
                };
                self.transition_to_playing(anchor);
            }
            EngineState::Eof => {
                // EOF からの Play は周回再開 (= seek 0)
                self.handle_seek_request(0.0);
            }
            EngineState::Loading | EngineState::Buffering | EngineState::Seeking { .. } => {
                // 遷移途中: autoplay=true 設定済 → Buffering 完了時に Playing
            }
            EngineState::Idle | EngineState::Playing => {
                // Idle: open() を経ていない不正な呼び出し → no-op (autoplay 設定だけ
                //   有効、後の begin_loading で尊重される)
                // Playing: 既に再生中なので何もしない
            }
        }
    }

    /// `Pause` 命令の処理。
    /// 全ての非 Eof パスで autoplay=false を保証 (= Playing 中の Pause 後に Seek が
    /// 来ても、Buffering 完了時に Playing に戻らないようにする)。
    fn handle_pause(&mut self) {
        self.opts.autoplay = false;
        match self.state {
            EngineState::Playing => {
                // 現在の再生位置を凍結
                let pts = self.clock.now_secs();
                self.transition_to_paused(pts);
            }
            EngineState::Loading | EngineState::Buffering | EngineState::Seeking { .. } => {
                // 遷移途中: autoplay=false 設定済 → Buffering 完了時に Paused
            }
            EngineState::Idle | EngineState::Paused | EngineState::Eof => {
                // 既に停止状態 → no-op
            }
        }
    }

    /// シーク要求を受けて state を Seeking に遷移させる。**adaptive bump**:
    /// - 共有カウンタが `last_observed_serial` より進んでいる場合 (= 外部経路、
    ///   `clock.request_seek` で既に bump 済) → 観測のみで bump しない。
    ///   decoder には `clock.request_seek` 経路で既に SeekRequest が publish 済。
    /// - 進んでいない場合 (= 内部経路、loop replay / EOF replay / resume) →
    ///   `av_clock.request_seek(target)` を呼び、それが内部で fetch_add(1) して
    ///   SeekRequest を decoder に publish する。本関数は state 更新だけを担当。
    ///
    /// この設計で **「双方が独立に bump する」古い API パターン (= Codex P2 の二重 ++
    /// バグ源)** を構造的に排除した。caller は `mod.rs::seek` 系で従来通り
    /// `clock.request_seek` → `engine.handle_seek_request` の順に呼べばよい。
    /// `pub` (mod.rs から呼ぶ、Phase 4 で apply_command 経由 channel に置き換え予定)。
    pub fn handle_seek_request(&mut self, target_secs: f64) {
        let target = if let Some(d) = self.duration_secs {
            target_secs.clamp(0.0, d)
        } else {
            target_secs.max(0.0)
        };
        let observed = self.seek_serial.load(Ordering::Acquire);
        if observed <= self.last_observed_serial {
            // 内部経路: 自分で bump して clock 経由で decoder に publish。
            // av_clock.request_seek が fetch_add(1) → SeekRequest を Mutex に push し、
            // decoder の `take_seek_request` で消費される。
            self.av_clock.request_seek(target);
        }
        // どちらの経路でも、ここで最新値を読む (= bump 後の値)。
        let new_epoch = self.seek_serial.load(Ordering::Acquire);
        self.last_observed_serial = new_epoch;
        self.latch = ReadinessLatch::new(new_epoch);
        self.transition_to_seeking(target);
    }

    /// decoder event 処理。
    /// stale (`ev.epoch < current_seek_epoch`) は捨てる。
    /// `pub` (Phase 3c で VideoPlayer から呼ぶ、Phase 4 で engine 主導 run loop に
    /// 移管したら `pub(super)` に戻す)。
    pub fn handle_decoder_event(&mut self, ev: DecoderEvent) {
        match ev {
            DecoderEvent::InfoReceived {
                epoch: _,
                duration_secs,
                has_audio,
            } => {
                // metadata は **epoch 関係なく常に保存** する (= pre-info user seek が
                // 走った場合でも duration/has_audio は捨てない)。
                self.duration_secs = Some(duration_secs);
                self.has_audio = has_audio;
                // 状態遷移は state=Loading のときだけ行う。
                // pre-info で user seek が走って既に Seeking に入っている場合は、
                // resume_secs を消費せずに残し、次回ファイル open でも作用させない。
                if !matches!(self.state, EngineState::Loading) {
                    return;
                }
                // resume が指定されていれば、open path 内で seek を発火する。
                // resume_secs を一度消費して以降の再 InfoReceived では発火しない。
                if let Some(resume) = self.opts.resume_secs.take() {
                    let near_end = duration_secs > 0.0
                        && resume >= duration_secs - VIDEO_RESUME_END_GUARD_SECS;
                    let safe = resume >= VIDEO_RESUME_MIN_POSITION_SECS && !near_end;
                    if safe {
                        // Loading → Seeking{resume} へ。epoch++ + latch reset を伴う。
                        self.handle_seek_request(resume);
                        return;
                    }
                }
                // resume 不要ケース: Loading → Buffering (preroll、READY を待つ)
                self.transition_to_buffering(0.0);
            }
            DecoderEvent::SeekCompleted { epoch, actual_pts } => {
                if epoch < self.current_seek_epoch() {
                    return;
                }
                // Seeking → Buffering (preroll、READY を待つ)
                // **epoch++ しない** (= handle_seek_request で既に進めた値、設計 v4)
                self.transition_to_buffering(actual_pts);
            }
            DecoderEvent::FirstFrameReady { epoch, pts } => {
                if epoch < self.current_seek_epoch() {
                    return;
                }
                if epoch > self.latch.epoch {
                    // 新世代: latch を再 reset (= handle_seek_request 後に届いた最初の event)
                    self.latch = ReadinessLatch::new(epoch);
                }
                self.latch.first_frame = true;
                self.latch.first_frame_pts = Some(pts);
                self.try_transition_from_buffering();
            }
            DecoderEvent::EofReached {
                epoch,
                duration_secs,
            } => {
                if epoch < self.current_seek_epoch() {
                    return;
                }
                if self.opts.loop_enabled {
                    // 周回再生: 0 へ seek
                    self.handle_seek_request(0.0);
                } else {
                    // 通常 EOF
                    self.transition_to_eof(duration_secs);
                }
            }
            DecoderEvent::Failed { reason: _ } => {
                // 致命的エラー: state を Idle に戻す (run loop は別途 channel close で抜ける)
                self.transition_to_loading(self.clock.anchor().pts_secs);
                self.state = EngineState::Idle;
                self.published_state.store(state_code::IDLE, Ordering::Release);
            }
        }
    }

    /// audio event 処理。
    /// `pub` (Phase 3c で VideoPlayer から呼ぶ、Phase 4 で engine 主導 run loop に
    /// 移管したら `pub(super)` に戻す)。
    pub fn handle_audio_event(&mut self, ev: AudioEvent) {
        match ev {
            AudioEvent::AudioRendered { epoch, pts, wall_now } => {
                if epoch < self.current_seek_epoch() {
                    return;
                }
                // monotonic guard と anchor 更新は **Playing 中のみ** 行う。
                // Buffering/Seeking/Paused 中の AudioRendered は cpal 先読み再生で
                // 届くが、Playing 入場時に anchor を BufferReady で再設定するため、
                // ここで guard を進めると Playing 入場直後に有効な小さい pts が捨て
                // られる (= clock が pre-Playing の pts に張り付く) 不具合になる。
                if !matches!(self.state, EngineState::Playing) {
                    return;
                }
                if epoch > self.last_audio_epoch {
                    // 新世代の最初のサンプル: monotonic guard を reset
                    self.last_audio_epoch = epoch;
                    self.last_audio_pts = pts;
                } else if pts < self.last_audio_pts {
                    return; // 同世代内で後退する callback は捨てる
                } else {
                    self.last_audio_pts = pts;
                }
                self.clock.set_anchor(ClockAnchor::audio(pts, wall_now));
            }
            AudioEvent::BufferReady { epoch, pts, wall_now } => {
                if epoch < self.current_seek_epoch() {
                    return;
                }
                if epoch > self.latch.epoch {
                    self.latch = ReadinessLatch::new(epoch);
                }
                self.latch.buffer_ready = true;
                self.latch.audio_anchor = Some((pts, wall_now));
                self.try_transition_from_buffering();
            }
            AudioEvent::BufferStarved { epoch } => {
                if epoch < self.current_seek_epoch() {
                    return;
                }
                // Playing 中で audio が枯渇したら Buffering に戻して再 preroll。
                // latch も自動 reset (= transition_to_buffering 内で実施)。
                if matches!(self.state, EngineState::Playing) {
                    let pts = self.clock.now_secs();
                    self.transition_to_buffering(pts);
                }
            }
            AudioEvent::AudioInactive => {
                // audio 出力起動失敗 → wall master に変更
                self.has_audio = false;
                // Buffering 中なら latch 再評価 (= has_audio=false で latch.is_ready が
                // first_frame だけで true になる可能性)
                if matches!(self.state, EngineState::Buffering) {
                    self.try_transition_from_buffering();
                }
            }
        }
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
        // is_ready が true なら Option は必ず Some (= 改訂 is_ready で保証、設計 v3 P2)。
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
}

/// 末尾近く (残り 5 秒以下) の resume は無視する境界 (= 完走済みとみなす)。
/// 既存 `crate::app::VIDEO_RESUME_END_GUARD_SECS` と同値で揃える (Phase 3b で配線)。
const VIDEO_RESUME_END_GUARD_SECS: f64 = 5.0;
/// resume が小さすぎ (動画開始直後) の場合は無視する閾値。
const VIDEO_RESUME_MIN_POSITION_SECS: f64 = 1.0;

#[cfg(test)]
mod tests {
    use super::*;
    use super::super::clock::ClockSource;
    use crate::video::clock::AvClock;

    /// テスト用 EngineActor 構築ヘルパ。
    /// 共有 `seek_serial` と `AvClock` を組み立てる (= 本番の `VideoPlayer::open` と
    /// 同じ依存関係)。テストでは `seek_serial` を test 内から直接観察したい場合があるが、
    /// `actor.current_seek_epoch()` (= `seek_serial.load()`) で同等に読める。
    fn fresh_actor_with_opts(opts: OpenOptions) -> EngineActor {
        let seek_serial = Arc::new(AtomicU64::new(0));
        let initial_volume = opts.initial_volume;
        let av_clock = Arc::new(AvClock::new(initial_volume, seek_serial.clone()));
        EngineActor::new(opts, seek_serial, av_clock)
    }

    fn fresh_actor() -> EngineActor {
        fresh_actor_with_opts(OpenOptions::default())
    }

    #[test]
    fn new_actor_is_idle_with_resume_zero() {
        let a = fresh_actor();
        assert_eq!(a.state, EngineState::Idle);
        assert_eq!(a.published_state_code(), state_code::IDLE);
        assert_eq!(a.current_seek_epoch(), 0);
        assert!((a.clock().now_secs() - 0.0).abs() < 1e-9);
        assert_eq!(a.clock().anchor().source, ClockSource::Frozen);
    }

    #[test]
    fn new_actor_with_resume_initializes_clock() {
        let opts = OpenOptions {
            resume_secs: Some(42.5),
            ..Default::default()
        };
        let a = fresh_actor_with_opts(opts);
        assert!((a.clock().now_secs() - 42.5).abs() < 1e-9);
    }

    #[test]
    fn handle_seek_request_advances_epoch_once() {
        let mut a = fresh_actor();
        assert_eq!(a.current_seek_epoch(), 0);
        a.handle_seek_request(10.0);
        assert_eq!(a.current_seek_epoch(), 1);
        assert_eq!(a.state, EngineState::Seeking { target_secs: 10.0 });
        assert_eq!(a.latch.epoch, 1);
        // anchor は target で frozen
        assert!((a.clock().now_secs() - 10.0).abs() < 1e-9);

        a.handle_seek_request(20.0);
        assert_eq!(a.current_seek_epoch(), 2);
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
        assert_eq!(a.current_seek_epoch(), 1, "Buffering must not advance epoch");
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
        let mut a = fresh_actor_with_opts(OpenOptions {
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

    // ──────────────────────────────────────────────────────────────
    // Phase 3a: full handler tests
    // ──────────────────────────────────────────────────────────────

    fn fresh_with_resume(secs: f64, autoplay: bool) -> EngineActor {
        fresh_actor_with_opts(OpenOptions {
            resume_secs: Some(secs),
            autoplay,
            ..Default::default()
        })
    }

    #[test]
    fn begin_loading_transitions_to_loading() {
        let mut a = fresh_actor();
        a.begin_loading();
        assert_eq!(a.state, EngineState::Loading);
        assert_eq!(a.published_state_code(), state_code::LOADING);
        assert!((a.clock().now_secs() - 0.0).abs() < 1e-9);
    }

    #[test]
    fn open_path_no_resume_no_audio_reaches_playing() {
        let mut a = fresh_actor();
        a.begin_loading();
        a.handle_decoder_event(DecoderEvent::InfoReceived {
            epoch: 0,
            duration_secs: 30.0,
            has_audio: false,
        });
        // resume なし → Loading → Buffering
        assert_eq!(a.state, EngineState::Buffering);
        // first_frame で latch 完成 → Playing (autoplay=true、has_audio=false)
        a.handle_decoder_event(DecoderEvent::FirstFrameReady { epoch: 0, pts: 0.0 });
        assert_eq!(a.state, EngineState::Playing);
        assert_eq!(a.clock().anchor().source, ClockSource::Wall);
    }

    #[test]
    fn open_path_with_audio_waits_for_buffer_ready() {
        let mut a = fresh_actor();
        a.begin_loading();
        a.handle_decoder_event(DecoderEvent::InfoReceived {
            epoch: 0,
            duration_secs: 30.0,
            has_audio: true,
        });
        a.handle_decoder_event(DecoderEvent::FirstFrameReady { epoch: 0, pts: 0.0 });
        // first_frame だけでは Playing には行かない (buffer_ready 待ち)
        assert_eq!(a.state, EngineState::Buffering);

        a.handle_audio_event(AudioEvent::BufferReady {
            epoch: 0,
            pts: 0.05,
            wall_now: Instant::now(),
        });
        assert_eq!(a.state, EngineState::Playing);
        assert_eq!(a.clock().anchor().source, ClockSource::Audio);
    }

    #[test]
    fn open_path_with_resume_triggers_seek() {
        let mut a = fresh_with_resume(15.0, true);
        a.begin_loading();
        a.handle_decoder_event(DecoderEvent::InfoReceived {
            epoch: 0,
            duration_secs: 30.0,
            has_audio: true,
        });
        // resume が消費されて Seeking に遷移
        assert_eq!(a.state, EngineState::Seeking { target_secs: 15.0 });
        assert_eq!(a.current_seek_epoch(), 1);

        // SeekCompleted で Buffering に
        a.handle_decoder_event(DecoderEvent::SeekCompleted {
            epoch: 1,
            actual_pts: 15.02,
        });
        assert_eq!(a.state, EngineState::Buffering);

        // FirstFrameReady (epoch=1) と BufferReady で Playing
        a.handle_decoder_event(DecoderEvent::FirstFrameReady { epoch: 1, pts: 15.02 });
        a.handle_audio_event(AudioEvent::BufferReady {
            epoch: 1,
            pts: 15.05,
            wall_now: Instant::now(),
        });
        assert_eq!(a.state, EngineState::Playing);
    }

    #[test]
    fn resume_too_close_to_end_is_ignored() {
        let mut a = fresh_with_resume(28.0, true);
        a.begin_loading();
        a.handle_decoder_event(DecoderEvent::InfoReceived {
            epoch: 0,
            duration_secs: 30.0,
            has_audio: false,
        });
        // resume=28 > duration=30 - 5 (= END_GUARD) → 末尾近くなので無視 → 通常 Buffering
        assert_eq!(a.state, EngineState::Buffering);
        assert_eq!(a.current_seek_epoch(), 0, "no seek consumed");
    }

    #[test]
    fn resume_too_small_is_ignored() {
        let mut a = fresh_with_resume(0.5, true);
        a.begin_loading();
        a.handle_decoder_event(DecoderEvent::InfoReceived {
            epoch: 0,
            duration_secs: 30.0,
            has_audio: false,
        });
        // resume=0.5 < MIN(1.0) → 無視 → 通常 Buffering
        assert_eq!(a.state, EngineState::Buffering);
        assert_eq!(a.current_seek_epoch(), 0);
    }

    #[test]
    fn stale_decoder_event_dropped_after_seek() {
        let mut a = fresh_actor();
        a.begin_loading();
        a.handle_decoder_event(DecoderEvent::InfoReceived {
            epoch: 0,
            duration_secs: 30.0,
            has_audio: false,
        });
        // ユーザーが手動 seek
        a.handle_seek_request(10.0);
        assert_eq!(a.current_seek_epoch(), 1);

        // 古い epoch=0 の FirstFrameReady が遅れて届く → 無視されるべき
        a.handle_decoder_event(DecoderEvent::FirstFrameReady { epoch: 0, pts: 0.0 });
        assert_eq!(a.state, EngineState::Seeking { target_secs: 10.0 });
        assert!(!a.latch.first_frame, "stale event must not satisfy latch");
    }

    #[test]
    fn audio_rendered_only_updates_anchor_in_playing() {
        let mut a = fresh_actor();
        a.has_audio = true;
        a.begin_loading();
        a.handle_decoder_event(DecoderEvent::InfoReceived {
            epoch: 0,
            duration_secs: 30.0,
            has_audio: true,
        });
        // Buffering 中: AudioRendered が来ても anchor は Frozen のまま
        a.handle_audio_event(AudioEvent::AudioRendered {
            epoch: 0,
            pts: 1.0,
            wall_now: Instant::now(),
        });
        assert_eq!(a.clock().anchor().source, ClockSource::Frozen);

        // latch を埋めて Playing に遷移
        a.handle_decoder_event(DecoderEvent::FirstFrameReady { epoch: 0, pts: 0.0 });
        a.handle_audio_event(AudioEvent::BufferReady {
            epoch: 0,
            pts: 0.05,
            wall_now: Instant::now(),
        });
        assert_eq!(a.state, EngineState::Playing);

        // Playing 中の AudioRendered は anchor を更新
        let new_wall = Instant::now();
        a.handle_audio_event(AudioEvent::AudioRendered {
            epoch: 0,
            pts: 0.10,
            wall_now: new_wall,
        });
        assert_eq!(a.clock().anchor().source, ClockSource::Audio);
        assert!((a.clock().anchor().pts_secs - 0.10).abs() < 1e-9);
    }

    #[test]
    fn audio_rendered_monotonic_guard_drops_backward() {
        let mut a = fresh_actor();
        a.has_audio = true;
        a.begin_loading();
        a.handle_decoder_event(DecoderEvent::InfoReceived {
            epoch: 0,
            duration_secs: 30.0,
            has_audio: true,
        });
        a.handle_decoder_event(DecoderEvent::FirstFrameReady { epoch: 0, pts: 0.0 });
        a.handle_audio_event(AudioEvent::BufferReady {
            epoch: 0,
            pts: 0.05,
            wall_now: Instant::now(),
        });

        a.handle_audio_event(AudioEvent::AudioRendered {
            epoch: 0,
            pts: 1.0,
            wall_now: Instant::now(),
        });
        let anchor_before = a.clock().anchor().pts_secs;

        // backward な pts は捨てられる
        a.handle_audio_event(AudioEvent::AudioRendered {
            epoch: 0,
            pts: 0.5,
            wall_now: Instant::now(),
        });
        let anchor_after = a.clock().anchor().pts_secs;
        assert!((anchor_before - anchor_after).abs() < 1e-9,
                "backward audio pts must not regress anchor");
    }

    #[test]
    fn audio_rendered_after_seek_resets_guard() {
        let mut a = fresh_actor();
        a.has_audio = true;
        a.begin_loading();
        a.handle_decoder_event(DecoderEvent::InfoReceived {
            epoch: 0,
            duration_secs: 60.0,
            has_audio: true,
        });
        a.handle_decoder_event(DecoderEvent::FirstFrameReady { epoch: 0, pts: 0.0 });
        a.handle_audio_event(AudioEvent::BufferReady {
            epoch: 0,
            pts: 0.05,
            wall_now: Instant::now(),
        });
        a.handle_audio_event(AudioEvent::AudioRendered {
            epoch: 0,
            pts: 30.0,
            wall_now: Instant::now(),
        });

        // backward seek 5.0
        a.handle_seek_request(5.0);
        a.handle_decoder_event(DecoderEvent::SeekCompleted {
            epoch: 1,
            actual_pts: 5.0,
        });
        a.handle_decoder_event(DecoderEvent::FirstFrameReady { epoch: 1, pts: 5.0 });
        a.handle_audio_event(AudioEvent::BufferReady {
            epoch: 1,
            pts: 5.05,
            wall_now: Instant::now(),
        });
        assert_eq!(a.state, EngineState::Playing);

        // 新世代の AudioRendered (= pts=5.10) が **30.0 より小さくても** 受け入れられる
        a.handle_audio_event(AudioEvent::AudioRendered {
            epoch: 1,
            pts: 5.10,
            wall_now: Instant::now(),
        });
        assert!((a.clock().anchor().pts_secs - 5.10).abs() < 1e-9,
                "new-epoch audio must reset monotonic guard");
    }

    #[test]
    fn pause_during_playing_freezes_clock() {
        let mut a = fresh_actor();
        a.begin_loading();
        a.handle_decoder_event(DecoderEvent::InfoReceived {
            epoch: 0,
            duration_secs: 30.0,
            has_audio: false,
        });
        a.handle_decoder_event(DecoderEvent::FirstFrameReady { epoch: 0, pts: 0.0 });
        assert_eq!(a.state, EngineState::Playing);

        a.apply_command(TransportCommand::Pause);
        assert_eq!(a.state, EngineState::Paused);
        assert_eq!(a.clock().anchor().source, ClockSource::Frozen);
    }

    #[test]
    fn play_during_paused_resumes_anchor() {
        let mut a = fresh_actor();
        a.begin_loading();
        a.handle_decoder_event(DecoderEvent::InfoReceived {
            epoch: 0,
            duration_secs: 30.0,
            has_audio: false,
        });
        a.handle_decoder_event(DecoderEvent::FirstFrameReady { epoch: 0, pts: 0.0 });
        a.apply_command(TransportCommand::Pause);
        assert_eq!(a.state, EngineState::Paused);

        a.apply_command(TransportCommand::Play);
        assert_eq!(a.state, EngineState::Playing);
        assert_eq!(a.clock().anchor().source, ClockSource::Wall);
    }

    #[test]
    fn play_during_buffering_sets_autoplay_true() {
        let mut a = fresh_actor_with_opts(OpenOptions {
            autoplay: false,
            ..Default::default()
        });
        a.begin_loading();
        a.handle_decoder_event(DecoderEvent::InfoReceived {
            epoch: 0,
            duration_secs: 30.0,
            has_audio: false,
        });
        // Buffering、autoplay=false
        assert_eq!(a.state, EngineState::Buffering);
        a.apply_command(TransportCommand::Play);
        assert!(a.opts.autoplay);

        // first_frame で Playing に到達 (autoplay=true 効く)
        a.handle_decoder_event(DecoderEvent::FirstFrameReady { epoch: 0, pts: 0.0 });
        assert_eq!(a.state, EngineState::Playing);
    }

    #[test]
    fn pause_during_buffering_sets_autoplay_false() {
        let mut a = fresh_actor();
        a.begin_loading();
        a.handle_decoder_event(DecoderEvent::InfoReceived {
            epoch: 0,
            duration_secs: 30.0,
            has_audio: false,
        });
        assert_eq!(a.state, EngineState::Buffering);
        a.apply_command(TransportCommand::Pause);
        assert!(!a.opts.autoplay);

        a.handle_decoder_event(DecoderEvent::FirstFrameReady { epoch: 0, pts: 0.0 });
        assert_eq!(a.state, EngineState::Paused);
    }

    #[test]
    fn eof_with_loop_disabled_transitions_to_eof() {
        let mut a = fresh_actor();
        a.begin_loading();
        a.handle_decoder_event(DecoderEvent::InfoReceived {
            epoch: 0,
            duration_secs: 30.0,
            has_audio: false,
        });
        a.handle_decoder_event(DecoderEvent::FirstFrameReady { epoch: 0, pts: 0.0 });
        a.handle_decoder_event(DecoderEvent::EofReached {
            epoch: 0,
            duration_secs: 30.0,
        });
        assert_eq!(a.state, EngineState::Eof);
        assert!((a.clock().anchor().pts_secs - 30.0).abs() < 1e-9);
    }

    #[test]
    fn eof_with_loop_enabled_seeks_to_zero() {
        let mut a = fresh_actor_with_opts(OpenOptions {
            loop_enabled: true,
            ..Default::default()
        });
        a.begin_loading();
        a.handle_decoder_event(DecoderEvent::InfoReceived {
            epoch: 0,
            duration_secs: 30.0,
            has_audio: false,
        });
        a.handle_decoder_event(DecoderEvent::FirstFrameReady { epoch: 0, pts: 0.0 });
        a.handle_decoder_event(DecoderEvent::EofReached {
            epoch: 0,
            duration_secs: 30.0,
        });
        assert_eq!(a.state, EngineState::Seeking { target_secs: 0.0 });
        assert_eq!(a.current_seek_epoch(), 1);
    }

    #[test]
    fn play_during_eof_seeks_to_zero() {
        let mut a = fresh_actor();
        a.begin_loading();
        a.handle_decoder_event(DecoderEvent::InfoReceived {
            epoch: 0,
            duration_secs: 30.0,
            has_audio: false,
        });
        a.handle_decoder_event(DecoderEvent::FirstFrameReady { epoch: 0, pts: 0.0 });
        a.handle_decoder_event(DecoderEvent::EofReached {
            epoch: 0,
            duration_secs: 30.0,
        });
        assert_eq!(a.state, EngineState::Eof);

        a.apply_command(TransportCommand::Play);
        assert_eq!(a.state, EngineState::Seeking { target_secs: 0.0 });
    }

    #[test]
    fn buffer_starved_returns_to_buffering() {
        let mut a = fresh_actor();
        a.has_audio = true;
        a.begin_loading();
        a.handle_decoder_event(DecoderEvent::InfoReceived {
            epoch: 0,
            duration_secs: 30.0,
            has_audio: true,
        });
        a.handle_decoder_event(DecoderEvent::FirstFrameReady { epoch: 0, pts: 0.0 });
        a.handle_audio_event(AudioEvent::BufferReady {
            epoch: 0,
            pts: 0.05,
            wall_now: Instant::now(),
        });
        assert_eq!(a.state, EngineState::Playing);

        a.handle_audio_event(AudioEvent::BufferStarved { epoch: 0 });
        assert_eq!(a.state, EngineState::Buffering);
        assert!(!a.latch.first_frame, "latch reset on starvation");
    }

    #[test]
    fn audio_inactive_during_buffering_completes_latch() {
        let mut a = fresh_actor();
        a.has_audio = true;
        a.begin_loading();
        a.handle_decoder_event(DecoderEvent::InfoReceived {
            epoch: 0,
            duration_secs: 30.0,
            has_audio: true,
        });
        a.handle_decoder_event(DecoderEvent::FirstFrameReady { epoch: 0, pts: 0.0 });
        // first_frame だけでは Playing に行かない
        assert_eq!(a.state, EngineState::Buffering);

        // audio 出力起動失敗 → has_audio=false で latch 再評価 → Playing
        a.handle_audio_event(AudioEvent::AudioInactive);
        assert!(!a.has_audio);
        assert_eq!(a.state, EngineState::Playing);
        assert_eq!(a.clock().anchor().source, ClockSource::Wall);
    }

    #[test]
    fn seek_relative_uses_current_position() {
        let mut a = fresh_actor();
        a.begin_loading();
        a.handle_decoder_event(DecoderEvent::InfoReceived {
            epoch: 0,
            duration_secs: 30.0,
            has_audio: false,
        });
        a.handle_decoder_event(DecoderEvent::FirstFrameReady { epoch: 0, pts: 5.0 });
        // anchor は Wall 5.0 になっている。+10 で seek
        a.apply_command(TransportCommand::SeekRelative { delta_secs: 10.0 });
        assert!(matches!(a.state, EngineState::Seeking { target_secs }
            if (target_secs - 15.0).abs() < 0.5));
    }

    #[test]
    fn pause_then_seek_keeps_paused_after_ready() {
        // Playing → Pause → Seek → READY で Paused 状態を維持する不変条件のテスト
        let mut a = fresh_actor();
        a.begin_loading();
        a.handle_decoder_event(DecoderEvent::InfoReceived {
            epoch: 0,
            duration_secs: 60.0,
            has_audio: false,
        });
        a.handle_decoder_event(DecoderEvent::FirstFrameReady { epoch: 0, pts: 0.0 });
        assert_eq!(a.state, EngineState::Playing);

        a.apply_command(TransportCommand::Pause);
        assert_eq!(a.state, EngineState::Paused);
        assert!(!a.opts.autoplay);

        a.apply_command(TransportCommand::SeekAbsolute { target_secs: 30.0 });
        a.handle_decoder_event(DecoderEvent::SeekCompleted {
            epoch: 1,
            actual_pts: 30.0,
        });
        a.handle_decoder_event(DecoderEvent::FirstFrameReady { epoch: 1, pts: 30.0 });
        // autoplay=false のままなので Paused に行く
        assert_eq!(a.state, EngineState::Paused);
    }

    #[test]
    fn play_from_eof_forces_autoplay_true() {
        // Pause で autoplay=false の状態でも、Eof Play は再生再開を強制する不変条件のテスト
        let mut a = fresh_actor();
        a.begin_loading();
        a.handle_decoder_event(DecoderEvent::InfoReceived {
            epoch: 0,
            duration_secs: 30.0,
            has_audio: false,
        });
        a.handle_decoder_event(DecoderEvent::FirstFrameReady { epoch: 0, pts: 0.0 });
        a.apply_command(TransportCommand::Pause);
        a.handle_decoder_event(DecoderEvent::EofReached {
            epoch: 0,
            duration_secs: 30.0,
        });
        // Eof 状態 (Pause からは Eof に行かないが、長時間放置すれば EOF が来る前提のテスト)
        // ここでは EOF を強制注入して状態確認
        // 実際の挙動: Paused → EofReached でも Eof に遷移する
        assert_eq!(a.state, EngineState::Eof);

        // Play → autoplay=true 強制 → Seeking{0}
        a.apply_command(TransportCommand::Play);
        assert_eq!(a.state, EngineState::Seeking { target_secs: 0.0 });
        assert!(a.opts.autoplay);

        // SeekCompleted + READY → Playing (Paused に戻らない)
        a.handle_decoder_event(DecoderEvent::SeekCompleted {
            epoch: 1,
            actual_pts: 0.0,
        });
        a.handle_decoder_event(DecoderEvent::FirstFrameReady { epoch: 1, pts: 0.0 });
        assert_eq!(a.state, EngineState::Playing);
    }

    #[test]
    fn info_received_metadata_saved_even_if_pre_info_seek() {
        // pre-info で user seek が走っても duration/has_audio は保存される不変条件のテスト
        let mut a = fresh_actor();
        a.begin_loading();
        // info 到着前に user seek (epoch++)
        a.apply_command(TransportCommand::SeekAbsolute { target_secs: 5.0 });
        assert_eq!(a.current_seek_epoch(), 1);
        assert_eq!(a.state, EngineState::Seeking { target_secs: 5.0 });

        // 古い epoch=0 の InfoReceived が遅れて届く → state 遷移はしないが
        // metadata は保存される
        a.handle_decoder_event(DecoderEvent::InfoReceived {
            epoch: 0,
            duration_secs: 45.0,
            has_audio: true,
        });
        assert_eq!(a.duration_secs, Some(45.0), "duration must be saved");
        assert!(a.has_audio, "has_audio must be saved");
        assert_eq!(a.state, EngineState::Seeking { target_secs: 5.0 }, "state unchanged");
    }

    #[test]
    fn seek_clamps_to_duration() {
        let mut a = fresh_actor();
        a.begin_loading();
        a.handle_decoder_event(DecoderEvent::InfoReceived {
            epoch: 0,
            duration_secs: 30.0,
            has_audio: false,
        });
        a.apply_command(TransportCommand::SeekAbsolute { target_secs: 100.0 });
        assert_eq!(a.state, EngineState::Seeking { target_secs: 30.0 });

        a.apply_command(TransportCommand::SeekAbsolute { target_secs: -5.0 });
        assert_eq!(a.state, EngineState::Seeking { target_secs: 0.0 });
    }

    // ──────────────────────────────────────────────────────────────
    // Phase 9 + Codex P2 反映後の不変条件テスト群
    // (= 「epoch++ は handle_seek_request の 1 箇所のみ」を保証する。
    //   Phase 9 シリーズで何度か破った invariant なので、回帰防止としてここに固める。)
    // ──────────────────────────────────────────────────────────────

    /// handle_play が **Seeking 状態** で呼ばれたとき、epoch を ++ せず autoplay=true
    /// だけ立てることを保証 (Codex P2 修正の核心)。
    /// この invariant を破ると、`handle_seek_request → apply_command(Play)` 順の
    /// `seek` / `seek_relative` / `toggle_play` EOF replay / loop replay が epoch を
    /// 二重 ++ し、decoder からの `SeekCompleted{epoch=serial}` が stale 判定で捨てられ
    /// engine が Seeking に張り付く。
    #[test]
    fn handle_play_in_seeking_state_does_not_advance_epoch() {
        let mut a = fresh_actor();
        a.begin_loading();
        a.handle_decoder_event(DecoderEvent::InfoReceived {
            epoch: 0,
            duration_secs: 30.0,
            has_audio: false,
        });

        // handle_seek_request で epoch=1 / state=Seeking に
        a.handle_seek_request(10.0);
        assert_eq!(a.current_seek_epoch(), 1);
        assert_eq!(a.state, EngineState::Seeking { target_secs: 10.0 });
        let opts_autoplay_before = a.opts.autoplay;

        // apply_command(Play) → handle_play は Seeking arm を通る
        a.apply_command(TransportCommand::Play);
        assert_eq!(
            a.current_seek_epoch(), 1,
            "epoch must not advance — Seeking arm only sets autoplay"
        );
        assert_eq!(a.state, EngineState::Seeking { target_secs: 10.0 });
        // autoplay=true が立つ (元が true のときも true のまま)
        assert!(a.opts.autoplay);
        let _ = opts_autoplay_before;
    }

    /// handle_play が **Eof 状態** で呼ばれたとき、内部で handle_seek_request(0.0)
    /// を呼んで epoch を ++ することを保証 (= Eof 専用の自動 replay 仕様)。
    /// この振る舞いが「呼び出し順注意 (handle_seek_request → apply_command(Play))」
    /// 規約の根拠 — 先に apply_command(Play) を呼ぶと Eof arm がここで epoch++ し、
    /// 続く明示 handle_seek_request で更に ++ するので二重 ++ 問題が起きる。
    #[test]
    fn handle_play_in_eof_state_advances_epoch_once_via_internal_seek() {
        let mut a = fresh_actor();
        a.begin_loading();
        a.handle_decoder_event(DecoderEvent::InfoReceived {
            epoch: 0,
            duration_secs: 30.0,
            has_audio: false,
        });
        a.handle_decoder_event(DecoderEvent::FirstFrameReady { epoch: 0, pts: 0.0 });
        a.handle_decoder_event(DecoderEvent::EofReached {
            epoch: 0,
            duration_secs: 30.0,
        });
        assert_eq!(a.state, EngineState::Eof);
        assert_eq!(a.current_seek_epoch(), 0);

        a.apply_command(TransportCommand::Play);
        // 内部で handle_seek_request(0.0) が呼ばれて epoch=1
        assert_eq!(
            a.current_seek_epoch(), 1,
            "Eof + Play は handle_seek_request(0.0) を内部呼びして epoch++ する"
        );
        assert_eq!(a.state, EngineState::Seeking { target_secs: 0.0 });
    }

    /// 正しい呼び出し順 (mod.rs::seek / seek_relative / toggle_play / loop replay):
    /// handle_seek_request(target) → apply_command(Play) で epoch がちょうど 1 進む。
    /// state が Eof でも Paused でも Playing でも同じ。
    #[test]
    fn seek_then_play_advances_epoch_exactly_once_from_any_state() {
        // ケース 1: Idle (= 開いた直後、まだ begin_loading 前)
        {
            let mut a = fresh_actor();
            a.handle_seek_request(5.0);
            a.apply_command(TransportCommand::Play);
            assert_eq!(a.current_seek_epoch(), 1, "Idle: epoch=1");
        }
        // ケース 2: Loading
        {
            let mut a = fresh_actor();
            a.begin_loading();
            a.handle_seek_request(5.0);
            a.apply_command(TransportCommand::Play);
            assert_eq!(a.current_seek_epoch(), 1, "Loading: epoch=1");
        }
        // ケース 3: Paused (Playing 経由)
        {
            let mut a = fresh_actor();
            a.begin_loading();
            a.handle_decoder_event(DecoderEvent::InfoReceived {
                epoch: 0,
                duration_secs: 30.0,
                has_audio: false,
            });
            a.handle_decoder_event(DecoderEvent::FirstFrameReady { epoch: 0, pts: 0.0 });
            a.apply_command(TransportCommand::Pause);
            assert_eq!(a.state, EngineState::Paused);
            a.handle_seek_request(10.0);
            a.apply_command(TransportCommand::Play);
            assert_eq!(a.current_seek_epoch(), 1, "Paused: epoch=1");
        }
        // ケース 4: Eof
        {
            let mut a = fresh_actor();
            a.begin_loading();
            a.handle_decoder_event(DecoderEvent::InfoReceived {
                epoch: 0,
                duration_secs: 30.0,
                has_audio: false,
            });
            a.handle_decoder_event(DecoderEvent::FirstFrameReady { epoch: 0, pts: 0.0 });
            a.handle_decoder_event(DecoderEvent::EofReached {
                epoch: 0,
                duration_secs: 30.0,
            });
            // 正しい順: handle_seek_request → apply_command(Play)
            a.handle_seek_request(0.0);
            assert_eq!(a.current_seek_epoch(), 1);
            assert_eq!(a.state, EngineState::Seeking { target_secs: 0.0 });
            a.apply_command(TransportCommand::Play);
            // handle_play が Seeking arm を通って autoplay=true セットのみ → epoch=1 維持
            assert_eq!(
                a.current_seek_epoch(), 1,
                "Eof: handle_seek_request → apply_command(Play) の順なら epoch=1"
            );
        }
    }

    /// 連続 seek 要求は各回ごとにきっちり epoch が +1 される (= バースト連打の race なし)。
    #[test]
    fn successive_seeks_each_advance_epoch_by_one() {
        let mut a = fresh_actor();
        a.begin_loading();
        a.handle_decoder_event(DecoderEvent::InfoReceived {
            epoch: 0,
            duration_secs: 100.0,
            has_audio: false,
        });
        for i in 1..=5 {
            a.handle_seek_request(i as f64 * 10.0);
            assert_eq!(a.current_seek_epoch(), i, "after seek #{i}");
            assert_eq!(a.latch.epoch, i, "latch epoch must follow current_seek_epoch");
        }
    }

    /// stale BufferReady (epoch < current_seek_epoch) は捨てられて遷移しない。
    /// `audio_rendered_after_seek_resets_guard` の対になるテスト (= rendered ではなく
    /// 初期化用の BufferReady 経路をカバー)。
    #[test]
    fn stale_buffer_ready_dropped_after_seek() {
        let mut a = fresh_actor();
        a.has_audio = true;
        a.begin_loading();
        a.handle_decoder_event(DecoderEvent::InfoReceived {
            epoch: 0,
            duration_secs: 30.0,
            has_audio: true,
        });
        // 新世代に
        a.handle_seek_request(15.0);
        assert_eq!(a.current_seek_epoch(), 1);

        // 古い epoch=0 の BufferReady → 捨てられる
        a.handle_audio_event(AudioEvent::BufferReady {
            epoch: 0,
            pts: 0.05,
            wall_now: Instant::now(),
        });
        assert!(
            !a.latch.buffer_ready,
            "stale BufferReady must not satisfy latch"
        );
        assert_eq!(a.state, EngineState::Seeking { target_secs: 15.0 });
    }

    /// stale EofReached も捨てられる (= 新世代 seek 中に旧世代 EOF が来てもループや
    /// 状態遷移を誤起動しない)。
    #[test]
    fn stale_eof_dropped_after_seek() {
        let mut a = fresh_actor();
        a.begin_loading();
        a.handle_decoder_event(DecoderEvent::InfoReceived {
            epoch: 0,
            duration_secs: 30.0,
            has_audio: false,
        });
        a.handle_decoder_event(DecoderEvent::FirstFrameReady { epoch: 0, pts: 0.0 });
        assert_eq!(a.state, EngineState::Playing);

        // 新 seek → epoch=1
        a.handle_seek_request(20.0);

        // 古い epoch=0 の EofReached → 無視
        a.handle_decoder_event(DecoderEvent::EofReached {
            epoch: 0,
            duration_secs: 30.0,
        });
        assert_eq!(
            a.state,
            EngineState::Seeking { target_secs: 20.0 },
            "stale EOF must not transition out of Seeking"
        );
    }

    /// loop replay: EOF + loop_enabled=true で seek(0) → SeekCompleted →
    /// FirstFrameReady で Playing に到達する end-to-end フロー。
    /// 旧 mod.rs のループ replay 経路 (apply_command(Play) → handle_seek_request)
    /// は二重 epoch++ で SeekCompleted が drop され Playing に行けなかった。
    #[test]
    fn loop_replay_reaches_playing_after_eof() {
        let mut a = fresh_actor_with_opts(OpenOptions {
            loop_enabled: true,
            ..Default::default()
        });
        a.begin_loading();
        a.handle_decoder_event(DecoderEvent::InfoReceived {
            epoch: 0,
            duration_secs: 30.0,
            has_audio: false,
        });
        a.handle_decoder_event(DecoderEvent::FirstFrameReady { epoch: 0, pts: 0.0 });

        // EOF 到達 → loop_enabled なので handle_seek_request(0.0) が内部発火
        a.handle_decoder_event(DecoderEvent::EofReached {
            epoch: 0,
            duration_secs: 30.0,
        });
        assert_eq!(a.state, EngineState::Seeking { target_secs: 0.0 });
        assert_eq!(a.current_seek_epoch(), 1);

        // decoder からの SeekCompleted{epoch=1} → Buffering
        a.handle_decoder_event(DecoderEvent::SeekCompleted {
            epoch: 1,
            actual_pts: 0.0,
        });
        assert_eq!(a.state, EngineState::Buffering);

        // FirstFrameReady{epoch=1} → Playing
        a.handle_decoder_event(DecoderEvent::FirstFrameReady { epoch: 1, pts: 0.0 });
        assert_eq!(a.state, EngineState::Playing);
    }

    /// EOF からの seek が target=0 ではなく user 指定 target を尊重することを保証。
    /// 旧 mod.rs::seek が Eof 状態で `apply_command(Play) → handle_seek_request(target)`
    /// 順だと、handle_play が internal で handle_seek_request(0.0) を呼んで Seeking{0.0}
    /// に遷移、続く明示 handle_seek_request(target) で再 Seeking{target} に上書き、
    /// epoch は 2 進むという二重ステップ問題があった。
    /// 正しい順 (handle_seek_request(target) → apply_command(Play)) で 1 ステップで
    /// 終わることを保証する。
    #[test]
    fn seek_to_target_during_eof_uses_target_not_zero() {
        let mut a = fresh_actor();
        a.begin_loading();
        a.handle_decoder_event(DecoderEvent::InfoReceived {
            epoch: 0,
            duration_secs: 30.0,
            has_audio: false,
        });
        a.handle_decoder_event(DecoderEvent::FirstFrameReady { epoch: 0, pts: 0.0 });
        a.handle_decoder_event(DecoderEvent::EofReached {
            epoch: 0,
            duration_secs: 30.0,
        });
        assert_eq!(a.state, EngineState::Eof);

        // 正しい順: handle_seek_request(15.0) → apply_command(Play)
        a.handle_seek_request(15.0);
        assert_eq!(a.current_seek_epoch(), 1);
        assert_eq!(a.state, EngineState::Seeking { target_secs: 15.0 });
        a.apply_command(TransportCommand::Play);
        // handle_play が Seeking arm を通る → epoch++ なし、state そのまま
        assert_eq!(a.current_seek_epoch(), 1, "no double-increment");
        assert_eq!(
            a.state,
            EngineState::Seeking { target_secs: 15.0 },
            "target must be 15.0, not 0.0"
        );
        assert!(a.opts.autoplay, "Play forces autoplay=true");
    }

    /// seek 後の SeekCompleted で actual_pts が target と微妙に異なっても
    /// (= keyframe スナップで実 pts が target ± preroll) Buffering に正しく遷移し
    /// 後続の FirstFrameReady で Playing に行ける。
    #[test]
    fn seek_completed_with_actual_pts_offset_still_reaches_playing() {
        let mut a = fresh_actor();
        a.begin_loading();
        a.handle_decoder_event(DecoderEvent::InfoReceived {
            epoch: 0,
            duration_secs: 60.0,
            has_audio: false,
        });
        a.handle_decoder_event(DecoderEvent::FirstFrameReady { epoch: 0, pts: 0.0 });

        // 30.0 を target に seek、actual は 28.5 (= keyframe が target より前)
        a.handle_seek_request(30.0);
        a.handle_decoder_event(DecoderEvent::SeekCompleted {
            epoch: 1,
            actual_pts: 28.5,
        });
        assert_eq!(a.state, EngineState::Buffering);
        // anchor は actual_pts (28.5) で frozen
        assert!(
            (a.clock().anchor().pts_secs - 28.5).abs() < 1e-9,
            "anchor must be set to actual_pts (preroll-aware)"
        );

        a.handle_decoder_event(DecoderEvent::FirstFrameReady {
            epoch: 1,
            pts: 28.5,
        });
        assert_eq!(a.state, EngineState::Playing);
    }
}
