//! App-wide annotation fonts. Only this owner may change the current snapshot.
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::{Arc, LazyLock, mpsc};

#[derive(Default)]
enum Current {
    #[default]
    Unloaded,
    Ready {
        set: Option<Arc<comic_core::FontSet>>,
        loaded: HashSet<String>,
        attempted: HashSet<String>,
    },
}

struct Load {
    rx: mpsc::Receiver<FontLoadResult>,
    keys: HashSet<String>,
}

pub(super) struct FontLoadResult {
    set: Option<comic_core::FontSet>,
    loaded: HashSet<String>,
    attempted: HashSet<String>,
}

#[cfg(test)]
impl FontLoadResult {
    pub(super) fn for_test(
        keys: &HashSet<String>,
        set: Option<comic_core::FontSet>,
        loaded: HashSet<String>,
    ) -> Self {
        Self {
            set,
            loaded,
            attempted: keys.clone(),
        }
    }
}

#[derive(Clone, Copy)]
pub(crate) enum FontInvalidation {
    SourcesChanged,
    FontAdded,
}

#[cfg(test)]
type TestLoader = Arc<dyn Fn(&HashSet<String>) -> FontLoadResult + Send + Sync>;

#[derive(Default)]
pub(crate) struct ComicFontOwner {
    current: Current,
    load: Option<Load>,
    #[cfg(test)]
    test_loader: Option<TestLoader>,
}

impl ComicFontOwner {
    pub(crate) fn fonts(&self) -> &Option<Arc<comic_core::FontSet>> {
        match &self.current {
            Current::Unloaded => &None,
            Current::Ready { set, .. } => set,
        }
    }

    pub(crate) fn loaded_keys(&self) -> &HashSet<String> {
        static EMPTY: LazyLock<HashSet<String>> = LazyLock::new(HashSet::new);
        match &self.current {
            Current::Unloaded => &EMPTY,
            Current::Ready { loaded, .. } => loaded,
        }
    }

    // Some(empty) requests the default font. Ready(None) is terminal too.
    fn needed(&self, keys: &[String]) -> Option<HashSet<String>> {
        match &self.current {
            Current::Unloaded => Some(keys.iter().cloned().collect()),
            Current::Ready { set: None, .. } => None,
            Current::Ready {
                loaded, attempted, ..
            } => {
                let missing: HashSet<_> = keys
                    .iter()
                    .filter(|key| !loaded.contains(*key) && !attempted.contains(*key))
                    .cloned()
                    .collect();
                (!missing.is_empty()).then_some(missing)
            }
        }
    }

    pub(super) fn needs_sync(&self, keys: &[String]) -> bool {
        self.needed(keys).is_some()
    }

    fn adopt(&mut self, result: FontLoadResult) {
        let mut attempted = result.attempted;
        if let Current::Ready { attempted: old, .. } = &self.current {
            attempted.extend(old.iter().cloned());
        }
        self.current = Current::Ready {
            set: result.set.map(Arc::new),
            loaded: result.loaded,
            attempted,
        };
    }

    pub(crate) fn poll(&mut self) {
        let Some(load) = &self.load else { return };
        let result = match load.rx.try_recv() {
            Ok(result) => result,
            Err(mpsc::TryRecvError::Empty) => return,
            Err(mpsc::TryRecvError::Disconnected) => {
                crate::logger::log("[comic] font worker disconnected".to_owned());
                FontLoadResult {
                    set: None,
                    loaded: HashSet::new(),
                    attempted: load.keys.clone(),
                }
            }
        };
        self.load = None;
        self.adopt(result);
    }

    pub(crate) fn request(&mut self, ctx: &egui::Context, keys: &[String]) {
        if self.load.is_some() {
            return;
        }
        let Some(mut requested) = self.needed(keys) else {
            return;
        };
        requested.extend(self.loaded_keys().iter().cloned());
        let (tx, rx) = mpsc::channel();
        self.load = Some(Load {
            rx,
            keys: requested.clone(),
        });
        let ctx = ctx.clone();
        #[cfg(test)]
        let test_loader = self.test_loader.clone();
        let spawn = std::thread::Builder::new()
            .name("comic-font-load".to_owned())
            .spawn(move || {
                let started = std::time::Instant::now();
                #[cfg(test)]
                let result = if let Some(loader) = test_loader {
                    loader(&requested)
                } else {
                    load_enumerated(&requested)
                };
                #[cfg(not(test))]
                let result = load_enumerated(&requested);
                if crate::perf::is_enabled() {
                    crate::perf::event(
                        "comic",
                        "font_load",
                        None,
                        0,
                        &[
                            (
                                "ms",
                                serde_json::json!(started.elapsed().as_secs_f64() * 1000.0),
                            ),
                            ("key_count", serde_json::json!(requested.len())),
                            ("loaded_count", serde_json::json!(result.loaded.len())),
                            (
                                "result",
                                serde_json::json!(if result.set.is_some() {
                                    "ready"
                                } else {
                                    "unavailable"
                                }),
                            ),
                        ],
                    );
                }
                let _ = tx.send(result);
                ctx.request_repaint_of(egui::ViewportId::ROOT);
            });
        if let Err(error) = spawn {
            crate::logger::log(format!("[comic] font worker spawn: {error}"));
        }
    }

    // Display never waits for a worker: consume a ready result, or remove its
    // publication right before reading synchronously. Both paths accumulate attempts.
    pub(crate) fn ensure(&mut self, keys: &[String], paths: &HashMap<String, PathBuf>) {
        self.poll();
        let Some(mut requested) = self.needed(keys) else {
            return;
        };
        self.load = None;
        // Preserve the existing display fallback: an unknown path must not
        // cause a synchronous default-font reread. Recording attempts is still
        // a current mutation, so its old receiver was discarded above.
        if let Current::Ready {
            set: Some(_),
            attempted,
            ..
        } = &mut self.current
        {
            if requested.iter().all(|key| !paths.contains_key(key)) {
                attempted.extend(requested);
                return;
            }
        }
        requested.extend(self.loaded_keys().iter().cloned());
        #[cfg(test)]
        let result = if let Some(loader) = &self.test_loader {
            loader(&requested)
        } else {
            load_from_paths(&requested, paths)
        };
        #[cfg(not(test))]
        let result = load_from_paths(&requested, paths);
        self.adopt(result);
    }

    pub(crate) fn invalidate(&mut self, reason: FontInvalidation) {
        self.load = None;
        match reason {
            FontInvalidation::SourcesChanged => self.current = Current::Unloaded,
            FontInvalidation::FontAdded => {
                if let Current::Ready { attempted, .. } = &mut self.current {
                    attempted.clear();
                }
            }
        }
    }

    #[cfg(test)]
    pub(crate) fn seed_for_test(
        &mut self,
        set: Option<Arc<comic_core::FontSet>>,
        loaded: HashSet<String>,
    ) {
        self.load = None;
        self.current = Current::Ready {
            set,
            loaded,
            attempted: HashSet::new(),
        };
    }

    #[cfg(test)]
    pub(super) fn loader_for_test(&mut self, loader: TestLoader) {
        self.test_loader = Some(loader);
    }

    // Wait only in tests, then leave the result for the real nonblocking poll.
    #[cfg(test)]
    pub(super) fn wait_for_test(&mut self) {
        let load = self.load.as_mut().expect("font request");
        let result = load
            .rx
            .recv_timeout(std::time::Duration::from_secs(10))
            .expect("font worker result");
        let (tx, rx) = mpsc::channel();
        tx.send(result)
            .unwrap_or_else(|_| panic!("live font receiver"));
        load.rx = rx;
    }

    #[cfg(test)]
    pub(super) fn pending_for_test(&self) -> bool {
        self.load.is_some()
    }
}

fn load_enumerated(keys: &HashSet<String>) -> FontLoadResult {
    let (_, paths) = crate::font_assets::enumerate_comic_fonts();
    load_from_paths(keys, &paths)
}

fn load_from_paths(keys: &HashSet<String>, paths: &HashMap<String, PathBuf>) -> FontLoadResult {
    let result = assemble_fontset(keys, crate::comic_overlay::load_comic_fonts(), |key| {
        std::fs::read(paths.get(key)?).ok()
    });
    if result.set.is_none() {
        crate::logger::log("[comic] 注釈描画用の日本語フォントが見つかりませんでした".to_owned());
    }
    for key in keys.difference(&result.loaded) {
        crate::logger::log(format!("[comic] font unavailable: {key}"));
    }
    result
}

/// Shared assembly; I/O is supplied by the caller, never by the planner/poll.
fn assemble_fontset(
    keys: &HashSet<String>,
    default: Option<comic_core::FontSet>,
    mut bytes: impl FnMut(&str) -> Option<Vec<u8>>,
) -> FontLoadResult {
    let mut loaded = HashSet::new();
    let set = default.map(|mut set| {
        let mut sorted: Vec<_> = keys.iter().collect();
        sorted.sort();
        for key in sorted {
            if let Some(font) = bytes(key)
                .and_then(|bytes| comic_core::LoadedFont::from_bytes(key.clone(), bytes).ok())
            {
                set.insert(font);
                loaded.insert(key.clone());
            }
        }
        set
    });
    FontLoadResult {
        set,
        loaded,
        attempted: keys.clone(),
    }
}

impl super::App {
    pub(crate) fn invalidate_comic_fonts(&mut self, reason: FontInvalidation) {
        self.comic_fonts.invalidate(reason);
        self.comic_font_registry_loaded = false;
    }

    pub(super) fn request_comic_fonts_for_page(&mut self, ctx: &egui::Context, idx: usize) {
        let keys = self
            .page_path_key(idx)
            .and_then(|key| self.comic_docs.get(&key))
            .map(|doc| crate::comic_overlay::referenced_font_keys(doc))
            .unwrap_or_default();
        self.comic_fonts.request(ctx, &keys);
    }
}

#[cfg(test)]
mod tests;
