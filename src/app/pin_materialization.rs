//! Worker-only pin cache preparation. Terminal data contains memory resources only.
use super::{CurrentViewRefresh, GridItem};
use crate::catalog::{CacheEntry, CatalogDb, FolderThumbProvenance};
use crate::cover_identity::CoverIdentity;
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
        pins: &HashMap<String, crate::folder_thumb_pins::FolderPin>,
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
        // The private map is a sparse record of this context's writes, not an
        // inventory of persisted rows. Inspect DB names without loading pixels.
        let mut deletes = HashSet::new();
        for prefix in prefixes {
            if cancel.load(Ordering::Relaxed) {
                return None;
            }
            match catalog.list_filenames_with_prefix(&prefix) {
                Ok(keys) => deletes.extend(keys.into_iter().filter(|key| {
                    !crate::thumbnail_cache_key::inventory_retains_key(&retained, key)
                })),
                Err(error) => {
                    crate::logger::log(format!(
                        "metadata pin materialization inventory failed: {error} ({prefix})"
                    ));
                    catalog.record_cache_error(format!("pin materialization inventory: {error}"));
                }
            }
        }
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
            let key =
                resolved.pinned_cache_key(&base, crate::folder_thumb_pins::PinKeyMode::Exact, None);
            let webp = video_db
                .lookup(&resolved.abs_path)
                .map(|pin| pin.thumb_webp);
            let Some(webp) =
                webp.filter(|bytes| crate::catalog::decode_thumb_dims(bytes).is_some())
            else {
                if catalog.contains_filename(&key).unwrap_or_else(|error| {
                    crate::logger::log(format!(
                        "metadata video seed existence failed: {error} ({key})"
                    ));
                    catalog.record_cache_error(format!("video seed existence: {error}"));
                    false
                }) {
                    deletes.insert(key);
                }
                continue;
            };
            let cover =
                CoverIdentity::Page(crate::adjustment_db::normalize_path(&resolved.abs_path));
            let already_seeded = map
                .get(&key)
                .map(|entry| {
                    entry.mtime == resolved.mtime
                        && entry.file_size == resolved.file_size
                        && entry.jpeg_data == webp
                        && entry.cover == cover
                })
                .unwrap_or_else(|| {
                    catalog
                        .matches_video_seed(&key, resolved.mtime, resolved.file_size, &webp, &cover)
                        .unwrap_or_else(|error| {
                            crate::logger::log(format!(
                                "metadata video seed lookup failed: {error} ({key})"
                            ));
                            catalog.record_cache_error(format!("video seed lookup: {error}"));
                            false
                        })
                });
            if already_seeded {
                continue;
            }
            seeds.push((
                key,
                CacheEntry {
                    auto_material: crate::cover_auto_material::AutoMaterial::NotAutomatic,
                    mtime: resolved.mtime,
                    file_size: resolved.file_size,
                    jpeg_data: webp,
                    cover,
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
                    if !catalog.contains_filename(&key).unwrap_or_else(|error| {
                        crate::logger::log(format!(
                            "metadata video seed existence failed: {error} ({key})"
                        ));
                        catalog.record_cache_error(format!("video seed existence: {error}"));
                        false
                    }) {
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::folder_thumb_pins::{FolderPin, FolderPinSource};

    #[test]
    fn sparse_video_pin_seed_matches_purges_and_cleans_up_failed_replacement() {
        for scenario in ["matching", "changed", "missing", "corrupt", "failed"] {
            let tmp = tempfile::tempdir().unwrap();
            let folder = tmp.path().join("book");
            std::fs::create_dir(&folder).unwrap();
            let video = folder.join("movie.mp4");
            std::fs::write(&video, b"video fixture").unwrap();
            let pin = FolderPin::from(FolderPinSource::File {
                rel: "movie.mp4".into(),
                kind: crate::folder_thumb_pins::FileKind::Video,
            });
            let resolved = crate::folder_thumb_pins::resolve_pin_target(&folder, &pin).unwrap();
            let base = crate::thumb_loader::folder_thumb_cache_key_for_path(
                &folder,
                false,
                crate::settings::SortOrder::Numeric,
                3,
                FolderThumbProvenance::Seeded,
            )
            .unwrap();
            let key =
                resolved.pinned_cache_key(&base, crate::folder_thumb_pins::PinKeyMode::Exact, None);
            let pixels = |red| {
                let mut bytes = Vec::new();
                image::DynamicImage::ImageRgba8(image::RgbaImage::from_pixel(
                    2,
                    3,
                    image::Rgba([red, 0, 0, 255]),
                ))
                .write_to(
                    &mut std::io::Cursor::new(&mut bytes),
                    image::ImageFormat::WebP,
                )
                .unwrap();
                bytes
            };
            let old_bytes = pixels(0);
            let new_bytes = if scenario == "matching" {
                old_bytes.clone()
            } else {
                pixels(255)
            };
            let video_db =
                crate::video_pins::VideoPinDb::open_at(&tmp.path().join("video_pins.db")).unwrap();
            if scenario != "missing" {
                video_db
                    .set_pin(
                        &video,
                        1.0,
                        if scenario == "corrupt" {
                            b"broken"
                        } else {
                            &new_bytes
                        },
                    )
                    .unwrap();
            }
            let cache_dir = tmp.path().join("catalogs");
            let catalog = Arc::new(CatalogDb::open(&cache_dir, tmp.path()).unwrap());
            let cover = CoverIdentity::Page(crate::adjustment_db::normalize_path(&video));
            catalog
                .save(
                    &key,
                    resolved.mtime,
                    resolved.file_size,
                    2,
                    3,
                    None,
                    &old_bytes,
                    &cover,
                    crate::cover_auto_material::AutoMaterial::NotAutomatic,
                )
                .unwrap();
            if matches!(scenario, "matching" | "failed") {
                rusqlite::Connection::open(crate::catalog::db_path_for(&cache_dir, tmp.path()))
                    .unwrap()
                    .execute_batch("CREATE TRIGGER reject_seed BEFORE INSERT ON thumbnails BEGIN SELECT RAISE(ABORT, 'rejected seed'); END;")
                    .unwrap();
            }
            let live = Arc::new(RwLock::new(HashMap::new()));
            let request = Request {
                rows: vec![Row {
                    index: 0,
                    item: GridItem::Folder(folder.clone()),
                    metadata: None,
                    container: folder.clone(),
                    dependency_root: folder.clone(),
                }],
                identity: Identity {
                    cache: Some(Arc::as_ptr(&live) as usize),
                    catalog: Some(Arc::as_ptr(&catalog) as usize),
                    sort: crate::settings::SortOrder::Numeric,
                    depth: 3,
                    full_path: false,
                },
                cache: Some((live.clone(), catalog.clone())),
                scope: CurrentViewRefresh::Full,
            };
            let pins = HashMap::from([(crate::path_key::normalize_keep_drive(&folder), pin)]);
            let prepared = request
                .prepare(
                    vec![0],
                    &pins,
                    None,
                    Some(&video_db),
                    &AtomicBool::new(false),
                )
                .unwrap();
            let replacement = prepared.replacement.unwrap();
            let replacement = replacement.read().unwrap();
            match scenario {
                "matching" => {
                    // The DB hit must avoid a write and stay out of the sparse map.
                    assert_eq!(
                        catalog.load_one(&key).unwrap().unwrap().jpeg_data,
                        old_bytes
                    );
                    assert!(replacement.is_empty());
                }
                "changed" => {
                    assert_eq!(
                        catalog.load_one(&key).unwrap().unwrap().jpeg_data,
                        new_bytes
                    );
                    assert_eq!(replacement[&key].jpeg_data, new_bytes);
                }
                _ => {
                    assert!(!catalog.contains_filename(&key).unwrap(), "{scenario}");
                    assert!(replacement.is_empty(), "{scenario}");
                }
            }
            assert!(live.read().unwrap().is_empty());
        }
    }

    #[test]
    fn epub_pin_retention_prepares_private_map_and_preserves_sibling_and_cancelled_owner() {
        check_epub_pin_retention(false);
    }

    #[test]
    fn sparse_epub_pin_retention_deletes_db_only_variants_and_preserves_siblings() {
        check_epub_pin_retention(true);
    }

    fn check_epub_pin_retention(sparse: bool) {
        let tmp = tempfile::tempdir().unwrap();
        let epub = tmp.path().join("book.epub");
        let sibling = tmp.path().join("sibling.epub");
        let item = GridItem::PdfFile(epub.clone());
        let pins = HashMap::from([(
            crate::path_key::normalize_keep_drive(&epub),
            FolderPin::from(FolderPinSource::PdfPage {
                pdf_rel: String::new(),
                page: 2,
            }),
        )]);
        let inventory = super::super::folder_thumb_existing_keys_for(
            &item,
            Some((123, 456)),
            &pins,
            None,
            Some(crate::settings::SortOrder::Numeric),
            3,
            false,
        );
        let pending = inventory.iter().find(|key| key.contains("#pin:")).unwrap();
        assert!(pending.ends_with("|generation-pending|generation-pending"));
        let first = format!("{pending}|generation:17:4096");
        let second = format!("{pending}|generation:18:8192");
        let removed = first.replace("|-|2|", "|-|3|");
        let sibling_key = "pdfthumb:sibling.epub#pin:pdfpage||-|3|generation-pending|generation-pending|generation:19:12288";
        let catalog = Arc::new(CatalogDb::open(&tmp.path().join("catalogs"), tmp.path()).unwrap());
        for key in [
            first.as_str(),
            second.as_str(),
            removed.as_str(),
            sibling_key,
        ] {
            catalog
                .save(
                    key,
                    17,
                    4096,
                    8,
                    12,
                    None,
                    b"opaque pixels",
                    &CoverIdentity::Legacy,
                    crate::cover_auto_material::AutoMaterial::NotAutomatic,
                )
                .unwrap();
        }
        let mut map = catalog.load_all().unwrap();
        if sparse {
            map.retain(|key, _| key == sibling_key);
        }
        let live = Arc::new(RwLock::new(map));
        // Another context can still hold the old map; preparation must not mutate it.
        let sibling_context = live.clone();
        let identity = Identity {
            cache: Some(Arc::as_ptr(&live) as usize),
            catalog: Some(Arc::as_ptr(&catalog) as usize),
            sort: crate::settings::SortOrder::Numeric,
            depth: 3,
            full_path: false,
        };
        let make_request = || Request {
            rows: vec![
                Row {
                    index: 0,
                    item: item.clone(),
                    metadata: Some((123, 456)),
                    container: epub.clone(),
                    dependency_root: epub.clone(),
                },
                Row {
                    index: 1,
                    item: GridItem::PdfFile(sibling.clone()),
                    metadata: Some((123, 456)),
                    container: sibling.clone(),
                    dependency_root: sibling.clone(),
                },
            ],
            identity,
            cache: Some((live.clone(), catalog.clone())),
            scope: CurrentViewRefresh::Pins {
                folders: HashSet::from([epub.clone()]),
                videos: HashSet::new(),
            },
        };
        assert!(
            make_request()
                .prepare(vec![0, 1], &pins, None, None, &AtomicBool::new(true))
                .is_none()
        );
        assert!(catalog.load_one(&removed).unwrap().is_some());
        assert_eq!(live.read().unwrap().len(), if sparse { 1 } else { 4 });

        let prepared = make_request()
            .prepare(vec![0, 1], &pins, None, None, &AtomicBool::new(false))
            .unwrap();
        assert!(prepared.matches(identity));
        let other_identity = Identity {
            cache: Some(0),
            ..identity
        };
        assert!(!prepared.matches(other_identity));
        assert_eq!(prepared.reset_indices, vec![0]);
        let replacement = prepared.replacement.unwrap();
        assert!(!Arc::ptr_eq(&live, &replacement));
        let replacement = replacement.read().unwrap();
        assert_eq!(replacement.len(), if sparse { 1 } else { 3 });
        for key in [first.as_str(), second.as_str(), sibling_key] {
            assert_eq!(replacement.contains_key(key), !sparse || key == sibling_key);
            assert!(catalog.load_one(key).unwrap().is_some());
        }
        assert!(!replacement.contains_key(&removed));
        assert!(catalog.load_one(&removed).unwrap().is_none());
        let sibling_context = sibling_context.read().unwrap();
        assert_eq!(sibling_context.len(), if sparse { 1 } else { 4 });
        assert_eq!(sibling_context.contains_key(&removed), !sparse);
        assert!(sibling_context.contains_key(sibling_key));
    }
}
