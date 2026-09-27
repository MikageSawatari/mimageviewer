//! Per-player audio track selection. UI owns desired; demux owns applied and failures.

use std::sync::Mutex;

pub(crate) fn audio_track_available_at(
    track: &crate::video::decoder::AudioTrackInfo,
    position: f64,
) -> bool {
    position.is_finite()
        && position >= 0.0
        && track.start_secs.is_none_or(|start| position >= start)
        && track
            .end_secs
            .is_none_or(|end| position + crate::video::audio::AUDIO_TRACK_READY_MARGIN_SECS <= end)
}

/// File-scoped choice. Unlike `AudioTrackChoice`, it has no request generation.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct SavedAudioTrackChoice {
    pub stream_index: usize,
    pub codec: String,
    pub language: Option<String>,
    pub channels: Option<u32>,
    pub title: Option<String>,
}

impl From<&crate::video::decoder::AudioTrackInfo> for SavedAudioTrackChoice {
    fn from(track: &crate::video::decoder::AudioTrackInfo) -> Self {
        Self {
            stream_index: track.stream_index,
            codec: track.codec.clone(),
            language: track.language.clone(),
            channels: track.channels,
            title: track.title.clone(),
        }
    }
}

pub(crate) fn resolve_initial_audio_track(
    tracks: &[crate::video::decoder::AudioTrackInfo],
    default: Option<usize>,
    saved: Option<&SavedAudioTrackChoice>,
) -> Option<usize> {
    saved
        .and_then(|saved| {
            tracks.iter().find(|track| {
                track.stream_index == saved.stream_index
                    && track.codec == saved.codec
                    && saved
                        .language
                        .as_ref()
                        .is_none_or(|value| track.language.as_ref() == Some(value))
                    && saved
                        .channels
                        .is_none_or(|value| track.channels == Some(value))
                    && saved
                        .title
                        .as_ref()
                        .is_none_or(|value| track.title.as_ref() == Some(value))
            })
        })
        .map(|track| track.stream_index)
        .or(default)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AudioTrackChoice {
    pub generation: u64,
    pub stream_index: usize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AudioTrackSwitchFailureReason {
    SetupFailed,
    SeekFailed,
    WorkerGone,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AudioTrackSwitchFailure {
    pub choice: AudioTrackChoice,
    pub reason: AudioTrackSwitchFailureReason,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AudioTrackOpenNotice {
    SavedTrackUnavailable,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AudioTrackSelectionSnapshot {
    pub desired: AudioTrackChoice,
    pub applied: AudioTrackChoice,
    pub last_failure: Option<AudioTrackSwitchFailure>,
    pub deferred_gen: Option<u64>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AudioTrackSelectionDisplayState {
    Applied,
    Deferred,
    Switching,
    Failed(AudioTrackSwitchFailureReason),
}

impl AudioTrackSelectionSnapshot {
    pub fn display_state(self, engine_eof: bool) -> AudioTrackSelectionDisplayState {
        if self.desired.generation == self.applied.generation {
            AudioTrackSelectionDisplayState::Applied
        } else if let Some(failure) = self
            .last_failure
            .filter(|failure| failure.choice.generation == self.desired.generation)
        {
            AudioTrackSelectionDisplayState::Failed(failure.reason)
        } else if engine_eof || self.deferred_gen == Some(self.desired.generation) {
            AudioTrackSelectionDisplayState::Deferred
        } else {
            AudioTrackSelectionDisplayState::Switching
        }
    }

    pub(crate) fn switch_candidate(self) -> Option<AudioTrackChoice> {
        (self.desired.generation > self.applied.generation
            && self
                .last_failure
                .is_none_or(|failure| failure.choice.generation != self.desired.generation))
        .then_some(self.desired)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AudioTrackSelectOutcome {
    Rejected,
    Unchanged,
    Deferred,
    Requested,
}

/// Extensible result envelope; S3 adds the selected track's Norm resolution.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AudioTrackSelectResult {
    pub outcome: AudioTrackSelectOutcome,
    pub normalize_unresolved: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum AudioTrackRequestOutcome {
    Rejected,
    Unchanged,
    Accepted,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum AudioLaneState {
    Active,
    Lost,
}

struct AudioTrackSelectionState {
    snapshot: AudioTrackSelectionSnapshot,
    lane: AudioLaneState,
    notified_failure_generation: u64,
}

pub(crate) struct AudioTrackSelection {
    state: Mutex<AudioTrackSelectionState>,
}

impl AudioTrackSelection {
    pub(crate) fn new(stream_index: usize) -> Self {
        let choice = AudioTrackChoice {
            generation: 0,
            stream_index,
        };
        Self {
            state: Mutex::new(AudioTrackSelectionState {
                snapshot: AudioTrackSelectionSnapshot {
                    desired: choice,
                    applied: choice,
                    last_failure: None,
                    deferred_gen: None,
                },
                lane: AudioLaneState::Active,
                notified_failure_generation: 0,
            }),
        }
    }

    pub(crate) fn snapshot(&self) -> AudioTrackSelectionSnapshot {
        self.state.lock().unwrap().snapshot
    }

    pub(crate) fn take_failure_notification(&self, deferred: bool) -> bool {
        let mut state = self.state.lock().unwrap();
        if !matches!(
            state.snapshot.display_state(deferred),
            AudioTrackSelectionDisplayState::Failed(_)
        ) {
            return false;
        }
        let generation = state.snapshot.desired.generation;
        if state.notified_failure_generation == generation {
            return false;
        }
        state.notified_failure_generation = generation;
        true
    }

    /// UI thread only. Admission and demux lane closure share one lock. The
    /// caller publishes the seek after an accepted request returns.
    pub(crate) fn request(&self, stream_index: usize) -> AudioTrackRequestOutcome {
        self.request_with_choice(stream_index).0
    }

    pub(crate) fn request_with_choice(
        &self,
        stream_index: usize,
    ) -> (AudioTrackRequestOutcome, Option<AudioTrackChoice>) {
        let mut state = self.state.lock().unwrap();
        if state.lane == AudioLaneState::Lost {
            return (AudioTrackRequestOutcome::Rejected, None);
        }
        let snapshot = &mut state.snapshot;
        if snapshot.desired.stream_index == stream_index
            && snapshot
                .last_failure
                .is_none_or(|failure| failure.choice.generation != snapshot.desired.generation)
        {
            return (AudioTrackRequestOutcome::Unchanged, None);
        }
        snapshot.desired.generation += 1;
        snapshot.desired.stream_index = stream_index;
        (AudioTrackRequestOutcome::Accepted, Some(snapshot.desired))
    }

    /// UI and demux can record a deferral only while this exact choice is current.
    pub(crate) fn defer_if_current(&self, choice: AudioTrackChoice) {
        let mut state = self.state.lock().unwrap();
        if state.lane == AudioLaneState::Active && state.snapshot.desired == choice {
            state.snapshot.deferred_gen = Some(choice.generation);
        }
    }

    /// A stale demux attempt cannot clear a newer choice's deferral.
    pub(crate) fn begin_attempt(&self, choice: AudioTrackChoice) {
        let mut state = self.state.lock().unwrap();
        if state.snapshot.deferred_gen == Some(choice.generation) {
            state.snapshot.deferred_gen = None;
        }
    }

    /// Demux thread only. A request admitted before closure gets WorkerGone;
    /// a request after closure is rejected by the same synchronized owner.
    pub(crate) fn close_lane(&self) {
        let mut state = self.state.lock().unwrap();
        if state.lane == AudioLaneState::Lost {
            return;
        }
        state.lane = AudioLaneState::Lost;
        if let Some(choice) = state.snapshot.switch_candidate() {
            state.snapshot.last_failure = Some(AudioTrackSwitchFailure {
                choice,
                reason: AudioTrackSwitchFailureReason::WorkerGone,
            });
        }
    }

    /// Demux thread only, after seek and every existing Flush were accepted.
    pub(crate) fn apply(&self, choice: AudioTrackChoice) {
        let mut state = self.state.lock().unwrap();
        if state.lane == AudioLaneState::Active {
            state.snapshot.applied = choice;
        }
    }

    /// Demux thread only. Failure of an older generation cannot change the
    /// display state of a newer desired generation.
    pub(crate) fn fail(&self, choice: AudioTrackChoice, reason: AudioTrackSwitchFailureReason) {
        let mut state = self.state.lock().unwrap();
        if state.lane == AudioLaneState::Active {
            state.snapshot.last_failure = Some(AudioTrackSwitchFailure { choice, reason });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn availability_uses_stream_bounds_and_a_margin_above_readiness() {
        assert!(
            crate::video::audio::AUDIO_TRACK_READY_MARGIN_SECS
                >= crate::video::audio::READY_THRESHOLD_SECS
        );
        let mut track = crate::video::decoder::AudioTrackInfo {
            stream_index: 2,
            ordinal: 2,
            language: None,
            title: None,
            codec: "aac".into(),
            channels: Some(1),
            sample_rate: Some(44_100),
            disposition_default: false,
            start_secs: Some(4.0),
            end_secs: Some(10.0),
        };
        assert!(!audio_track_available_at(&track, 3.99));
        assert!(audio_track_available_at(&track, 4.0));
        assert!(audio_track_available_at(&track, 9.5));
        assert!(!audio_track_available_at(&track, 9.501));
        assert!(!audio_track_available_at(&track, 10.0));
        track.end_secs = None;
        assert!(audio_track_available_at(&track, 20.0));
        track.start_secs = None;
        assert!(audio_track_available_at(&track, 0.0));
    }

    #[test]
    fn stale_availability_result_cannot_defer_new_choice() {
        let selection = AudioTrackSelection::new(1);
        assert_eq!(selection.request(2), AudioTrackRequestOutcome::Accepted);
        let old = selection.snapshot().desired;
        assert_eq!(selection.request(3), AudioTrackRequestOutcome::Accepted);
        selection.defer_if_current(old);
        let snapshot = selection.snapshot();
        assert_eq!(snapshot.deferred_gen, None);
        assert_eq!(
            snapshot.display_state(false),
            AudioTrackSelectionDisplayState::Switching
        );
    }

    #[test]
    fn stale_attempt_cannot_clear_new_choice_deferral() {
        let selection = AudioTrackSelection::new(1);
        assert_eq!(selection.request(2), AudioTrackRequestOutcome::Accepted);
        let old = selection.snapshot().desired;
        selection.defer_if_current(old);
        assert_eq!(selection.request(3), AudioTrackRequestOutcome::Accepted);
        let newest = selection.snapshot().desired;
        selection.defer_if_current(newest);
        selection.begin_attempt(old);
        assert_eq!(selection.snapshot().deferred_gen, Some(newest.generation));
        assert_eq!(
            selection.snapshot().display_state(false),
            AudioTrackSelectionDisplayState::Deferred
        );
        selection.begin_attempt(newest);
        assert_eq!(
            selection.snapshot().display_state(false),
            AudioTrackSelectionDisplayState::Switching
        );
        selection.fail(newest, AudioTrackSwitchFailureReason::SeekFailed);
        assert_eq!(
            selection.snapshot().display_state(true),
            AudioTrackSelectionDisplayState::Failed(AudioTrackSwitchFailureReason::SeekFailed)
        );
    }

    #[test]
    fn saved_track_identity_requires_present_metadata_and_title() {
        let track = crate::video::decoder::AudioTrackInfo {
            stream_index: 2,
            ordinal: 1,
            language: Some("jpn".into()),
            title: Some("Commentary".into()),
            codec: "aac".into(),
            channels: Some(2),
            sample_rate: Some(48_000),
            disposition_default: false,
            start_secs: None,
            end_secs: None,
        };
        let tracks = [track.clone()];
        let saved = SavedAudioTrackChoice::from(&track);
        assert_eq!(
            resolve_initial_audio_track(&tracks, Some(1), Some(&saved)),
            Some(2)
        );
        for changed in [
            SavedAudioTrackChoice {
                stream_index: 3,
                ..saved.clone()
            },
            SavedAudioTrackChoice {
                codec: "ac3".into(),
                ..saved.clone()
            },
            SavedAudioTrackChoice {
                language: Some("eng".into()),
                ..saved.clone()
            },
            SavedAudioTrackChoice {
                channels: Some(6),
                ..saved.clone()
            },
            SavedAudioTrackChoice {
                title: Some("New title".into()),
                ..saved.clone()
            },
        ] {
            assert_eq!(
                resolve_initial_audio_track(&tracks, Some(1), Some(&changed)),
                Some(1)
            );
        }
        let sparse = SavedAudioTrackChoice {
            language: None,
            channels: None,
            title: None,
            ..saved
        };
        assert_eq!(
            resolve_initial_audio_track(&tracks, Some(1), Some(&sparse)),
            Some(2)
        );
    }

    #[test]
    fn failure_notification_is_once_per_desired_generation() {
        let selection = AudioTrackSelection::new(1);
        assert!(!selection.take_failure_notification(false));
        assert_eq!(selection.request(2), AudioTrackRequestOutcome::Accepted);
        let first = selection.snapshot().desired;
        selection.fail(first, AudioTrackSwitchFailureReason::SetupFailed);
        assert!(selection.take_failure_notification(false));
        assert!(!selection.take_failure_notification(false));
        assert_eq!(selection.request(2), AudioTrackRequestOutcome::Accepted);
        let second = selection.snapshot().desired;
        selection.fail(second, AudioTrackSwitchFailureReason::SeekFailed);
        assert!(selection.take_failure_notification(false));
        assert!(!selection.take_failure_notification(false));
    }

    #[test]
    fn display_state_is_derived_from_generation_and_deferred_condition() {
        let selection = AudioTrackSelection::new(1);
        assert_eq!(
            selection.snapshot().display_state(false),
            AudioTrackSelectionDisplayState::Applied
        );
        assert_eq!(selection.request(2), AudioTrackRequestOutcome::Accepted);
        let first = selection.snapshot().desired;
        assert_eq!(
            selection.snapshot().display_state(true),
            AudioTrackSelectionDisplayState::Deferred
        );
        assert_eq!(
            selection.snapshot().display_state(false),
            AudioTrackSelectionDisplayState::Switching
        );
        selection.fail(first, AudioTrackSwitchFailureReason::SetupFailed);
        assert_eq!(
            selection.snapshot().display_state(false),
            AudioTrackSelectionDisplayState::Failed(AudioTrackSwitchFailureReason::SetupFailed)
        );
        assert_eq!(selection.snapshot().switch_candidate(), None);
        assert_eq!(selection.request(2), AudioTrackRequestOutcome::Accepted);
        assert_eq!(
            selection.snapshot().display_state(false),
            AudioTrackSelectionDisplayState::Switching
        );
        selection.apply(first);
        assert_eq!(
            selection.snapshot().display_state(false),
            AudioTrackSelectionDisplayState::Switching
        );
        selection.apply(selection.snapshot().desired);
        assert_eq!(
            selection.snapshot().display_state(false),
            AudioTrackSelectionDisplayState::Applied
        );
    }

    #[test]
    fn late_failure_cannot_overwrite_new_desired_generation() {
        let selection = AudioTrackSelection::new(2);
        assert_eq!(selection.request(1), AudioTrackRequestOutcome::Accepted);
        let old = selection.snapshot().desired;
        assert_eq!(selection.request(3), AudioTrackRequestOutcome::Accepted);
        selection.fail(old, AudioTrackSwitchFailureReason::SeekFailed);
        let snapshot = selection.snapshot();
        assert_eq!(snapshot.desired.stream_index, 3);
        assert_eq!(snapshot.switch_candidate(), Some(snapshot.desired));
        assert_eq!(
            snapshot.display_state(false),
            AudioTrackSelectionDisplayState::Switching
        );
    }

    #[test]
    fn request_before_lane_closure_records_worker_gone_for_that_generation() {
        let selection = std::sync::Arc::new(AudioTrackSelection::new(1));
        let worker_selection = selection.clone();
        let (admitted_tx, admitted_rx) = std::sync::mpsc::channel();
        let requester = std::thread::spawn(move || {
            admitted_tx.send(worker_selection.request(2)).unwrap();
        });
        assert_eq!(
            admitted_rx.recv().unwrap(),
            AudioTrackRequestOutcome::Accepted
        );
        requester.join().unwrap();
        let admitted = selection.snapshot().desired;
        selection.close_lane();
        let snapshot = selection.snapshot();
        assert_eq!(snapshot.desired, admitted);
        assert_eq!(
            snapshot.last_failure,
            Some(AudioTrackSwitchFailure {
                choice: admitted,
                reason: AudioTrackSwitchFailureReason::WorkerGone,
            })
        );
        assert_eq!(
            snapshot.display_state(false),
            AudioTrackSelectionDisplayState::Failed(AudioTrackSwitchFailureReason::WorkerGone)
        );
        assert_eq!(snapshot.switch_candidate(), None);
        assert_eq!(selection.request(3), AudioTrackRequestOutcome::Rejected);
    }

    #[test]
    fn lane_closure_before_request_rejects_without_advancing_generation() {
        let selection = std::sync::Arc::new(AudioTrackSelection::new(1));
        let worker_selection = selection.clone();
        let (closed_tx, closed_rx) = std::sync::mpsc::channel();
        let closer = std::thread::spawn(move || {
            worker_selection.close_lane();
            closed_tx.send(()).unwrap();
        });
        closed_rx.recv().unwrap();
        closer.join().unwrap();
        let before = selection.snapshot();
        assert_eq!(selection.request(2), AudioTrackRequestOutcome::Rejected);
        assert_eq!(selection.request(1), AudioTrackRequestOutcome::Rejected);
        assert_eq!(selection.snapshot(), before);
    }
}
