//! Per-player Norm resolution. The pump looks up the gain by the decoded frame's stream.

use std::collections::HashMap;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};

use super::clock::clamp_normalize_gain;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct NormalizeLookupRequest {
    pub table_epoch: u64,
    pub request_seq: u64,
    pub stream_index: usize,
    pub target_lufs_milli: i32,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum NormalizeTrackGain {
    Pending(Option<NormalizeLookupRequest>),
    Gain(f64),
}

struct Inner {
    epoch: u64,
    next_request_seq: u64,
    default: NormalizeTrackGain,
    explicit: HashMap<usize, NormalizeTrackGain>,
}

pub(crate) struct NormalizeGainTable(Mutex<Inner>);
static NEXT_TABLE_EPOCH: AtomicU64 = AtomicU64::new(1);

impl NormalizeGainTable {
    pub fn new(enabled: bool, initial_gain: f64) -> Self {
        Self(Mutex::new(Inner {
            epoch: NEXT_TABLE_EPOCH.fetch_add(1, Ordering::Relaxed),
            next_request_seq: 1,
            default: if enabled {
                NormalizeTrackGain::Pending(None)
            } else {
                NormalizeTrackGain::Gain(clamp_normalize_gain(initial_gain))
            },
            explicit: HashMap::new(),
        }))
    }

    pub fn reset(&self, enabled: bool) {
        let mut inner = self.0.lock().unwrap();
        inner.epoch = NEXT_TABLE_EPOCH.fetch_add(1, Ordering::Relaxed);
        inner.explicit.clear();
        inner.default = if enabled {
            NormalizeTrackGain::Pending(None)
        } else {
            NormalizeTrackGain::Gain(1.0)
        };
    }

    /// A live player moved to another viewer must issue its unfinished lookups
    /// from the new owner. Preserve resolved gains while invalidating old replies.
    pub fn rearm_pending_after_context_transfer(&self) -> Vec<usize> {
        let mut inner = self.0.lock().unwrap();
        let streams: Vec<usize> = inner
            .explicit
            .iter_mut()
            .filter_map(|(&stream, state)| {
                if matches!(state, NormalizeTrackGain::Pending(Some(_))) {
                    *state = NormalizeTrackGain::Pending(None);
                    Some(stream)
                } else {
                    None
                }
            })
            .collect();
        if !streams.is_empty() {
            inner.epoch = NEXT_TABLE_EPOCH.fetch_add(1, Ordering::Relaxed);
        }
        streams
    }

    pub fn get(&self, stream_index: usize) -> NormalizeTrackGain {
        let inner = self.0.lock().unwrap();
        inner
            .explicit
            .get(&stream_index)
            .copied()
            .unwrap_or(inner.default)
    }

    pub fn gain(&self, stream_index: usize) -> Option<f64> {
        match self.get(stream_index) {
            NormalizeTrackGain::Pending(_) => None,
            NormalizeTrackGain::Gain(gain) => Some(gain),
        }
    }

    pub fn begin_lookup(
        &self,
        stream_index: usize,
        target_lufs_milli: i32,
    ) -> Option<NormalizeLookupRequest> {
        let mut inner = self.0.lock().unwrap();
        if !matches!(
            inner
                .explicit
                .get(&stream_index)
                .copied()
                .unwrap_or(inner.default),
            NormalizeTrackGain::Pending(None)
        ) {
            return None;
        }
        let request = NormalizeLookupRequest {
            table_epoch: inner.epoch,
            request_seq: inner.next_request_seq,
            stream_index,
            target_lufs_milli,
        };
        inner.next_request_seq = inner.next_request_seq.wrapping_add(1);
        inner
            .explicit
            .insert(stream_index, NormalizeTrackGain::Pending(Some(request)));
        Some(request)
    }

    pub fn resolve(&self, request: NormalizeLookupRequest, gain: f64) -> bool {
        let mut inner = self.0.lock().unwrap();
        if inner.epoch != request.table_epoch
            || inner.explicit.get(&request.stream_index)
                != Some(&NormalizeTrackGain::Pending(Some(request)))
        {
            return false;
        }
        inner.explicit.insert(
            request.stream_index,
            NormalizeTrackGain::Gain(clamp_normalize_gain(gain)),
        );
        true
    }

    pub fn set_gain(&self, stream_index: usize, gain: f64) {
        self.0.lock().unwrap().explicit.insert(
            stream_index,
            NormalizeTrackGain::Gain(clamp_normalize_gain(gain)),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn requests_are_exact_and_off_invalidates_old_results() {
        let table = NormalizeGainTable::new(true, 1.0);
        let old = table.begin_lookup(2, -14000).unwrap();
        assert!(table.begin_lookup(2, -14000).is_none());
        assert!(table.gain(2).is_none());
        table.reset(false);
        assert_eq!(table.gain(2), Some(1.0));
        table.reset(true);
        let new = table.begin_lookup(2, -16000).unwrap();
        assert!(!table.resolve(old, 2.0));
        assert_eq!(table.gain(2), None);
        assert!(table.resolve(new, 3.0));
        assert_eq!(table.gain(2), Some(3.0));
        assert_eq!(table.gain(1), None);

        table.reset(true);
        let superseded_by_scan = table.begin_lookup(1, -14000).unwrap();
        table.set_gain(1, 1.0);
        assert!(!table.resolve(superseded_by_scan, 4.0));
        assert_eq!(table.gain(1), Some(1.0));
    }
}
