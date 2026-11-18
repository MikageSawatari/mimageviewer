//! A source table and its DB observation move as one context-owned value.
use super::{App, ConvertedArchiveSourceState, ThumbnailState};
use crate::archive_cache::{ArchiveCacheChange, ArchiveCacheDb};
use crate::thumb_loader::ConvertedSourceBinding;
use std::collections::{HashMap, HashSet};
use std::path::Path;

#[cfg(test)]
thread_local! {
    static THUMBNAIL_VISITS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
    static SOURCE_STATE_VISITS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

#[cfg(test)]
pub(super) fn observe_thumbnail_visit() {
    THUMBNAIL_VISITS.with(|count| count.set(count.get() + 1));
}
#[cfg(test)]
pub(super) fn take_thumbnail_visits() -> usize {
    THUMBNAIL_VISITS.with(|count| count.replace(0))
}
#[cfg(test)]
fn observe_source_state_visit() {
    SOURCE_STATE_VISITS.with(|count| count.set(count.get() + 1));
}
#[cfg(test)]
pub(super) fn take_source_state_visits() -> usize {
    SOURCE_STATE_VISITS.with(|count| count.replace(0))
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct ConvertedArchiveSources {
    states: HashMap<String, ConvertedArchiveSourceState>,
    synced_rev: u64,
}

impl ConvertedArchiveSources {
    pub(crate) fn new(synced_rev: u64) -> Self {
        Self {
            states: HashMap::new(),
            synced_rev,
        }
    }

    pub(crate) fn synced_revision(&self) -> u64 {
        self.synced_rev
    }

    fn apply(&mut self, revision: u64, changes: &[(u64, ArchiveCacheChange)]) -> bool {
        let changed = revision > self.synced_rev;
        let mut source_keys = HashSet::new();
        let mut all_cached = false;
        for (_, change) in changes.iter().filter(|(rev, _)| *rev > self.synced_rev) {
            match change {
                ArchiveCacheChange::Source(source) => {
                    source_keys.insert(source.as_str());
                }
                ArchiveCacheChange::AllCached => all_cached = true,
            }
        }
        // Every change can only move a state to Pending, so the invalidations commute.
        // Gather/deduplicate the log first; traverse the candidates only once.
        if all_cached || !source_keys.is_empty() {
            for (key, state) in &mut self.states {
                #[cfg(test)]
                observe_source_state_visit();
                let invalid = (all_cached
                    && matches!(
                        state.read_source(),
                        ConvertedArchiveSourceState::CachedZip { .. }
                    ))
                    || (!source_keys.is_empty()
                        && (source_keys
                            .contains(crate::path_key::normalize(Path::new(key)).as_str())
                            || match state.read_source() {
                                ConvertedArchiveSourceState::Direct(path) => {
                                    source_keys.contains(crate::path_key::normalize(path).as_str())
                                }
                                ConvertedArchiveSourceState::CachedZip {
                                    logical_source, ..
                                }
                                | ConvertedArchiveSourceState::Unavailable {
                                    logical_source: Some(logical_source),
                                } => source_keys
                                    .contains(crate::path_key::normalize(logical_source).as_str()),
                                _ => false,
                            }));
                if invalid {
                    *state = ConvertedArchiveSourceState::Pending;
                }
            }
        }
        self.synced_rev = revision;
        changed
    }

    pub(crate) fn catch_up(&mut self, db: Option<&ArchiveCacheDb>) -> bool {
        let Some(db) = db else {
            return false;
        };
        if self.synced_rev == db.revision() {
            return false;
        }
        let (revision, changes) = db.changes_since(self.synced_rev);
        self.apply(revision, &changes)
    }

    pub(crate) fn merge_caught_up(&mut self, mut other: Self, db: Option<&ArchiveCacheDb>) -> bool {
        let changed = if let Some(db) = db {
            let (revision, changes) = db.changes_since(self.synced_rev.min(other.synced_rev));
            let changed = self.apply(revision, &changes);
            other.apply(revision, &changes);
            changed
        } else {
            false
        };
        self.states.extend(other.states);
        changed
    }
}

impl std::ops::Deref for ConvertedArchiveSources {
    type Target = HashMap<String, ConvertedArchiveSourceState>;
    fn deref(&self) -> &Self::Target {
        &self.states
    }
}
impl std::ops::DerefMut for ConvertedArchiveSources {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.states
    }
}
#[cfg(test)]
impl FromIterator<(String, ConvertedArchiveSourceState)> for ConvertedArchiveSources {
    fn from_iter<T: IntoIterator<Item = (String, ConvertedArchiveSourceState)>>(iter: T) -> Self {
        Self {
            states: iter.into_iter().collect(),
            synced_rev: 0,
        }
    }
}

/// Only the worker's actual branch establishes what the pixels depend on.
pub(crate) fn binding_is_stale(
    binding: Option<&ConvertedSourceBinding>,
    sources: &ConvertedArchiveSources,
) -> bool {
    binding.is_some_and(|binding| {
        sources
            .get(&binding.archive_key)
            .and_then(ConvertedArchiveSourceState::load_path)
            .is_some_and(|path| crate::path_key::normalize_keep_drive(path) != binding.read_archive)
    })
}

impl App {
    /// Publication can arrive in a reply drain or a prepared membership map.
    /// Scan only once per publication and evict each requested/stale tile once.
    pub(super) fn reconcile_converted_archive_bindings(
        &mut self,
        published_keys: &HashSet<String>,
        mut reload_indices: HashSet<usize>,
    ) {
        if !published_keys.is_empty() {
            for (idx, thumbnail) in self.thumbnails.iter().enumerate() {
                #[cfg(test)]
                observe_thumbnail_visit();
                if matches!(thumbnail, ThumbnailState::Loaded {
                    converted_source_binding: Some(binding), ..
                } if published_keys.contains(&binding.archive_key))
                    && self.converted_thumbnail_is_stale(thumbnail)
                {
                    reload_indices.insert(idx);
                }
            }
        }
        for idx in reload_indices {
            self.evict_thumbnail_for_reload(idx);
        }
    }

    pub(crate) fn catch_up_converted_archive_sources(&mut self) {
        if self
            .converted_archive_cache_paths
            .catch_up(self.archive_cache_db.as_deref())
            && let Some(pending) = self.converted_archive_cache_paths_pending.take()
        {
            pending.cancel();
        }
    }

    pub(crate) fn converted_thumbnail_is_stale(&self, thumbnail: &ThumbnailState) -> bool {
        matches!(thumbnail, ThumbnailState::Loaded { converted_source_binding, .. }
            if binding_is_stale(converted_source_binding.as_ref(), &self.converted_archive_cache_paths))
    }
}
