//! Worker-only pin cache preparation. Terminal data contains memory resources only.
use super::{CurrentViewRefresh, GridItem};
use crate::catalog::{CacheEntry, CatalogDb, FolderThumbProvenance};
use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, RwLock};

pub(crate) type CacheMap = Arc<RwLock<HashMap<String, CacheEntry>>>;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Identity {
    pub(crate) cache: Option<usize>,
    pub(crate) catalog: Option<usize>,
    pub(crate) sort: crate::settings::SortOrder,
    pub(crate) depth: u32,
    pub(crate) full_path: bool,
}

pub(crate) struct Row {
    pub(crate) index: usize,
    pub(crate) item: GridItem,
    pub(crate) metadata: Option<(i64, i64)>,
    pub(crate) container: std::path::PathBuf,
    pub(crate) dependency_root: std::path::PathBuf,
}

pub(crate) struct Request {
    pub(crate) rows: Vec<Row>,
    pub(crate) identity: Identity,
    pub(crate) cache: Option<(CacheMap, Arc<CatalogDb>)>,
    pub(crate) scope: CurrentViewRefresh,
}

impl Default for Request {
    fn default() -> Self {
        Self {
            rows: Vec::new(),
            identity: Identity {
                cache: None,
                catalog: None,
                sort: crate::settings::SortOrder::Numeric,
                depth: 0,
                full_path: false,
            },
            cache: None,
            scope: CurrentViewRefresh::Full,
        }
    }
}

impl std::fmt::Debug for Request {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PinMaterializationRequest")
            .field("rows", &self.rows.len())
            .field("identity", &self.identity)
            .finish()
    }
}

/// No filesystem paths to inspect, database handles, or I/O callbacks survive preparation.
pub(crate) struct Prepared {
    pub(crate) reset_indices: Vec<usize>,
    pub(crate) replacement: Option<CacheMap>,
    pub(crate) identity: Identity,
}

impl Prepared {
    pub(crate) fn matches(&self, identity: Identity) -> bool {
        self.identity == identity
    }
}

impl Request {
    /// Called by metadata_import_refresh::run, on the already owning worker.
    pub(crate) fn prepare(
        self,
        mut reset_indices: Vec<usize>,
        pins: &HashMap<String, crate::folder_thumb_pins::FolderPinSource>,
        pin_db: Option<&crate::folder_thumb_pins::FolderThumbPinDb>,
        video_db: Option<&crate::video_pins::VideoPinDb>,
        cancel: &AtomicBool,
    ) -> Option<Prepared> {
        use crate::folder_thumb_pins::ResolvedKind;
        let affected = |row: &Row| match &self.scope {
            CurrentViewRefresh::Full => true,
            CurrentViewRefresh::Pins { folders, videos } => {
                let root = &row.dependency_root;
                folders
                    .iter()
                    .any(|path| crate::books::path_is_under_or_equal(path, root))
                    || (!videos.is_empty()
                        && pins
                            .get(&crate::path_key::normalize_keep_drive(&row.container))
                            .and_then(|pin| {
                                crate::folder_thumb_pins::resolve_pin_target_cascaded_via(
                                    &row.container,
                                    pin,
                                    |path| pin_db.and_then(|db| db.lookup(path)),
                                    self.identity.depth as usize,
                                )
                            })
                            .is_some_and(|target| {
                                videos.iter().any(|path| {
                                    crate::path_key::eq_keep_drive(path, &target.abs_path)
                                })
                            }))
            }
        };
        let requested: HashSet<_> = reset_indices.iter().copied().collect();
        let mut rows = Vec::new();
        for row in &self.rows {
            if cancel.load(Ordering::Relaxed) {
                return None;
            }
            if requested.contains(&row.index) && affected(row) {
                rows.push(row);
            }
        }
        let selected: HashSet<_> = rows.iter().map(|row| row.index).collect();
        reset_indices.retain(|index| selected.contains(index));
        if reset_indices.is_empty() {
            return Some(Prepared {
                reset_indices,
                replacement: None,
                identity: self.identity,
            });
        }
        let Some((cache, catalog)) = &self.cache else {
            return Some(Prepared {
                reset_indices,
                replacement: None,
                identity: self.identity,
            });
        };
        // A private map prevents old thumbnail producers and rejected terminal results from
        // changing the adopted context's cache. Never hold a live-map lock across I/O.
        let mut map = cache.read().ok()?.clone();
        let mut prefixes = HashSet::new();
        let mut retained = HashSet::new();
        for row in &rows {
            if cancel.load(Ordering::Relaxed) {
                return None;
            }
            let keys = super::folder_thumb_existing_keys_for(
                &row.item,
                row.metadata,
                pins,
                pin_db,
                Some(self.identity.sort),
                self.identity.depth,
                self.identity.full_path,
            );
            if let Some(base) = keys.first() {
                prefixes.insert(format!(
                    "{base}{}",
                    crate::thumb_loader::CACHE_KEY_PIN_SUFFIX
                ));
            }
            retained.extend(keys.into_iter().skip(1));
        }
        let mut deletes: HashSet<_> = map
            .keys()
            .filter(|key| {
                prefixes.iter().any(|prefix| key.starts_with(prefix)) && !retained.contains(*key)
            })
            .cloned()
            .collect();
        let mut seeds = Vec::new();
        for row in rows {
            let Some(video_db) = video_db else {
                continue;
            };
            if cancel.load(Ordering::Relaxed) {
                return None;
            }
            let GridItem::Folder(path) = &row.item else {
                continue;
            };
            let Some(source) = pins.get(&crate::path_key::normalize_keep_drive(path)) else {
                continue;
            };
            let Some(resolved) = crate::folder_thumb_pins::resolve_pin_target_cascaded_via(
                path,
                source,
                |path| pin_db.and_then(|db| db.lookup(path)),
                self.identity.depth as usize,
            ) else {
                continue;
            };
            if resolved.kind != ResolvedKind::Video {
                continue;
            }
            let Some(base) = crate::thumb_loader::folder_thumb_cache_key_for_path(
                path,
                self.identity.full_path,
                self.identity.sort,
                self.identity.depth,
                FolderThumbProvenance::Seeded,
            ) else {
                continue;
            };
            let key = format!(
                "{base}{}{}",
                crate::thumb_loader::CACHE_KEY_PIN_SUFFIX,
                resolved.source_id
            );
            let webp = video_db
                .lookup(&resolved.abs_path)
                .map(|pin| pin.thumb_webp);
            let Some(webp) =
                webp.filter(|bytes| crate::catalog::decode_thumb_dims(bytes).is_some())
            else {
                if map.contains_key(&key) {
                    deletes.insert(key);
                }
                continue;
            };
            if map.get(&key).is_some_and(|entry| {
                entry.mtime == resolved.mtime
                    && entry.file_size == resolved.file_size
                    && entry.jpeg_data == webp
            }) {
                continue;
            }
            seeds.push((
                key,
                CacheEntry {
                    mtime: resolved.mtime,
                    file_size: resolved.file_size,
                    jpeg_data: webp,
                    source_dims: None,
                    layout_dims: None,
                    folder_provenance: Some(FolderThumbProvenance::Seeded),
                    selection_proof: None,
                },
            ));
        }
        let deletes: Vec<_> = deletes.into_iter().collect();
        match catalog.commit_pin_materializations(&deletes, &seeds, cancel) {
            Ok(false) => return None,
            Ok(true) => {
                for key in deletes {
                    map.remove(&key);
                }
                map.extend(seeds);
            }
            Err(error) => {
                crate::logger::log(format!("metadata pin materialization failed: {error}"));
                // Keep the existing seed failure contract: changed video bytes must never
                // reload an old frame under the unchanged key. This is cleanup, not a seed
                // retry; each purge checks cancellation inside the catalog writer boundary.
                for (key, _) in seeds {
                    if !map.contains_key(&key) {
                        continue;
                    }
                    match catalog.commit_pin_materializations(
                        std::slice::from_ref(&key),
                        &[],
                        cancel,
                    ) {
                        Ok(false) => return None,
                        Ok(true) => {
                            map.remove(&key);
                        }
                        Err(error) => crate::logger::log(format!(
                            "metadata video seed purge failed: {error} ({key})"
                        )),
                    }
                }
            }
        }
        if cancel.load(Ordering::Relaxed) {
            return None;
        }
        Some(Prepared {
            reset_indices,
            replacement: Some(Arc::new(RwLock::new(map))),
            identity: self.identity,
        })
    }
}
