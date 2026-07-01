//! Audio effect-chain boundary.
//!
//! Production integration should adapt the existing video `DspBridge` to this
//! trait from an audio pump thread. VST3 IPC must not run on the cpal real-time
//! callback thread; the lab can use `NoopEffectChain` while the data flow is
//! being shaped.

#[derive(Debug)]
pub struct AudioProcessBlock<'a> {
    pub samples_interleaved: &'a mut [f32],
    pub channels: u16,
    pub sample_rate: u32,
    pub stream_position_secs: f64,
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct EffectChainStats {
    pub active: bool,
    pub latency_samples: u32,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum EffectError {
    TemporarilyUnavailable,
    Failed(String),
}

impl std::fmt::Display for EffectError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::TemporarilyUnavailable => write!(f, "effect chain temporarily unavailable"),
            Self::Failed(message) => write!(f, "{message}"),
        }
    }
}

impl std::error::Error for EffectError {}

pub trait EffectChain: Send {
    fn process_block(
        &mut self,
        block: AudioProcessBlock<'_>,
    ) -> Result<EffectChainStats, EffectError>;
}

#[derive(Default)]
pub struct NoopEffectChain;

impl EffectChain for NoopEffectChain {
    fn process_block(
        &mut self,
        _block: AudioProcessBlock<'_>,
    ) -> Result<EffectChainStats, EffectError> {
        Ok(EffectChainStats::default())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn noop_chain_reports_inactive() {
        let mut chain = NoopEffectChain;
        let mut samples = [0.0_f32; 8];
        let stats = chain
            .process_block(AudioProcessBlock {
                samples_interleaved: &mut samples,
                channels: 2,
                sample_rate: 48_000,
                stream_position_secs: 0.0,
            })
            .unwrap();

        assert_eq!(stats, EffectChainStats::default());
    }
}
