//! 動画再生エンジンの新世代 (リデザイン v6)。
//!
//! 設計ドキュメント: [docs/video-engine-redesign.md](../../../docs/video-engine-redesign.md)
//!
//! Phase 1a: `MasterClock` + `ClockAnchor` を導入 (既存の `AvClock` 未利用、compile-only)。
//! Phase 1b: `EngineState` + `ReadinessLatch` + event 型を追加 (= state machine の骨格)。
//! Phase 1c: `EngineActor` skeleton を追加。
//! Phase 2: AvClock を facade 化 (内部実装を engine::clock + audio_bookkeeping に分離)。
//! Phase 3: state machine を runtime に配線、decoder/audio events を engine が処理。
//! Phase 4: AvClock を薄い facade として固定 (= 当初計画の完全撤去から軌道修正)。
//! 旧 AvClock の互換 callsite が `decoder.rs` / `audio.rs` / `VideoPlayer` に残るが、
//! source of truth は `EngineActor`。

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
