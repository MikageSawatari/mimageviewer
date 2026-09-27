//! Shared display text for local audio track selectors and the future Remote UI.

use super::audio_track_selection::{AudioTrackSelectionDisplayState, AudioTrackSelectionSnapshot};
use super::decoder::{AudioTrackInfo, VideoInfo};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AudioTrackRow {
    pub label: String,
    pub stream_index: usize,
    pub ordinal: usize,
    pub is_current: bool,
    pub state: AudioTrackSelectionDisplayState,
}

pub fn language_label(code: &str) -> &str {
    match code {
        "jpn" | "ja" => "日本語",
        "eng" | "en" => "英語",
        "fra" | "fre" | "fr" => "フランス語",
        "deu" | "ger" | "de" => "ドイツ語",
        "spa" | "es" => "スペイン語",
        "kor" | "ko" => "韓国語",
        "zho" | "chi" | "zh" => "中国語",
        _ => code,
    }
}

pub fn audio_track_label(
    track: &AudioTrackInfo,
    default_stream_index: Option<usize>,
    state: AudioTrackSelectionDisplayState,
) -> String {
    let mut details = Vec::new();
    if let Some(language) = track.language.as_deref().filter(|s| !s.is_empty()) {
        details.push(language_label(language).to_owned());
    }
    if let Some(title) = track.title.as_deref().filter(|s| !s.is_empty()) {
        details.push(title.to_owned());
    }
    let mut label = format!("{}:", track.ordinal);
    if !details.is_empty() {
        label.push(' ');
        label.push_str(&details.join(" "));
    }
    let mut technical = Vec::new();
    if !track.codec.is_empty() {
        technical.push(track.codec.clone());
    }
    if let Some(channels) = track.channels {
        technical.push(format!("{channels}ch"));
    }
    if !technical.is_empty() {
        label.push_str(" — ");
        label.push_str(&technical.join(" "));
    }
    if default_stream_index == Some(track.stream_index) {
        label.push_str(" (既定)");
    }
    match state {
        AudioTrackSelectionDisplayState::Applied => {}
        AudioTrackSelectionDisplayState::Switching => label.push_str(" (切り替え中)"),
        AudioTrackSelectionDisplayState::Deferred => label.push_str(" (次の再生位置で切り替え)"),
        AudioTrackSelectionDisplayState::Failed(_) => label.push_str(" (切り替えできません)"),
    }
    label
}

pub fn audio_track_rows(
    info: &VideoInfo,
    selection: Option<AudioTrackSelectionSnapshot>,
    deferred: bool,
) -> Vec<AudioTrackRow> {
    // A player without an active audio lane has no selectable or applied track (§7.4).
    let Some(selection) = selection else {
        return Vec::new();
    };
    let applied = selection.applied.stream_index;
    let desired = selection.desired.stream_index;
    let derived = selection.display_state(deferred);
    info.audio_tracks
        .iter()
        .map(|track| {
            let state = if desired == track.stream_index {
                derived
            } else {
                AudioTrackSelectionDisplayState::Applied
            };
            AudioTrackRow {
                label: audio_track_label(track, info.default_audio_stream_index, state),
                stream_index: track.stream_index,
                ordinal: track.ordinal,
                is_current: applied == track.stream_index,
                state,
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn labels_include_only_known_fields_and_derived_state() {
        let track = AudioTrackInfo {
            stream_index: 4,
            ordinal: 2,
            language: Some("jpn".into()),
            title: Some("主音声".into()),
            codec: "aac".into(),
            channels: Some(2),
            sample_rate: None,
            disposition_default: false,
        };
        assert_eq!(
            audio_track_label(&track, Some(4), AudioTrackSelectionDisplayState::Applied),
            "2: 日本語 主音声 — aac 2ch (既定)"
        );
        assert!(
            audio_track_label(&track, None, AudioTrackSelectionDisplayState::Switching)
                .ends_with("(切り替え中)")
        );
        assert!(
            audio_track_label(&track, None, AudioTrackSelectionDisplayState::Deferred)
                .ends_with("(次の再生位置で切り替え)")
        );
        assert!(
            audio_track_label(
                &track,
                None,
                AudioTrackSelectionDisplayState::Failed(
                    super::super::audio_track_selection::AudioTrackSwitchFailureReason::SetupFailed
                )
            )
            .ends_with("(切り替えできません)")
        );
        let missing = AudioTrackInfo {
            language: Some("por".into()),
            title: None,
            codec: String::new(),
            channels: None,
            ..track
        };
        assert_eq!(
            audio_track_label(&missing, None, AudioTrackSelectionDisplayState::Applied),
            "2: por"
        );
    }
}
