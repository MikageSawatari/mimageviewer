//! §1.886: batched cache-key adoption. No filesystem or database reads on the UI thread.
use super::App;
use std::collections::{BTreeSet, HashMap, HashSet};
use std::path::{Path, PathBuf};

#[cfg(test)]
thread_local! {
    pub(super) static SYNC_REHYDRATIONS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
    static LAST_BATCH_VISITS: std::cell::Cell<(usize, usize, usize)> = const { std::cell::Cell::new((0, 0, 0)) };
}

struct CacheKeyMappings {
    moves: HashMap<String, String>,
    roots: HashSet<String>,
    #[cfg(test)]
    visits: std::cell::Cell<usize>,
}

impl CacheKeyMappings {
    fn new(pairs: &[(PathBuf, PathBuf)]) -> Self {
        let mut moves = HashMap::with_capacity(pairs.len());
        let mut roots = HashSet::with_capacity(pairs.len() * 2);
        for (old, new) in pairs {
            let old = crate::adjustment_db::normalize_path(old);
            let new = crate::adjustment_db::normalize_path(new);
            roots.insert(old.clone());
            roots.insert(new.clone());
            if old != new {
                moves.insert(old, new);
            }
        }
        Self {
            moves,
            roots,
            #[cfg(test)]
            visits: std::cell::Cell::new(0),
        }
    }

    fn root<'a>(&self, key: &'a str) -> &'a str {
        #[cfg(test)]
        self.visits.set(self.visits.get() + 1);
        key.split("::").next().unwrap_or(key)
    }

    fn rewritten(&self, key: &str) -> Option<String> {
        let root = self.root(key);
        self.moves
            .get(root)
            .map(|new| format!("{new}{}", &key[root.len()..]))
    }

    fn contains(&self, key: &str) -> bool {
        self.roots.contains(self.root(key))
    }

    fn contains_path(&self, path: &Path) -> bool {
        self.contains(&crate::adjustment_db::normalize_path(path))
    }

    fn rewrite_set(&self, set: &mut BTreeSet<String>) {
        *set = std::mem::take(set)
            .into_iter()
            .map(|key| self.rewritten(&key).unwrap_or(key))
            .collect();
    }

    fn rewrite_map<T>(&self, map: &mut HashMap<String, T>) {
        let mut moved = Vec::new();
        map.retain(|key, _| {
            if let Some(destination) = self.rewritten(key) {
                moved.push((key.clone(), destination));
            }
            true
        });
        for (source, destination) in moved {
            if let Some(value) = map.remove(&source) {
                map.entry(destination).or_insert(value);
            }
        }
    }
}

impl App {
    pub(crate) fn rewrite_converted_archive_presence(&mut self, pairs: &[(PathBuf, PathBuf)]) {
        if pairs.is_empty() {
            return;
        }
        self.rewrite_converted_archive_presence_with_map(&CacheKeyMappings::new(pairs));
    }

    fn rewrite_converted_archive_presence_with_map(&mut self, keys: &CacheKeyMappings) {
        for set in [
            &mut self.adjusted_page_keys,
            &mut self.local_adjust_page_keys,
            &mut self.mask_page_keys,
            &mut self.conceal_page_keys,
            &mut self.comic_page_keys,
            &mut self.export_crop_page_keys,
        ] {
            keys.rewrite_set(set);
        }
        let mut rotations = Vec::new();
        for (key, angle) in &self.rotation_page_keys {
            if let Some(destination) = keys.rewritten(key) {
                if !self.rotation_page_keys.contains_key(&destination) {
                    rotations.push((destination, *angle));
                }
                rotations.push((key.clone(), crate::rotation_db::Rotation::None));
            }
        }
        self.update_rotation_page_owner(rotations);
    }

    pub(super) fn apply_converted_archive_rename_completion(
        &mut self,
        pairs: &[(PathBuf, PathBuf)],
    ) {
        if pairs.is_empty() {
            return;
        }
        let keys = CacheKeyMappings::new(pairs);
        self.rewrite_converted_archive_presence_with_map(&keys);
        #[cfg(test)]
        let presence_visits = keys.visits.get();
        keys.rewrite_map(&mut self.rating_session_writes);
        #[cfg(test)]
        let rating_visits = keys.visits.get() - presence_visits;
        let meter_roots = pairs
            .iter()
            .flat_map(|(old, new)| [old, new])
            .map(|path| crate::path_key::normalize(path))
            .collect();
        self.book_resume_meters.remove_converted_roots(meter_roots);
        self.refresh_converted_archive_context(&keys);
        #[cfg(test)]
        LAST_BATCH_VISITS.with(|stats| {
            stats.set((
                presence_visits,
                rating_visits,
                keys.visits.get() - presence_visits - rating_visits,
            ))
        });
        #[cfg(windows)]
        {
            let references = |context: super::viewer_context_registry::ContextRef<'_>| {
                context
                    .current_folder()
                    .is_some_and(|path| keys.contains_path(path))
                    || context.items().iter().any(|item| {
                        item.drag_source_path()
                            .is_some_and(|path| keys.contains_path(path))
                    })
            };
            if self.active_viewer_context_id().is_some_and(|id| {
                self.with_viewer_context_ref(id, references)
                    .unwrap_or(false)
            }) {
                self.with_active_viewer_context(|app| app.refresh_converted_archive_context(&keys));
            }
            let windows: Vec<_> = self
                .detached_image_windows
                .iter()
                .filter(|window| {
                    self.locate_window_context(window.id)
                        .is_some_and(|(id, _)| {
                            self.with_viewer_context_ref(id, references)
                                .unwrap_or(false)
                        })
                })
                .map(|window| window.id)
                .collect();
            for window in windows {
                self.with_window_viewer_context(window, |app| {
                    app.refresh_converted_archive_context(&keys)
                })
                .unwrap_or_else(|error| {
                    panic!("converted rename context {window} failed to mount: {error:?}")
                });
            }
        }
    }

    fn refresh_converted_archive_context(&mut self, keys: &CacheKeyMappings) {
        // Visit the preview map once, including inherited pinned-page previews.
        let affected: Vec<_> = self
            .thumb_edit_preview_keys
            .iter()
            .filter_map(|(&index, key)| keys.contains(key).then_some(index))
            .collect();
        for index in affected {
            self.evict_thumbnail_for_reload(index);
        }
        let referenced = self
            .current_folder
            .as_deref()
            .is_some_and(|path| keys.contains_path(path))
            || self.items.iter().any(|item| {
                item.drag_source_path()
                    .is_some_and(|path| keys.contains_path(path))
            });
        if !referenced {
            return;
        }
        // ZIP enumeration carries no edit DB snapshot; its first hydration reads after
        // enumeration. Already running async edit prepares validate PAGE_EDIT_WRITES at
        // adoption. Do not hydrate the previous list or synchronously reread this context.
        if self.zip_enumerate_pending.is_none() {
            self.prepare_page_edits_for_current_virtual_items();
        }
        self.rating_cache.clear();
        self.current_folder_rating_cache = None;
        self.clear_tags_cache();
    }
}

#[cfg(test)]
mod tests;
