#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct PlaybackSnapshot {
    pub position_secs: f64,
    pub duration_secs: f64,
    pub playing: bool,
    pub effect_chain_active: bool,
    pub effect_latency_samples: u32,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum PlaybackIntent {
    Play,
    Pause,
    Toggle,
    SeekSeconds(f64),
}
