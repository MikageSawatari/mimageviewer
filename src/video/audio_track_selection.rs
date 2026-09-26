//! Per-player audio track selection. UI owns desired; demux owns applied and failures.

use std::sync::Mutex;

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
    pub open_notice: Option<AudioTrackOpenNotice>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AudioTrackSelectionDisplayState {
    Applied,
    Deferred,
    Switching,
    Failed(AudioTrackSwitchFailureReason),
}

impl AudioTrackSelectionSnapshot {
    pub fn display_state(self, deferred: bool) -> AudioTrackSelectionDisplayState {
        if self.desired.generation == self.applied.generation {
            AudioTrackSelectionDisplayState::Applied
        } else if let Some(failure) = self
            .last_failure
            .filter(|failure| failure.choice.generation == self.desired.generation)
        {
            AudioTrackSelectionDisplayState::Failed(failure.reason)
        } else if deferred {
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
                    open_notice: None,
                },
                lane: AudioLaneState::Active,
            }),
        }
    }

    pub(crate) fn snapshot(&self) -> AudioTrackSelectionSnapshot {
        self.state.lock().unwrap().snapshot
    }

    /// UI thread only. Admission and demux lane closure share one lock. The
    /// caller publishes the seek after an accepted request returns.
    pub(crate) fn request(&self, stream_index: usize) -> AudioTrackRequestOutcome {
        let mut state = self.state.lock().unwrap();
        if state.lane == AudioLaneState::Lost {
            return AudioTrackRequestOutcome::Rejected;
        }
        let snapshot = &mut state.snapshot;
        if snapshot.desired.stream_index == stream_index
            && snapshot
                .last_failure
                .is_none_or(|failure| failure.choice.generation != snapshot.desired.generation)
        {
            return AudioTrackRequestOutcome::Unchanged;
        }
        snapshot.desired.generation += 1;
        snapshot.desired.stream_index = stream_index;
        AudioTrackRequestOutcome::Accepted
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
