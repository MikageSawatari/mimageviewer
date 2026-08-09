//! `items` の index 空間に属する一時状態へ viewer-context 世代を刻む。

use std::collections::HashMap;

struct Entry<T> {
    items_generation: u64,
    value: T,
}

fn ignore_discard<T>(_: &mut T) {}

fn log_stale(cache_name: &str, idx: usize, expected: u64, actual: u64) {
    let message = format!(
        "[fs-generation] stale entry discarded cache={cache_name} idx={idx} expected_generation={expected} actual_generation={actual}"
    );
    crate::logger::log(message);
}

/// 1 つの viewer context が所有する idx 状態を世代付きで保持する map。
pub(crate) struct ItemsGenerationMap<T> {
    cache_name: &'static str,
    items_generation: u64,
    entries: HashMap<usize, Entry<T>>,
    on_discard: fn(&mut T),
}

pub(crate) struct ItemsGenerationMapIter<'a, T> {
    cache_name: &'static str,
    expected_generation: u64,
    inner: std::collections::hash_map::Iter<'a, usize, Entry<T>>,
}

impl<'a, T> Iterator for ItemsGenerationMapIter<'a, T> {
    type Item = (&'a usize, &'a T);

    fn next(&mut self) -> Option<Self::Item> {
        for (idx, entry) in self.inner.by_ref() {
            if entry.items_generation == self.expected_generation {
                return Some((idx, &entry.value));
            }
            log_stale(
                self.cache_name,
                *idx,
                self.expected_generation,
                entry.items_generation,
            );
        }
        None
    }
}

impl<T> ItemsGenerationMap<T> {
    pub(crate) fn new(cache_name: &'static str) -> Self {
        Self::with_discard(cache_name, ignore_discard::<T>)
    }

    pub(crate) fn with_discard(cache_name: &'static str, on_discard: fn(&mut T)) -> Self {
        Self {
            cache_name,
            items_generation: 0,
            entries: HashMap::new(),
            on_discard,
        }
    }

    pub(crate) fn set_items_generation(&mut self, items_generation: u64) {
        if self.items_generation == items_generation {
            return;
        }
        self.items_generation = items_generation;
        let cache_name = self.cache_name;
        let on_discard = self.on_discard;
        self.entries.retain(|&idx, entry| {
            let keep = entry.items_generation == items_generation;
            if !keep {
                log_stale(cache_name, idx, items_generation, entry.items_generation);
                on_discard(&mut entry.value);
            }
            keep
        });
    }

    #[cfg(test)]
    pub(crate) fn current_items_generation(&self) -> u64 {
        self.items_generation
    }

    fn generation_matches(&self, idx: usize, actual_generation: u64) -> bool {
        let matches = actual_generation == self.items_generation;
        if !matches {
            log_stale(
                self.cache_name,
                idx,
                self.items_generation,
                actual_generation,
            );
        }
        matches
    }

    pub(crate) fn accepts_generation(&self, idx: usize, actual_generation: u64) -> bool {
        self.generation_matches(idx, actual_generation)
    }

    pub(crate) fn get(&self, idx: &usize) -> Option<&T> {
        let entry = self.entries.get(idx)?;
        self.generation_matches(*idx, entry.items_generation)
            .then_some(&entry.value)
    }

    pub(crate) fn get_mut(&mut self, idx: &usize) -> Option<&mut T> {
        let entry = self.entries.get(idx)?;
        if entry.items_generation != self.items_generation {
            log_stale(
                self.cache_name,
                *idx,
                self.items_generation,
                entry.items_generation,
            );
            return None;
        }
        self.entries.get_mut(idx).map(|entry| &mut entry.value)
    }

    pub(crate) fn contains_key(&self, idx: &usize) -> bool {
        self.get(idx).is_some()
    }

    pub(crate) fn insert(&mut self, idx: usize, value: T) {
        self.insert_for_generation(idx, self.items_generation, value);
    }

    pub(crate) fn insert_for_generation(
        &mut self,
        idx: usize,
        actual_generation: u64,
        mut value: T,
    ) -> bool {
        if !self.generation_matches(idx, actual_generation) {
            (self.on_discard)(&mut value);
            return false;
        }
        if let Some(mut replaced) = self.entries.insert(
            idx,
            Entry {
                items_generation: actual_generation,
                value,
            },
        ) {
            (self.on_discard)(&mut replaced.value);
        }
        true
    }

    pub(crate) fn remove(&mut self, idx: &usize) -> Option<T> {
        let mut entry = self.entries.remove(idx)?;
        if !self.generation_matches(*idx, entry.items_generation) {
            (self.on_discard)(&mut entry.value);
            return None;
        }
        Some(entry.value)
    }

    pub(crate) fn clear(&mut self) {
        let on_discard = self.on_discard;
        for entry in self.entries.values_mut() {
            on_discard(&mut entry.value);
        }
        self.entries.clear();
    }

    pub(crate) fn len(&self) -> usize {
        self.iter().count()
    }

    #[cfg(test)]
    pub(crate) fn is_empty(&self) -> bool {
        self.iter().next().is_none()
    }

    pub(crate) fn iter(&self) -> ItemsGenerationMapIter<'_, T> {
        ItemsGenerationMapIter {
            cache_name: self.cache_name,
            expected_generation: self.items_generation,
            inner: self.entries.iter(),
        }
    }

    pub(crate) fn iter_mut(&mut self) -> impl Iterator<Item = (&usize, &mut T)> {
        let expected = self.items_generation;
        let cache_name = self.cache_name;
        self.entries.iter_mut().filter_map(move |(idx, entry)| {
            if entry.items_generation == expected {
                Some((&*idx, &mut entry.value))
            } else {
                log_stale(cache_name, *idx, expected, entry.items_generation);
                None
            }
        })
    }

    pub(crate) fn iter_with_generation(&self) -> impl Iterator<Item = (&usize, u64, &T)> {
        self.entries.iter().filter_map(|(idx, entry)| {
            self.generation_matches(*idx, entry.items_generation)
                .then_some((idx, entry.items_generation, &entry.value))
        })
    }

    pub(crate) fn values(&self) -> impl Iterator<Item = &T> {
        self.iter().map(|(_, value)| value)
    }

    pub(crate) fn keys(&self) -> impl Iterator<Item = &usize> {
        self.iter().map(|(idx, _)| idx)
    }

    pub(crate) fn drain(&mut self) -> impl Iterator<Item = (usize, T)> + '_ {
        let expected = self.items_generation;
        let cache_name = self.cache_name;
        let on_discard = self.on_discard;
        self.entries.drain().filter_map(move |(idx, mut entry)| {
            if entry.items_generation == expected {
                Some((idx, entry.value))
            } else {
                log_stale(cache_name, idx, expected, entry.items_generation);
                on_discard(&mut entry.value);
                None
            }
        })
    }

    pub(crate) fn retain(&mut self, mut keep: impl FnMut(&usize, &mut T) -> bool) {
        let expected = self.items_generation;
        let cache_name = self.cache_name;
        let on_discard = self.on_discard;
        self.entries.retain(|idx, entry| {
            let retain = if entry.items_generation == expected {
                keep(idx, &mut entry.value)
            } else {
                log_stale(cache_name, *idx, expected, entry.items_generation);
                false
            };
            if !retain {
                on_discard(&mut entry.value);
            }
            retain
        });
    }

    pub(crate) fn take_values(&mut self) -> Vec<(usize, T)> {
        self.drain().collect()
    }

    pub(crate) fn extend(&mut self, entries: impl IntoIterator<Item = (usize, T)>) {
        for (idx, value) in entries {
            self.insert(idx, value);
        }
    }
}

impl<'a, T> IntoIterator for &'a ItemsGenerationMap<T> {
    type Item = (&'a usize, &'a T);
    type IntoIter = ItemsGenerationMapIter<'a, T>;

    fn into_iter(self) -> Self::IntoIter {
        self.iter()
    }
}

/// item index を含む FIFO 状態を、1 つの items 世代に限定して保持する。
pub(crate) struct ItemsGenerationVec<T> {
    cache_name: &'static str,
    items_generation: u64,
    entries: Vec<Entry<T>>,
    idx_of: fn(&T) -> usize,
}

impl<T> ItemsGenerationVec<T> {
    pub(crate) fn new(cache_name: &'static str, idx_of: fn(&T) -> usize) -> Self {
        Self {
            cache_name,
            items_generation: 0,
            entries: Vec::new(),
            idx_of,
        }
    }

    pub(crate) fn set_items_generation(&mut self, items_generation: u64) {
        if self.items_generation == items_generation {
            return;
        }
        self.items_generation = items_generation;
        let cache_name = self.cache_name;
        let idx_of = self.idx_of;
        self.entries.retain(|entry| {
            let keep = entry.items_generation == items_generation;
            if !keep {
                log_stale(
                    cache_name,
                    idx_of(&entry.value),
                    items_generation,
                    entry.items_generation,
                );
            }
            keep
        });
    }

    #[cfg(test)]
    pub(crate) fn current_items_generation(&self) -> u64 {
        self.items_generation
    }

    pub(crate) fn accepts_generation(&self, idx: usize, actual_generation: u64) -> bool {
        let matches = actual_generation == self.items_generation;
        if !matches {
            log_stale(
                self.cache_name,
                idx,
                self.items_generation,
                actual_generation,
            );
        }
        matches
    }

    #[cfg(test)]
    pub(crate) fn push(&mut self, value: T) {
        self.entries.push(Entry {
            items_generation: self.items_generation,
            value,
        });
    }

    pub(crate) fn push_for_generation(&mut self, actual_generation: u64, value: T) -> bool {
        if !self.accepts_generation((self.idx_of)(&value), actual_generation) {
            return false;
        }
        self.entries.push(Entry {
            items_generation: actual_generation,
            value,
        });
        true
    }

    pub(crate) fn replace_for_generation(
        &mut self,
        position: usize,
        actual_generation: u64,
        value: T,
    ) -> bool {
        if !self.accepts_generation((self.idx_of)(&value), actual_generation) {
            return false;
        }
        let entry = &mut self.entries[position];
        if entry.items_generation != self.items_generation {
            log_stale(
                self.cache_name,
                (self.idx_of)(&entry.value),
                self.items_generation,
                entry.items_generation,
            );
            return false;
        }
        entry.items_generation = actual_generation;
        entry.value = value;
        true
    }

    pub(crate) fn iter(&self) -> impl Iterator<Item = &T> {
        let expected = self.items_generation;
        let cache_name = self.cache_name;
        let idx_of = self.idx_of;
        self.entries.iter().filter_map(move |entry| {
            if entry.items_generation == expected {
                Some(&entry.value)
            } else {
                log_stale(
                    cache_name,
                    idx_of(&entry.value),
                    expected,
                    entry.items_generation,
                );
                None
            }
        })
    }

    pub(crate) fn remove_with_generation(&mut self, position: usize) -> (u64, T) {
        let entry = self.entries.remove(position);
        (entry.items_generation, entry.value)
    }

    pub(crate) fn retain(&mut self, mut keep: impl FnMut(&T) -> bool) {
        let expected = self.items_generation;
        let cache_name = self.cache_name;
        let idx_of = self.idx_of;
        self.entries.retain(|entry| {
            if entry.items_generation != expected {
                log_stale(
                    cache_name,
                    idx_of(&entry.value),
                    expected,
                    entry.items_generation,
                );
                return false;
            }
            keep(&entry.value)
        });
    }

    pub(crate) fn clear(&mut self) {
        self.entries.clear();
    }

    pub(crate) fn len(&self) -> usize {
        self.iter().count()
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.iter().next().is_none()
    }
}

#[cfg(test)]
mod tests {
    use super::{Entry, ItemsGenerationMap};

    #[test]
    fn matching_generation_returns_entry() {
        let mut cache = ItemsGenerationMap::new("test_cache");
        cache.set_items_generation(7);
        cache.insert(3, 42);

        assert_eq!(cache.get(&3), Some(&42));
    }

    #[test]
    fn mismatched_generation_fails_closed() {
        let mut cache = ItemsGenerationMap::new("test_cache");
        cache.items_generation = 8;
        cache.entries.insert(
            3,
            Entry {
                items_generation: 7,
                value: 42,
            },
        );

        assert_eq!(cache.get(&3), None);
    }

    #[test]
    fn generation_update_discards_old_entries() {
        let mut cache = ItemsGenerationMap::new("test_cache");
        cache.set_items_generation(7);
        cache.insert(3, 42);

        cache.set_items_generation(8);

        assert_eq!(cache.get(&3), None);
        assert!(cache.is_empty());
    }
}
