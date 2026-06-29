use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum MediaVisualMode {
    Video,
    Music,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum MusicModeSource {
    AudioFile,
    VideoAudioOnly,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct MusicBookmark {
    pub id: u64,
    pub position_secs: f64,
    pub title: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct MusicTimelineLayout {
    pub row_secs: f64,
    pub beats_per_bar: u8,
    pub bars_per_visual_line_hint: u8,
}

impl Default for MusicTimelineLayout {
    fn default() -> Self {
        Self {
            row_secs: 60.0,
            beats_per_bar: 4,
            bars_per_visual_line_hint: 8,
        }
    }
}
