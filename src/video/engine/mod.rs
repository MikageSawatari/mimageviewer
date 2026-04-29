//! 動画再生エンジンの新世代 (リデザイン v6)。
//!
//! 設計ドキュメント: [docs/video-engine-redesign.md](../../../docs/video-engine-redesign.md)
//!
//! Phase 1a: `MasterClock` + `ClockAnchor` を導入 (既存の `AvClock` 未利用、compile-only)。
//! Phase 1b: `EngineState` + `ReadinessLatch` + event 型を追加 (= state machine の骨格)。
//! Phase 1c: `EngineActor` skeleton を追加予定。
//! Phase 2 以降で AvClock を facade 化し、最終的に削除する。

pub mod actor;
pub mod audio_bookkeeping;
pub mod clock;
pub mod state;

/// decoder thread と audio thread が EngineActor に流す統合イベント。
/// VideoPlayer::tick で channel から drain して `EngineActor::handle_*` に dispatch する。
#[derive(Debug, Clone)]
pub enum EngineEvent {
    Decoder(state::DecoderEvent),
    Audio(state::AudioEvent),
}

impl From<state::DecoderEvent> for EngineEvent {
    fn from(ev: state::DecoderEvent) -> Self {
        EngineEvent::Decoder(ev)
    }
}

impl From<state::AudioEvent> for EngineEvent {
    fn from(ev: state::AudioEvent) -> Self {
        EngineEvent::Audio(ev)
    }
}
