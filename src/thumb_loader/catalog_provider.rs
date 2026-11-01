//! Worker-only lazy catalogs. The LRU contains idle references, never I/O under its lock.
use super::*;
use crate::catalog::{CatalogDb, CatalogWork};
use std::collections::VecDeque;
use std::path::PathBuf;

#[derive(Clone, Default)]
pub(crate) enum RequestCatalog {
    #[default]
    Fixed,
    CollectionSourceParent {
        identity: PathBuf,
        work: CatalogWork,
    },
}

impl RequestCatalog {
    pub(crate) fn collection(source: &Path, work: CatalogWork) -> Self {
        let identity = source
            .parent()
            .and_then(crate::catalog::collection_source_parent_identity);
        match identity {
            Some(identity) => Self::CollectionSourceParent { identity, work },
            None => {
                crate::logger::log(format!(
                    "collection thumbnail: invalid source parent: {}",
                    source.display()
                ));
                Self::Fixed
            }
        }
    }

    pub(crate) fn is_collection(&self) -> bool {
        matches!(self, Self::CollectionSourceParent { .. })
    }

    pub(crate) fn is_retired(&self) -> bool {
        match self {
            Self::Fixed => false,
            Self::CollectionSourceParent { work, .. } => {
                !work.access().is_admitted(work.identity().1)
            }
        }
    }
}

#[derive(Default)]
pub struct CollectionParentCatalogs {
    idle: Mutex<VecDeque<ParentCatalog>>,
}

struct ParentCatalog {
    identity: PathBuf,
    cache_dir: PathBuf,
    admission: crate::catalog::CatalogAdmission,
    writable: bool,
    db: Arc<std::sync::OnceLock<Option<Arc<CatalogDb>>>>,
}

/// The single target choice for a worker pool. Fixed callers retain their exact
/// catalog destination. A Collection pool only resolves request-owned parents.
#[derive(Clone)]
pub enum ThumbnailCatalogProvider {
    FixedCatalog(Option<Arc<CatalogDb>>),
    CollectionSourceParents(Arc<CollectionParentCatalogs>),
}

impl From<Option<&Arc<CatalogDb>>> for ThumbnailCatalogProvider {
    fn from(value: Option<&Arc<CatalogDb>>) -> Self {
        Self::FixedCatalog(value.cloned())
    }
}

impl ThumbnailCatalogProvider {
    pub(crate) fn collection() -> Self {
        Self::CollectionSourceParents(Arc::default())
    }

    pub(crate) fn resolve(
        &self,
        req: &LoadRequest,
        decision: CacheDecision,
        cancel: Option<&Arc<AtomicBool>>,
    ) -> Option<Arc<CatalogDb>> {
        let Self::CollectionSourceParents(pool) = self else {
            let Self::FixedCatalog(db) = self else {
                unreachable!()
            };
            return db.clone();
        };
        let RequestCatalog::CollectionSourceParent { identity, work } = &req.catalog_target else {
            return None;
        };
        // Video/frame-pin seeds are authoritative memory assets, not catalog rows.
        // Their CacheOnly request must neither read nor mirror a stale disk seed.
        if req.uses_memory_pin_seed() {
            return None;
        }
        let started = std::time::Instant::now();
        let access = work.access();
        let admitted = access.is_admitted(work.identity().1);
        let writable = req.source_policy.allows_source()
            && (req.force_cache || decision.policy != crate::settings::CachePolicy::Off);
        let open = |work: &CatalogWork, writable: bool| {
            let result = if writable {
                work.open(identity).map(Some)
            } else {
                work.open_existing(identity)
            };
            match result {
                Ok(db) => db.map(Arc::new),
                Err(error) => {
                    if access.is_admitted(work.identity().1) {
                        access.record_cache_error(format!("collection thumbnail catalog: {error}"));
                    }
                    crate::logger::log(format!(
                        "collection thumbnail catalog open failed: {}: {error}",
                        identity.display()
                    ));
                    None
                }
            }
        };
        let db = if !admitted {
            // Only CacheOnly may take the existing one-shot RO continuation.
            // The request's writer admission never changes, and this connection
            // is temporary, so it cannot escape into the writable LRU.
            if req.source_policy != LoadSourcePolicy::CacheOnly {
                return None;
            }
            let admission = access.wait_read_admission_with(&|| {
                cancel.is_some_and(|flag| flag.load(Ordering::Acquire))
            })?;
            open(
                &CatalogWork::with_admission(work.identity().0, admission),
                false,
            )
        } else {
            let (slot, retired) = {
                let mut idle = pool.idle.lock().unwrap();
                let pos = idle.iter().position(|entry| {
                    entry.identity == *identity
                        && entry.cache_dir == work.identity().0
                        && entry.admission == work.identity().1
                        && entry.writable == writable
                });
                if let Some(pos) = pos {
                    let entry = idle.remove(pos).unwrap();
                    let db = Arc::clone(&entry.db);
                    idle.push_back(entry);
                    (db, None)
                } else {
                    let slot = Arc::new(std::sync::OnceLock::new());
                    idle.push_back(ParentCatalog {
                        identity: identity.clone(),
                        cache_dir: work.identity().0.to_path_buf(),
                        admission: work.identity().1,
                        writable,
                        db: Arc::clone(&slot),
                    });
                    let retired = (idle.len() > 16).then(|| idle.pop_front()).flatten();
                    (slot, retired)
                }
            };
            drop(retired);
            // Same-pool opens share a slot; initialization/SQL never holds the
            // LRU lock. The native catalog gate handles other pools and deletion.
            let db = slot.get_or_init(|| open(work, writable)).clone();
            if db.is_none() {
                let removed = {
                    let mut idle = pool.idle.lock().unwrap();
                    idle.iter()
                        .position(|entry| Arc::ptr_eq(&entry.db, &slot))
                        .and_then(|pos| idle.remove(pos))
                };
                drop(removed);
            }
            db
        };
        if crate::perf::is_enabled() {
            crate::perf::event(
                "collection",
                "thumb_catalog_open",
                None,
                req.input_seq,
                &[
                    (
                        "ms",
                        serde_json::json!(started.elapsed().as_secs_f64() * 1000.0),
                    ),
                    ("writable", serde_json::json!(writable && admitted)),
                    ("opened", serde_json::json!(db.is_some())),
                ],
            );
        }
        db
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::catalog::{CatalogAccess, CatalogDeleteOperation, db_path_for};
    use crate::settings::CachePolicy;
    use std::sync::RwLock;

    fn request(source: &Path, cache: &Path) -> LoadRequest {
        LoadRequest {
            path: source.to_owned(),
            mtime: 7,
            file_size: 9,
            cache_key_override: Some(format!("imgthumb:{}", source.display())),
            catalog_target: RequestCatalog::collection(source, CatalogWork::capture(cache)),
            ..Default::default()
        }
    }

    fn decision(policy: CachePolicy) -> CacheDecision {
        let mut decision = CacheDecision::without_thumbnail();
        decision.policy = policy;
        decision
    }

    fn run(
        req: &mut LoadRequest,
        provider: &ThumbnailCatalogProvider,
        policy: CachePolicy,
        map: &RwLock<std::collections::HashMap<String, crate::catalog::CacheEntry>>,
    ) -> Vec<ThumbMsg> {
        let (tx, rx) = mpsc::channel();
        let done = Arc::new(AtomicUsize::new(0));
        process_load_request(
            req,
            map,
            &tx,
            provider.clone(),
            64,
            75,
            32,
            decision(policy),
            &done,
            &Arc::new(Mutex::new(crate::stats::ThumbStats::default())),
            None,
            &Arc::new(AtomicUsize::new(0)),
            &Arc::new(AtomicUsize::new(1)),
            None,
            None,
            None,
            None,
            None,
        );
        assert_eq!(done.load(Ordering::Relaxed), 1);
        rx.try_iter().collect()
    }

    fn image_source(temp: &Path) -> PathBuf {
        let source = temp.join("source").join("image.png");
        std::fs::create_dir_all(source.parent().unwrap()).unwrap();
        image::RgbImage::from_pixel(16, 12, image::Rgb([220, 30, 10]))
            .save(&source)
            .unwrap();
        source
    }

    #[test]
    fn collection_parent_cache_only_empty_corrupt_and_seed_admission() {
        let temp = tempfile::tempdir().unwrap();
        let source = image_source(temp.path());
        let cache = temp.path().join("cache");
        let identity =
            crate::catalog::collection_source_parent_identity(source.parent().unwrap()).unwrap();
        let path = db_path_for(&cache, &identity);
        let provider = ThumbnailCatalogProvider::collection();
        let mut req = request(&source, &cache);
        req.source_policy = LoadSourcePolicy::CacheOnly;
        let messages = run(&mut req, &provider, CachePolicy::Always, &RwLock::default());
        assert!(matches!(
            messages[0].payload,
            ThumbMsgPayload::CacheOnlyMiss
        ));
        assert!(!path.exists());
        req.source_policy = LoadSourcePolicy::CacheOrSource;
        req.force_cache = true;
        run(&mut req, &provider, CachePolicy::Off, &RwLock::default());
        assert!(path.exists(), "existing force-cache exception");
        let db = provider
            .resolve(&req, decision(CachePolicy::Always), None)
            .unwrap();
        db.save(
            req.cache_key_override.as_deref().unwrap(),
            7,
            9,
            8,
            8,
            None,
            b"broken WebP",
        )
        .unwrap();
        req.force_cache = false;
        let messages = run(&mut req, &provider, CachePolicy::Always, &RwLock::default());
        assert!(matches!(
            messages[0].pixels().unwrap().origin,
            ThumbLoadOrigin::SourceGenerated { .. }
        ));
        let row = db
            .load_one(req.cache_key_override.as_deref().unwrap())
            .unwrap()
            .unwrap();
        assert!(crate::catalog::decode_thumb_to_color_image(&row.jpeg_data).is_some());
        req.folder_thumb_provenance = Some(crate::catalog::FolderThumbProvenance::Seeded);
        assert!(
            provider
                .resolve(&req, decision(CachePolicy::Always), None)
                .is_none(),
            "video seed must not consult disk"
        );
    }

    #[test]
    fn collection_parent_catalog_fault_keeps_source_and_reports_without_repair() {
        for blocked_directory in [false, true] {
            let temp = tempfile::tempdir().unwrap();
            let source = image_source(temp.path());
            let cache = temp.path().join("cache");
            let identity =
                crate::catalog::collection_source_parent_identity(source.parent().unwrap())
                    .unwrap();
            let damaged = if blocked_directory {
                cache.clone()
            } else {
                db_path_for(&cache, &identity)
            };
            std::fs::create_dir_all(damaged.parent().unwrap()).unwrap();
            std::fs::write(&damaged, b"keep damaged data").unwrap();
            let provider = ThumbnailCatalogProvider::collection();
            let mut req = request(&source, &cache);
            let messages = run(&mut req, &provider, CachePolicy::Always, &RwLock::default());
            assert!(messages[0].pixels().is_some());
            let access = CatalogAccess::for_cache_dir(&cache);
            assert!(access.take_cache_notice().is_some());
            req.source_policy = LoadSourcePolicy::CacheOnly;
            let messages = run(&mut req, &provider, CachePolicy::Always, &RwLock::default());
            assert!(matches!(
                messages[0].payload,
                ThumbMsgPayload::CacheOnlyMiss
            ));
            assert!(access.take_cache_notice().is_none());
            assert_eq!(std::fs::read(&damaged).unwrap(), b"keep damaged data");
        }
    }

    #[test]
    fn collection_parent_simultaneous_contexts_keep_independent_writers() {
        let temp = tempfile::tempdir().unwrap();
        let source = image_source(temp.path());
        let cache = temp.path().join("cache");
        let req = request(&source, &cache);
        let first = ThumbnailCatalogProvider::collection();
        let second = ThumbnailCatalogProvider::collection();
        let barrier = std::sync::Barrier::new(2);
        std::thread::scope(|scope| {
            for provider in [&first, &second] {
                let req = req.clone();
                let barrier = &barrier;
                scope.spawn(move || {
                    barrier.wait();
                    let messages = run(
                        &mut req.clone(),
                        provider,
                        CachePolicy::Always,
                        &RwLock::default(),
                    );
                    assert!(messages[0].pixels().is_some());
                });
            }
        });
        let second_db = second
            .resolve(&req, decision(CachePolicy::Always), None)
            .unwrap();
        drop(first);
        assert!(!second_db.is_retired());
        second_db
            .save("after sibling close", 7, 9, 1, 1, None, b"still live")
            .unwrap();
    }

    #[test]
    fn collection_parent_drive_unc_identity_and_legacy_counterexample() {
        let temp = tempfile::tempdir().unwrap();
        let c = Path::new(r"C:\photos\same");
        let d = Path::new(r"D:\photos\same");
        assert_eq!(db_path_for(temp.path(), c), db_path_for(temp.path(), d));
        let identities: Vec<_> = [
            c,
            d,
            Path::new(r"C:\"),
            Path::new(r"D:\"),
            Path::new(r"\\server\one"),
            Path::new(r"\\server\two"),
        ]
        .map(|p| crate::catalog::collection_source_parent_identity(p).unwrap())
        .into();
        let paths: std::collections::HashSet<_> = identities
            .iter()
            .map(|p| db_path_for(temp.path(), p))
            .collect();
        assert_eq!(paths.len(), identities.len());
        assert_eq!(
            Some(identities[0].clone()),
            crate::catalog::collection_source_parent_identity(Path::new(
                r"c:/photos/./child/../same/"
            ))
        );
        let legacy = CatalogDb::open(temp.path(), c).unwrap();
        legacy
            .save("same.jpg", 7, 9, 1, 1, None, b"C contents")
            .unwrap();
        assert_eq!(
            CatalogDb::open(temp.path(), d)
                .unwrap()
                .load_one("same.jpg")
                .unwrap()
                .unwrap()
                .jpeg_data,
            b"C contents"
        );
        let provider = ThumbnailCatalogProvider::collection();
        for (source, color) in [
            (c.join("same.jpg"), [240, 0, 0]),
            (d.join("same.jpg"), [0, 0, 240]),
        ] {
            let req = request(&source, temp.path());
            let db = provider
                .resolve(&req, decision(CachePolicy::Always), None)
                .unwrap();
            let image = image::DynamicImage::ImageRgb8(image::RgbImage::from_pixel(
                8,
                8,
                image::Rgb(color),
            ));
            let (bytes, w, h) = crate::catalog::encode_thumb_webp(&image, 8, 75.0).unwrap();
            db.save(
                req.cache_key_override.as_deref().unwrap(),
                7,
                9,
                w,
                h,
                Some((8, 8)),
                &bytes,
            )
            .unwrap();
        }
        for (source, red) in [(c.join("same.jpg"), true), (d.join("same.jpg"), false)] {
            let mut req = request(&source, temp.path());
            req.source_policy = LoadSourcePolicy::CacheOnly;
            let messages = run(
                &mut req,
                &ThumbnailCatalogProvider::collection(),
                CachePolicy::Always,
                &RwLock::default(),
            );
            let pixels = messages[0].pixels().unwrap();
            let pixel = pixels.image.pixels[0];
            assert_eq!(pixel.r() > pixel.b(), red);
            assert_eq!(pixels.origin, ThumbLoadOrigin::UpgradeableCache);
        }
        // Removing either provider is a local close, never epoch retirement.
        drop(provider);
        assert!(!legacy.is_retired());
    }

    #[test]
    fn collection_parent_reopen_scroll_stamp_policy_and_no_legacy_seed() {
        let temp = tempfile::tempdir().unwrap();
        let source = image_source(temp.path());
        let cache = temp.path().join("cache");
        let legacy = CatalogDb::open(&cache, source.parent().unwrap()).unwrap();
        legacy
            .save("image.png", 7, 9, 1, 1, None, b"ambiguous old row")
            .unwrap();
        let identity =
            crate::catalog::collection_source_parent_identity(source.parent().unwrap()).unwrap();
        let path = db_path_for(&cache, &identity);
        let mut req = request(&source, &cache);
        for policy in [CachePolicy::Off, CachePolicy::Auto] {
            let map = RwLock::default();
            let messages = run(
                &mut req,
                &ThumbnailCatalogProvider::collection(),
                policy,
                &map,
            );
            assert!(matches!(
                messages[0].pixels().unwrap().origin,
                ThumbLoadOrigin::SourceGenerated { .. }
            ));
            // Auto with threshold zero saves; Off never creates a DB.
            assert_eq!(path.exists(), policy == CachePolicy::Auto);
        }
        let provider = ThumbnailCatalogProvider::collection();
        let map = RwLock::default();
        let first = run(&mut req, &provider, CachePolicy::Always, &map);
        assert_eq!(
            first[0].pixels().unwrap().origin,
            ThumbLoadOrigin::UpgradeableCache
        );
        let back = run(&mut req, &provider, CachePolicy::Always, &map);
        assert_eq!(
            back[0].pixels().unwrap().origin,
            ThumbLoadOrigin::UpgradeableCache
        );
        let reopen = run(
            &mut req,
            &ThumbnailCatalogProvider::collection(),
            CachePolicy::Always,
            &RwLock::default(),
        );
        assert_eq!(
            reopen[0].pixels().unwrap().origin,
            ThumbLoadOrigin::UpgradeableCache
        );
        req.mtime += 1;
        let changed = run(&mut req, &provider, CachePolicy::Always, &RwLock::default());
        assert!(matches!(
            changed[0].pixels().unwrap().origin,
            ThumbLoadOrigin::SourceGenerated { .. }
        ));
        let upgraded = {
            req.source_policy = LoadSourcePolicy::SourceOnly;
            run(&mut req, &provider, CachePolicy::Always, &RwLock::default())
        };
        assert!(matches!(
            upgraded[0].pixels().unwrap().origin,
            ThumbLoadOrigin::SourceGenerated { .. }
        ));
        req.source_policy = LoadSourcePolicy::CacheOnly;
        let db = provider
            .resolve(&req, decision(CachePolicy::Always), None)
            .unwrap();
        assert!(
            db.is_read_only(),
            "CacheOnly must not borrow a warm RW handle"
        );
        req.source_policy = LoadSourcePolicy::CacheOrSource;
        let db = provider
            .resolve(&req, decision(CachePolicy::Off), None)
            .unwrap();
        assert!(db.is_read_only());
        assert!(!legacy.is_retired());
        assert_eq!(
            legacy.load_one("image.png").unwrap().unwrap().jpeg_data,
            b"ambiguous old row"
        );
    }

    #[test]
    fn collection_parent_clear_family_and_stale_requests_do_not_recreate() {
        let temp = tempfile::tempdir().unwrap();
        let cache = temp.path();
        let source = Path::new(r"C:\photos\image.png");
        let other = Path::new(r"D:\photos\image.png");
        let mut old = request(source, cache);
        let provider = ThumbnailCatalogProvider::collection();
        let sibling = ThumbnailCatalogProvider::collection();
        let db = provider
            .resolve(&old, decision(CachePolicy::Always), None)
            .unwrap();
        let other_db = sibling
            .resolve(&request(other, cache), decision(CachePolicy::Always), None)
            .unwrap();
        let legacy = CatalogDb::open(cache, source.parent().unwrap()).unwrap();
        legacy.save("old", 1, 1, 1, 1, None, b"old").unwrap();
        let c_path = db_path_for(
            cache,
            &crate::catalog::collection_source_parent_identity(source.parent().unwrap()).unwrap(),
        );
        let d_path = db_path_for(
            cache,
            &crate::catalog::collection_source_parent_identity(other.parent().unwrap()).unwrap(),
        );
        let deletion = CatalogAccess::for_cache_dir(cache)
            .begin_delete(CatalogDeleteOperation::Folder(
                source.parent().unwrap().to_owned(),
            ))
            .unwrap();
        deletion.retire_connections();
        assert!(
            crate::catalog::delete_folder_cache_under_delete(cache, source.parent().unwrap())
                .unwrap()
        );
        drop(deletion);
        assert!(!c_path.exists());
        assert!(d_path.exists());
        assert!(db.is_retired() && other_db.is_retired());
        assert!(
            provider
                .resolve(&old, decision(CachePolicy::Always), None)
                .is_none()
        );
        old.source_policy = LoadSourcePolicy::CacheOnly;
        assert!(
            provider
                .resolve(&old, decision(CachePolicy::Always), None)
                .is_none()
        );
        assert!(!c_path.exists());
        let fresh = request(source, cache);
        let fresh_db = provider
            .resolve(&fresh, decision(CachePolicy::Always), None)
            .unwrap();
        assert!(!fresh_db.is_retired());
        assert_eq!(crate::catalog::delete_old_cache(cache, 0), 2);
        assert!(!c_path.exists() && !d_path.exists());
        assert!(
            provider
                .resolve(&fresh, decision(CachePolicy::Always), None)
                .is_none()
        );
        let fresh = request(source, cache);
        assert!(
            provider
                .resolve(&fresh, decision(CachePolicy::Always), None)
                .is_some()
        );
        assert_eq!(crate::catalog::delete_all_cache(cache), 1);
        assert!(!c_path.exists());
    }

    #[test]
    fn collection_parent_cache_only_after_clear_reads_successor_not_old_memory() {
        let temp = tempfile::tempdir().unwrap();
        let source = Path::new(r"C:\photos\image.png");
        let mut req = request(source, temp.path());
        let provider = ThumbnailCatalogProvider::collection();
        let db = provider
            .resolve(&req, decision(CachePolicy::Always), None)
            .unwrap();
        let image = image::DynamicImage::new_rgb8(8, 8);
        let (bytes, w, h) = crate::catalog::encode_thumb_webp(&image, 8, 75.0).unwrap();
        let key = req.cache_key_override.clone().unwrap();
        db.save(&key, 7, 9, w, h, Some((8, 8)), &bytes).unwrap();
        let map = RwLock::new(std::collections::HashMap::from([(
            key.clone(),
            db.load_one(&key).unwrap().unwrap(),
        )]));
        crate::catalog::delete_all_cache(temp.path());
        req.source_policy = LoadSourcePolicy::CacheOnly;
        let messages = run(&mut req, &provider, CachePolicy::Always, &map);
        assert!(matches!(
            messages[0].payload,
            ThumbMsgPayload::CacheOnlyMiss
        ));
        let fresh = request(source, temp.path());
        let successor = provider
            .resolve(&fresh, decision(CachePolicy::Always), None)
            .unwrap();
        let image = image::DynamicImage::ImageRgb8(image::RgbImage::from_pixel(
            8,
            8,
            image::Rgb([240, 0, 0]),
        ));
        let (bytes, w, h) = crate::catalog::encode_thumb_webp(&image, 8, 75.0).unwrap();
        successor
            .save(&key, 7, 9, w, h, Some((8, 8)), &bytes)
            .unwrap();
        let messages = run(&mut req, &provider, CachePolicy::Always, &map);
        let pixel = messages[0].pixels().unwrap().image.pixels[0];
        assert!(pixel.r() > 200 && pixel.b() < 20);
        assert!(!successor.is_retired());
        // The old writer admission has not been upgraded by the RO continuation.
        req.source_policy = LoadSourcePolicy::CacheOrSource;
        assert!(
            provider
                .resolve(&req, decision(CachePolicy::Always), None)
                .is_none()
        );
    }

    #[test]
    fn collection_parent_lru_bounded_context_close_and_logical_owner() {
        let temp = tempfile::tempdir().unwrap();
        let provider = ThumbnailCatalogProvider::collection();
        let sibling = ThumbnailCatalogProvider::collection();
        let source = Path::new(r"C:\logical\book.zip");
        let mut req = request(source, temp.path());
        // A pin/conversion decode path never determines the owning parent.
        req.path = PathBuf::from(r"D:\converted\page.png");
        let db = provider
            .resolve(&req, decision(CachePolicy::Always), None)
            .unwrap();
        let sibling_db = sibling
            .resolve(&req, decision(CachePolicy::Always), None)
            .unwrap();
        for idx in 0..40 {
            let source = PathBuf::from(format!(r"C:\parent{idx}\same.png"));
            provider
                .resolve(
                    &request(&source, temp.path()),
                    decision(CachePolicy::Always),
                    None,
                )
                .unwrap();
        }
        let ThumbnailCatalogProvider::CollectionSourceParents(pool) = &provider else {
            unreachable!()
        };
        assert_eq!(pool.idle.lock().unwrap().len(), 16);
        drop(provider);
        assert!(!sibling_db.is_retired());
        sibling_db
            .save("sibling", 1, 1, 1, 1, None, b"still writable")
            .unwrap();
        assert!(!db.is_retired());
        assert!(
            db_path_for(
                temp.path(),
                &crate::catalog::collection_source_parent_identity(Path::new(r"C:\logical"))
                    .unwrap()
            )
            .exists()
        );
        assert!(
            !db_path_for(
                temp.path(),
                &crate::catalog::collection_source_parent_identity(Path::new(r"D:\converted"))
                    .unwrap()
            )
            .exists()
        );
    }
}
