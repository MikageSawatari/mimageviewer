//! Per-viewer rotation memo and its cancellable still-seek reader.
//!
//! Keeping the pending read inside the existing cache owner makes swaps, clears,
//! replacement by an import snapshot, and drop carry the complete read lifetime.
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
    mpsc,
};

use crate::rotation_db::{Rotation, RotationDb};

type RotationRequest = Vec<(usize, String)>;

struct StillSeekRotationsPending {
    cancel: Arc<AtomicBool>,
    rx: mpsc::Receiver<Vec<(usize, Rotation)>>,
    generation: u64,
    requests: RotationRequest,
}

impl Drop for StillSeekRotationsPending {
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::Relaxed);
    }
}

#[derive(Default)]
pub(crate) struct RotationCache {
    values: HashMap<usize, Rotation>,
    pending: Option<StillSeekRotationsPending>,
    /// A memo of a completely hydrated immutable nav list, not a request state.
    /// It avoids walking the whole book in every otherwise idle overlay frame.
    verified_nav: Option<Arc<Vec<usize>>>,
}

impl Clone for RotationCache {
    fn clone(&self) -> Self {
        // A newly forked viewer inherits known values, never another viewer's
        // receiver/cancellation token. Its own first draw requests missing keys.
        Self::from(self.values.clone())
    }
}

impl From<HashMap<usize, Rotation>> for RotationCache {
    fn from(values: HashMap<usize, Rotation>) -> Self {
        Self {
            values,
            pending: None,
            verified_nav: None,
        }
    }
}

// Read-only map access is safe; mutations must go through the owner below so
// an invalidated value can never be repopulated by a pre-invalidation read.
impl std::ops::Deref for RotationCache {
    type Target = HashMap<usize, Rotation>;
    fn deref(&self) -> &Self::Target {
        &self.values
    }
}

impl RotationCache {
    pub(crate) fn has_rotations_for_nav(&mut self, nav: &Arc<Vec<usize>>) -> bool {
        if self
            .verified_nav
            .as_ref()
            .is_some_and(|known| Arc::ptr_eq(known, nav))
        {
            return true;
        }
        if nav.iter().all(|idx| self.values.contains_key(idx)) {
            self.verified_nav = Some(Arc::clone(nav));
            true
        } else {
            false
        }
    }
    #[cfg(test)]
    pub(crate) fn pending_cancel_for_test(&self) -> Option<Arc<AtomicBool>> {
        self.pending.as_ref().map(|p| Arc::clone(&p.cancel))
    }

    #[cfg(test)]
    pub(crate) fn wait_for_result_for_test(&mut self) {
        let pending = self.pending.as_mut().expect("pending rotation worker");
        let result = pending
            .rx
            .recv_timeout(std::time::Duration::from_secs(10))
            .expect("rotation result");
        let (tx, rx) = mpsc::channel();
        tx.send(result).unwrap();
        pending.rx = rx;
    }
    pub(crate) fn insert(&mut self, idx: usize, rotation: Rotation) -> Option<Rotation> {
        self.values.insert(idx, rotation)
    }

    pub(crate) fn clear(&mut self) {
        self.cancel_still_seek_rotations();
        self.verified_nav = None;
        self.values.clear();
    }

    pub(crate) fn remove(&mut self, idx: &usize) -> Option<Rotation> {
        self.verified_nav = None;
        if self
            .pending
            .as_ref()
            .is_some_and(|p| p.requests.iter().any(|(i, _)| i == idx))
        {
            self.cancel_still_seek_rotations();
        }
        self.values.remove(idx)
    }

    pub(crate) fn cancel_still_seek_rotations(&mut self) {
        self.pending = None;
    }

    /// Poll only merges memory. A newer in-session rotation always wins.
    pub(crate) fn poll_still_seek_rotations(&mut self, generation: u64) -> Vec<usize> {
        let Some(pending) = self.pending.as_ref() else {
            return Vec::new();
        };
        if pending.generation != generation {
            self.cancel_still_seek_rotations();
            return Vec::new();
        }
        let result = match pending.rx.try_recv() {
            Ok(result) => result,
            Err(mpsc::TryRecvError::Empty) => return Vec::new(),
            Err(mpsc::TryRecvError::Disconnected) => {
                // A failed spawn/panic is terminal for these requested values,
                // just as a failed DB read is. Do not respawn every idle frame.
                pending
                    .requests
                    .iter()
                    .map(|(idx, _)| (*idx, Rotation::None))
                    .collect()
            }
        };
        self.pending = None;
        result
            .into_iter()
            .filter_map(|(idx, rotation)| {
                if let std::collections::hash_map::Entry::Vacant(entry) = self.values.entry(idx) {
                    entry.insert(rotation);
                    Some(idx)
                } else {
                    None
                }
            })
            .collect()
    }

    /// Requests are a bounded snapshot of page keys, never borrowed App/DB handles.
    pub(crate) fn start_still_seek_rotations(
        &mut self,
        generation: u64,
        requests: RotationRequest,
        db_path: PathBuf,
        ctx: &egui::Context,
    ) {
        if requests.is_empty() {
            self.cancel_still_seek_rotations();
            return;
        }
        if self.pending.as_ref().is_some_and(|p| {
            p.generation == generation
                && requests.iter().all(|request| p.requests.contains(request))
        }) {
            return;
        }
        self.cancel_still_seek_rotations();
        let cancel = Arc::new(AtomicBool::new(false));
        let cancel_w = Arc::clone(&cancel);
        let (tx, rx) = mpsc::channel();
        let input = requests.clone();
        let wake = ctx.clone();
        let viewport = ctx.viewport_id();
        let _ = std::thread::Builder::new()
            .name("still-seek-rotations".into())
            .spawn(move || {
                if cancel_w.load(Ordering::Relaxed) {
                    return;
                }
                let started = std::time::Instant::now();
                let db = RotationDb::open_readonly_at(&db_path);
                let mut error = db.as_ref().err().map(ToString::to_string);
                let mut result = Vec::with_capacity(input.len());
                for (idx, key) in input {
                    if cancel_w.load(Ordering::Relaxed) {
                        return;
                    }
                    let rotation = match db.as_ref() {
                        Ok(db) => match db.get_key_checked(&key) {
                            Ok(rotation) => rotation.unwrap_or(Rotation::None),
                            Err(e) => {
                                error.get_or_insert_with(|| e.to_string());
                                Rotation::None
                            }
                        },
                        Err(_) => Rotation::None,
                    };
                    result.push((idx, rotation));
                }
                if cancel_w.load(Ordering::Relaxed) {
                    return;
                }
                if let Some(error) = &error {
                    crate::logger::log(format!(
                        "still-seek rotation read failed; using no rotation: {error}"
                    ));
                }
                if crate::perf::is_enabled() {
                    crate::perf::event(
                        "still_seek",
                        "rotation_read",
                        None,
                        generation,
                        &[
                            (
                                "ms",
                                serde_json::json!(started.elapsed().as_secs_f64() * 1000.0),
                            ),
                            ("items", serde_json::json!(result.len())),
                            ("failed", serde_json::json!(error.is_some())),
                        ],
                    );
                }
                if tx.send(result).is_ok() {
                    wake.request_repaint_of(viewport);
                }
            });
        self.pending = Some(StillSeekRotationsPending {
            cancel,
            rx,
            generation,
            requests,
        });
        // Also covers spawn failure. Completion wakes its originating viewport;
        // no repaint spin or UI-side wait is needed while SQLite is busy.
        ctx.request_repaint();
    }
}
