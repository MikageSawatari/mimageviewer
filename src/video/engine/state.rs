//! `EngineState` — 動画再生エンジンの state machine 状態。
//!
//! 設計ドキュメント [docs/video-engine-redesign.md] の「2. EngineState」節を実装。
//!
//! ## 不変条件
//! - `Loading` / `Buffering` / `Seeking` / `Paused` / `Eof` のとき MasterClock は
//!   必ず `Frozen` source。**時間が暴走することがない**。
//! - `Buffering` から `Playing` への遷移トリガは `ReadinessRequirements` が要求する
//!   output readiness の latch (= `ReadinessLatch`)。Video 表示は従来どおり映像 + 音声、
//!   Music 表示は音声があれば audio buffer を要求する。両 readiness イベントは
//!   seek_epoch スコープで管理する。
//! - `Paused` / `Eof` 中は decoder thread が park している (= state を見て
//!   condvar 待ちに入る)。
//!
//! Phase 1b ではここで型と遷移ヘルパだけを定義する。実際の遷移は Phase 1c の
//! `EngineActor` が担当する。

use music_core::MediaVisualMode;
use std::time::Instant;

/// state machine の各状態。
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum EngineState {
    /// VideoEngine 構築直後。decoder thread 未起動。
    Idle,
    /// open 中: decoder spawn 済み、info_rx 待ち。
    Loading,
    /// info 受領後 / seek 完了後の preroll 待ち。Clock は Frozen で時間進行なし。
    /// `ReadinessRequirements` で選んだ readiness latch が揃うと `Playing`
    /// (or `Paused` if !autoplay) に遷移。
    ///
    /// 設計 doc では `Buffering { resume_target: Option<f64> }` と書かれているが、
    /// `resume_target` は EngineActor の context (= seek 履歴) でカバーでき、
    /// state 自身は payload を持たない方が `==` 比較・log 表記がシンプル。
    Buffering,
    /// 通常再生中。Clock は Audio または Wall で進行。
    Playing,
    /// 一時停止中。Clock は Frozen at last_pts。
    Paused,
    /// シーク中: decoder に seek 命令を出して `SeekCompleted` を待つ。
    /// Clock は Frozen at target。pre-seek frames は drain される。
    Seeking { target_secs: f64 },
    /// EOF 到達 (`loop_enabled=false` で停止状態)。Clock は Frozen at duration。
    Eof,
}

impl EngineState {
    /// この state で MasterClock は Frozen であるべきか? (= 時間進行を止めるか)
    pub fn requires_frozen_clock(&self) -> bool {
        matches!(
            self,
            EngineState::Idle
                | EngineState::Loading
                | EngineState::Buffering
                | EngineState::Paused
                | EngineState::Seeking { .. }
                | EngineState::Eof
        )
    }

    /// この state で decoder thread は park すべきか? (= 動画 frame の処理を止める)
    pub fn parks_decoder(&self) -> bool {
        matches!(self, EngineState::Paused | EngineState::Eof)
    }

    /// この state で decoder pacing は通常モードか? (= PACE_LEAD/audio_buf escape を有効化)
    /// `false` の場合、decoder は preroll モード (pacing skip + 即送出) で動く。
    pub fn pacing_normal(&self) -> bool {
        matches!(self, EngineState::Playing)
    }

    /// debug/log 表示用の短い名前。
    pub fn name(&self) -> &'static str {
        match self {
            EngineState::Idle => "Idle",
            EngineState::Loading => "Loading",
            EngineState::Buffering => "Buffering",
            EngineState::Playing => "Playing",
            EngineState::Paused => "Paused",
            EngineState::Seeking { .. } => "Seeking",
            EngineState::Eof => "Eof",
        }
    }
}

/// engine 内 events に共通する seek_epoch tag。
/// stale 検出のため全 readiness/anchor events に必ず含める。
pub type SeekEpoch = u64;

/// `Buffering -> Playing` に必要な出力 readiness。
///
/// `MediaVisualMode` を要求へ正規化してから latch を評価することで、表示されない
/// hidden presenter の実装詳細を再生開始条件から切り離す。通常の動画表示では従来どおり
/// first frame を要求し、音声表示では audio-only ファイルと動画の音声モードを同じ
/// audio buffer 要件で扱う。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ReadinessRequirements {
    pub first_frame: bool,
    pub audio_buffer: bool,
}

impl ReadinessRequirements {
    pub fn for_media(has_audio: bool, has_timed_video: bool, visual_mode: MediaVisualMode) -> Self {
        Self {
            // Music 表示でも音声が無い動画なら、再生可能な出力である映像を待つ。
            first_frame: has_timed_video && (visual_mode == MediaVisualMode::Video || !has_audio),
            audio_buffer: has_audio,
        }
    }

    fn has_playable_output(self) -> bool {
        self.first_frame || self.audio_buffer
    }
}

/// EngineActor が受け取る decoder 由来の events。
#[derive(Debug, Clone)]
pub enum DecoderEvent {
    /// open path 完了 (`info_rx` 受信相当)。
    InfoReceived {
        epoch: SeekEpoch,
        duration_secs: f64,
        has_audio: bool,
        /// timed playable video stream を持つか (= 映像 decode thread が走り
        /// FirstFrameReady が届くか)。audio-only ファイル (映像トラック無し /
        /// 添付画像のみ) では false。`MediaVisualMode` と合わせて
        /// `ReadinessRequirements` を導出する metadata。既定 (未設定時) は true で
        /// 従来の動画経路と等価。
        has_video: bool,
    },
    /// seek 完了 (= avformat_seek_file 後の最初の post-seek decode 直前)。
    /// `actual_pts` は seek 後の最初の動画 PTS の見込み (decoder が確定した値)。
    SeekCompleted { epoch: SeekEpoch, actual_pts: f64 },
    /// post-seek (or open) の最初の動画 frame が decode/blit 完了し UI に届いた。
    FirstFrameReady { epoch: SeekEpoch, pts: f64 },
    /// decoder が file 末尾 (demux EOF) に到達。
    EofReached {
        epoch: SeekEpoch,
        duration_secs: f64,
    },
    /// open / decode が致命的に失敗した (panic 相当)。EngineActor は Idle に戻す。
    Failed { reason: String },
}

/// EngineActor が受け取る audio 由来の events。
#[derive(Debug, Clone)]
pub enum AudioEvent {
    /// 通常 callback ごとに送出される anchor 候補 (cpal callback 内で消費した
    /// 末尾 sample の PTS と、その瞬間の wall 時刻)。
    AudioRendered {
        epoch: SeekEpoch,
        pts: f64,
        wall_now: Instant,
    },
    /// audio buffer が READY_THRESHOLD に到達している間、level event として届く
    /// (Phase 8.K で edge → level 化)。Buffering→Playing 判定の latch を閉じる。
    BufferReady {
        epoch: SeekEpoch,
        pts: f64,
        wall_now: Instant,
    },
    /// audio buffer が AUDIO_SAFE_LO (= 250ms) を下回った (underrun 警告)。
    /// EngineActor が Playing → Buffering に戻すかどうか判断する材料。
    BufferStarved { epoch: SeekEpoch },
    /// audio 出力起動失敗 (cpal device open エラー等)。以降 wall master で進む。
    AudioInactive,
}

/// readiness latch (Buffering → Playing 遷移用)。
///
/// 全 events は `SeekEpoch` を含み、stale (= `epoch < current_seek_epoch`) は捨てる。
/// 1 論理 seek につき epoch (= 共有 `Arc<AtomicU64>`) を 1 回だけ bump する:
/// 外部経路は `AvClock::request_seek`、engine 内部経路は
/// `EngineActor::handle_seek_request` 経由で `av_clock.request_seek` をディスパッチする。
/// SeekCompleted / Buffering 入場では epoch を進めない (= 既に進めた値を使う)。
#[derive(Debug)]
pub struct ReadinessLatch {
    /// この latch が属する seek 世代。stale 検出に使う。
    pub epoch: SeekEpoch,
    /// `FirstFrameReady` を観測したか。
    pub first_frame: bool,
    /// FirstFrameReady の actual pts (= no-audio anchor source 用)。
    pub first_frame_pts: Option<f64>,
    /// `BufferReady` を観測したか。
    pub buffer_ready: bool,
    /// BufferReady event が返した最初の有効 audio anchor (pts, wall)。
    /// `Playing` 遷移時に `ClockAnchor::audio` を作るために保持する。
    pub audio_anchor: Option<(f64, Instant)>,
}

impl ReadinessLatch {
    /// 新世代の latch を作る (全 readiness false)。
    pub fn new(epoch: SeekEpoch) -> Self {
        Self {
            epoch,
            first_frame: false,
            first_frame_pts: None,
            buffer_ready: false,
            audio_anchor: None,
        }
    }

    /// `Buffering → Playing` の遷移条件を満たすか。
    ///
    /// - `requirements.first_frame = false` の場合は `FirstFrameReady` を待たない。
    /// - `requirements.audio_buffer = false` の場合は `BufferReady` を待たない。
    /// - 両要件が false のときは anchor source が無いので never ready
    ///   (= 再生可能ストリーム皆無。通常は decoder 側で弾くが防御的に扱う)。
    ///
    /// `is_ready=true` の必要十分条件として、anchor 構築に必要な `Option` (=
    /// first frame を要求するなら `first_frame_pts`、audio buffer を要求するなら
    /// `audio_anchor`) も同時に
    /// 存在することを保証する (= 呼び出し側の `expect` を不要にする)。
    pub fn is_ready(&self, requirements: ReadinessRequirements) -> bool {
        if !requirements.has_playable_output() {
            return false;
        }
        if requirements.first_frame && (!self.first_frame || self.first_frame_pts.is_none()) {
            return false;
        }
        if requirements.audio_buffer && (!self.buffer_ready || self.audio_anchor.is_none()) {
            return false;
        }
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn video_requirements(has_audio: bool, has_video: bool) -> ReadinessRequirements {
        ReadinessRequirements::for_media(has_audio, has_video, MediaVisualMode::Video)
    }

    #[test]
    fn frozen_clock_states_classified_correctly() {
        assert!(EngineState::Idle.requires_frozen_clock());
        assert!(EngineState::Loading.requires_frozen_clock());
        assert!(EngineState::Buffering.requires_frozen_clock());
        assert!(EngineState::Paused.requires_frozen_clock());
        assert!(EngineState::Seeking { target_secs: 10.0 }.requires_frozen_clock());
        assert!(EngineState::Eof.requires_frozen_clock());
        // Playing のみ Frozen ではない
        assert!(!EngineState::Playing.requires_frozen_clock());
    }

    #[test]
    fn parks_decoder_only_for_paused_and_eof() {
        assert!(!EngineState::Playing.parks_decoder());
        assert!(!EngineState::Buffering.parks_decoder());
        assert!(!EngineState::Seeking { target_secs: 5.0 }.parks_decoder());
        assert!(EngineState::Paused.parks_decoder());
        assert!(EngineState::Eof.parks_decoder());
    }

    #[test]
    fn pacing_normal_only_for_playing() {
        assert!(EngineState::Playing.pacing_normal());
        assert!(!EngineState::Buffering.pacing_normal());
        assert!(!EngineState::Seeking { target_secs: 0.0 }.pacing_normal());
        assert!(!EngineState::Loading.pacing_normal());
        assert!(!EngineState::Paused.pacing_normal());
        assert!(!EngineState::Eof.pacing_normal());
    }

    #[test]
    fn latch_not_ready_when_no_events() {
        let l = ReadinessLatch::new(0);
        assert!(!l.is_ready(video_requirements(true, true)));
        assert!(!l.is_ready(video_requirements(false, true)));
    }

    #[test]
    fn latch_video_only_ready_after_first_frame() {
        let mut l = ReadinessLatch::new(1);
        l.first_frame = true;
        l.first_frame_pts = Some(0.0);
        assert!(
            l.is_ready(video_requirements(false, true)),
            "video-only: first_frame alone should suffice"
        );
        // has_audio=true ではまだ不足
        assert!(
            !l.is_ready(video_requirements(true, true)),
            "with audio: buffer_ready also required"
        );
    }

    #[test]
    fn latch_audio_path_requires_both() {
        let mut l = ReadinessLatch::new(2);
        l.buffer_ready = true;
        l.audio_anchor = Some((10.0, Instant::now()));
        assert!(
            !l.is_ready(video_requirements(true, true)),
            "buffer_ready alone: not ready (need first_frame)"
        );
        l.first_frame = true;
        l.first_frame_pts = Some(10.0);
        assert!(
            l.is_ready(video_requirements(true, true)),
            "first_frame + buffer_ready: ready"
        );
    }

    #[test]
    fn latch_new_carries_epoch() {
        let l = ReadinessLatch::new(42);
        assert_eq!(l.epoch, 42);
    }

    #[test]
    fn parks_decoder_table_complete() {
        // 全 variant で false / true が design table と一致すること
        let cases: &[(EngineState, bool)] = &[
            (EngineState::Idle, false),
            (EngineState::Loading, false),
            (EngineState::Buffering, false),
            (EngineState::Playing, false),
            (EngineState::Seeking { target_secs: 0.0 }, false),
            (EngineState::Paused, true),
            (EngineState::Eof, true),
        ];
        for (state, expected) in cases {
            assert_eq!(
                state.parks_decoder(),
                *expected,
                "parks_decoder mismatch for {}",
                state.name()
            );
        }
    }

    #[test]
    fn pacing_normal_table_complete() {
        let cases: &[(EngineState, bool)] = &[
            (EngineState::Idle, false),
            (EngineState::Loading, false),
            (EngineState::Buffering, false),
            (EngineState::Playing, true),
            (EngineState::Paused, false),
            (EngineState::Seeking { target_secs: 0.0 }, false),
            (EngineState::Eof, false),
        ];
        for (state, expected) in cases {
            assert_eq!(
                state.pacing_normal(),
                *expected,
                "pacing_normal mismatch for {}",
                state.name()
            );
        }
    }

    #[test]
    fn latch_first_frame_then_buffer_ready_order() {
        let mut l = ReadinessLatch::new(3);
        l.first_frame = true;
        l.first_frame_pts = Some(0.0);
        assert!(
            !l.is_ready(video_requirements(true, true)),
            "first_frame alone w/ audio: not ready"
        );
        l.buffer_ready = true;
        l.audio_anchor = Some((0.05, Instant::now()));
        assert!(
            l.is_ready(video_requirements(true, true)),
            "first_frame then buffer_ready: ready"
        );
    }

    #[test]
    fn latch_buffer_ready_then_first_frame_order() {
        let mut l = ReadinessLatch::new(4);
        l.buffer_ready = true;
        l.audio_anchor = Some((0.05, Instant::now()));
        assert!(
            !l.is_ready(video_requirements(true, true)),
            "buffer_ready alone: not ready"
        );
        l.first_frame = true;
        l.first_frame_pts = Some(0.0);
        assert!(
            l.is_ready(video_requirements(true, true)),
            "buffer_ready then first_frame: ready"
        );
    }

    #[test]
    fn latch_audio_disabled_midflight_satisfies_readiness() {
        // has_audio=true で開始したが、AudioInactive で audio が無くなった場合、
        // EngineActor は has_audio=false で is_ready を再評価することで latch 完成。
        let mut l = ReadinessLatch::new(5);
        l.first_frame = true;
        l.first_frame_pts = Some(0.0);
        assert!(
            !l.is_ready(video_requirements(true, true)),
            "has_audio=true, no buffer_ready: not ready"
        );
        assert!(
            l.is_ready(video_requirements(false, true)),
            "has_audio=false (audio inactive): ready"
        );
    }

    #[test]
    fn latch_audio_only_ready_on_buffer_ready_without_first_frame() {
        // audio-only ファイル (has_video=false): 映像 thread が無く FirstFrameReady が
        // 来ないので、first_frame を待たず BufferReady + anchor だけで ready になる。
        let mut l = ReadinessLatch::new(6);
        assert!(
            !l.is_ready(video_requirements(true, false)),
            "audio-only: buffer_ready 未達なら not ready"
        );
        l.buffer_ready = true;
        l.audio_anchor = Some((3.0, Instant::now()));
        assert!(
            l.is_ready(video_requirements(true, false)),
            "audio-only: first_frame 無しでも buffer_ready + anchor で ready"
        );
        // first_frame が来ないことは audio-only では ready を妨げない
        assert!(!l.first_frame, "audio-only は first_frame 前提にしない");
    }

    #[test]
    fn audio_file_and_video_audio_mode_share_audio_only_requirements() {
        let audio_file = ReadinessRequirements::for_media(true, false, MediaVisualMode::Music);
        let video_audio_mode = ReadinessRequirements::for_media(true, true, MediaVisualMode::Music);

        assert_eq!(audio_file, video_audio_mode);
        assert_eq!(
            audio_file,
            ReadinessRequirements {
                first_frame: false,
                audio_buffer: true,
            }
        );
    }

    #[test]
    fn video_mode_keeps_first_frame_requirement() {
        assert_eq!(
            ReadinessRequirements::for_media(true, true, MediaVisualMode::Video),
            ReadinessRequirements {
                first_frame: true,
                audio_buffer: true,
            }
        );
    }

    #[test]
    fn latch_never_ready_without_any_stream() {
        // has_video=false かつ has_audio=false は anchor source が無いので never ready
        // (= 再生可能ストリーム皆無。防御的挙動)。
        let mut l = ReadinessLatch::new(7);
        assert!(
            !l.is_ready(video_requirements(false, false)),
            "no video/no audio: not ready"
        );
        l.first_frame = true;
        l.first_frame_pts = Some(0.0);
        l.buffer_ready = true;
        l.audio_anchor = Some((0.0, Instant::now()));
        assert!(
            !l.is_ready(video_requirements(false, false)),
            "no video/no audio: latch 全埋めでも not ready"
        );
    }

    #[test]
    fn state_name_distinct_for_each_variant() {
        let names = [
            EngineState::Idle.name(),
            EngineState::Loading.name(),
            EngineState::Buffering.name(),
            EngineState::Playing.name(),
            EngineState::Paused.name(),
            EngineState::Seeking { target_secs: 1.0 }.name(),
            EngineState::Eof.name(),
        ];
        // 全名前がユニークであることを確認
        let mut sorted = names.to_vec();
        sorted.sort();
        sorted.dedup();
        assert_eq!(sorted.len(), names.len(), "state names must be distinct");
    }
}
