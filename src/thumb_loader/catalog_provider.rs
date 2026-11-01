//! Worker-only lazy catalogs. The LRU contains idle references, never I/O under its lock.
use super::*;
use crate::catalog::{CatalogDb, CatalogWork};
use std::collections::VecDeque;
use std::path::PathBuf;

/// Request-owned fallback scope/admission. The pool's provider selects the
/// actual destination; a FixedCatalog always takes precedence over this scope.
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

/// A destination and its write admission, independent of the lookup connection.
/// Only `open_if` may turn a Collection destination into a writable catalog.
#[derive(Clone, Copy)]
pub struct ThumbnailSaveTarget<'a>(SaveTarget<'a>);

#[derive(Clone, Copy)]
enum SaveTarget<'a> {
    Unavailable,
    Fixed(&'a CatalogDb),
    CollectionSourceParent {
        pool: &'a CollectionParentCatalogs,
        identity: &'a Path,
        work: &'a CatalogWork,
    },
}

pub(super) enum SaveCatalog<'a> {
    Fixed(&'a CatalogDb),
    Parent(Arc<CatalogDb>),
}

impl AsRef<CatalogDb> for SaveCatalog<'_> {
    fn as_ref(&self) -> &CatalogDb {
        match self {
            Self::Fixed(db) => db,
            Self::Parent(db) => db,
        }
    }
}

impl<'a> From<Option<&'a CatalogDb>> for ThumbnailSaveTarget<'a> {
    fn from(db: Option<&'a CatalogDb>) -> Self {
        Self(db.map_or(SaveTarget::Unavailable, SaveTarget::Fixed))
    }
}

impl<'a> ThumbnailSaveTarget<'a> {
    pub(super) fn fixed_catalog(self) -> Option<&'a CatalogDb> {
        match self.0 {
            SaveTarget::Fixed(db) => Some(db),
            _ => None,
        }
    }

    pub(super) fn can_harvest(self, force: bool, decision: CacheDecision) -> bool {
        match self.0 {
            SaveTarget::Unavailable => false,
            SaveTarget::Fixed(_) => true,
            SaveTarget::CollectionSourceParent { work, .. } => {
                (force || decision.policy != crate::settings::CachePolicy::Off)
                    && work.access().is_admitted(work.identity().1)
            }
        }
    }

    pub(super) fn open_if(self, force: bool, policy_should_save: bool) -> Option<SaveCatalog<'a>> {
        if !(force || policy_should_save) {
            return None;
        }
        match self.0 {
            SaveTarget::Unavailable => None,
            SaveTarget::Fixed(db) => Some(SaveCatalog::Fixed(db)),
            SaveTarget::CollectionSourceParent {
                pool,
                identity,
                work,
            } => {
                // Keep the captured epoch even if decode/encode crosses a clear.
                // CatalogWork's open and every later SQL operation also take leases.
                if !work.access().is_admitted(work.identity().1) {
                    return None;
                }
                pool.open(identity, work, true).map(SaveCatalog::Parent)
            }
        }
    }
}

impl From<Option<&Arc<CatalogDb>>> for ThumbnailCatalogProvider {
    fn from(value: Option<&Arc<CatalogDb>>) -> Self {
        Self::FixedCatalog(value.cloned())
    }
}

impl ThumbnailCatalogProvider {
    pub(super) fn uses_collection_source_parents(&self, req: &LoadRequest) -> bool {
        matches!(
            (self, &req.catalog_target),
            (
                Self::CollectionSourceParents(_),
                RequestCatalog::CollectionSourceParent { .. }
            )
        )
    }

    pub(crate) fn collection() -> Self {
        Self::CollectionSourceParents(Arc::default())
    }

    pub(super) fn save_target<'a>(&'a self, req: &'a LoadRequest) -> ThumbnailSaveTarget<'a> {
        let target = match self {
            Self::FixedCatalog(db) => {
                return db.as_deref().filter(|db| !db.is_read_only()).into();
            }
            Self::CollectionSourceParents(pool) => match &req.catalog_target {
                RequestCatalog::CollectionSourceParent { identity, work }
                    if req.source_policy.allows_source() && !req.uses_memory_pin_seed() =>
                {
                    SaveTarget::CollectionSourceParent {
                        pool,
                        identity,
                        work,
                    }
                }
                _ => SaveTarget::Unavailable,
            },
        };
        ThumbnailSaveTarget(target)
    }

    pub(crate) fn resolve(
        &self,
        req: &LoadRequest,
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
        // SourceOnly and authoritative memory pins never need a disk lookup.
        if !req.source_policy.allows_cache() || req.uses_memory_pin_seed() {
            return None;
        }
        let started = std::time::Instant::now();
        let access = work.access();
        let db = if !access.is_admitted(work.identity().1) {
            // Only CacheOnly may take the existing one-shot RO continuation.
            // Its original writer admission never changes; do not cache this handle.
            if req.source_policy != LoadSourcePolicy::CacheOnly {
                return None;
            }
            let admission = access.wait_read_admission_with(&|| {
                cancel.is_some_and(|flag| flag.load(Ordering::Acquire))
            })?;
            CollectionParentCatalogs::open_catalog(
                identity,
                &CatalogWork::with_admission(work.identity().0, admission),
                false,
            )
        } else {
            pool.open(identity, work, false)
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
                    ("writable", serde_json::json!(false)),
                    ("opened", serde_json::json!(db.is_some())),
                ],
            );
        }
        db
    }

    pub(super) fn resume_cache_only_lookup(
        &self,
        req: &LoadRequest,
        previous: &CatalogDb,
        key: &str,
        cancel: Option<&Arc<AtomicBool>>,
    ) -> rusqlite::Result<Option<crate::catalog::CacheEntry>> {
        let canceled = || cancel.is_some_and(|flag| flag.load(Ordering::Relaxed));
        match self {
            Self::FixedCatalog(_) => previous.resume_cache_only_after_maintenance(key, &canceled),
            Self::CollectionSourceParents(_) => {
                // A clear can retire the RO after the initial resolve. Its
                // successor must pass the same Collection schema publication
                // boundary; the legacy-compatible generic resume is not valid.
                if canceled() {
                    return Ok(None);
                }
                let Some(db) = self.resolve(req, cancel) else {
                    return Ok(None);
                };
                let entry = db.load_one(key)?;
                if canceled() { Ok(None) } else { Ok(entry) }
            }
        }
    }
}

impl CollectionParentCatalogs {
    fn open_catalog(identity: &Path, work: &CatalogWork, writable: bool) -> Option<Arc<CatalogDb>> {
        let result = if writable {
            work.open(identity).map(Some)
        } else {
            // No mkdir, schema initialization or empty DB creation on lookup.
            work.open_existing_collection_parent(identity)
        };
        match result {
            Ok(db) => db.map(Arc::new),
            Err(error) => {
                let access = work.access();
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
    }

    fn open(&self, identity: &Path, work: &CatalogWork, writable: bool) -> Option<Arc<CatalogDb>> {
        let (slot, retired) = {
            let mut idle = self.idle.lock().unwrap();
            let pos = idle.iter().position(|entry| {
                entry.identity == identity
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
                    identity: identity.to_owned(),
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
        // Same-pool opens share a slot; initialization/SQL never holds the LRU lock.
        // The native catalog gate handles other pools and deletion.
        let db = slot
            .get_or_init(|| Self::open_catalog(identity, work, writable))
            .clone();
        if db.is_none() {
            // Do not memoize absence: another request/context may save this parent.
            let removed = {
                let mut idle = self.idle.lock().unwrap();
                idle.iter()
                    .position(|entry| Arc::ptr_eq(&entry.db, &slot))
                    .and_then(|pos| idle.remove(pos))
            };
            drop(removed);
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

    impl ThumbnailCatalogProvider {
        // Fixture setup explicitly commits to a save; lookup must never create DBs.
        fn writer(&self, req: &LoadRequest) -> Option<Arc<CatalogDb>> {
            match self.save_target(req).open_if(true, false)? {
                SaveCatalog::Parent(db) => Some(db),
                SaveCatalog::Fixed(_) => unreachable!(),
            }
        }
    }

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
        run_decided(req, provider, decision(policy), map)
    }

    fn run_decided(
        req: &mut LoadRequest,
        provider: &ThumbnailCatalogProvider,
        decision: CacheDecision,
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
            decision,
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
    fn collection_auto_unsaved_parent_fanout_never_creates_catalog_files() {
        let temp = tempfile::tempdir().unwrap();
        let cache = temp.path().join("cache");
        let provider = ThumbnailCatalogProvider::collection();
        let mut cheap_auto = decision(CachePolicy::Auto);
        cheap_auto.threshold_ms = u32::MAX;
        cheap_auto.size_threshold = u64::MAX;
        // Exceed the LRU and revisit with both retained and reopened providers.
        let sources: Vec<_> = (0..24)
            .map(|idx| image_source(&temp.path().join(format!("parent{idx}"))))
            .collect();
        for pool in [
            &provider,
            &provider,
            &ThumbnailCatalogProvider::collection(),
        ] {
            for source in &sources {
                let mut req = request(source, &cache);
                let messages = run_decided(&mut req, pool, cheap_auto, &RwLock::default());
                assert!(matches!(
                    messages[0].payload,
                    ThumbMsgPayload::SourcePixels(_)
                ));
            }
            assert!(
                !cache.exists(),
                "unsaved Auto must not even mkdir the cache"
            );
            assert_eq!(crate::catalog::cache_stats(&cache).0, 0);
        }
        let mut missing = request(&temp.path().join("missing/image.png"), &cache);
        run(
            &mut missing,
            &provider,
            CachePolicy::Always,
            &RwLock::default(),
        );
        assert!(
            !cache.exists(),
            "failed source decode must not create a catalog"
        );
    }

    #[test]
    fn collection_ro_does_not_publish_incomplete_initial_schema() {
        let temp = tempfile::tempdir().unwrap();
        let source = image_source(temp.path());
        let cache = temp.path().join("cache");
        let provider = ThumbnailCatalogProvider::collection();
        let req = request(&source, &cache);
        let identity =
            crate::catalog::collection_source_parent_identity(source.parent().unwrap()).unwrap();
        let path = db_path_for(&cache, &identity);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        // Pause the real creation protocol immediately after Connection::open.
        // A sibling worker/pool can observe this valid, still empty SQLite file.
        let creating = rusqlite::Connection::open(&path).unwrap();
        assert!(provider.resolve(&req, None).is_none());
        let ThumbnailCatalogProvider::CollectionSourceParents(pool) = &provider else {
            unreachable!()
        };
        assert!(pool.idle.lock().unwrap().is_empty());
        assert!(
            CatalogAccess::for_cache_dir(&cache)
                .take_cache_notice()
                .is_none()
        );
        drop(creating);
        let writer = CatalogWork::capture(&cache).open(&identity).unwrap();
        let proof = crate::catalog::FolderSelectionProof {
            directories: Vec::new(),
            pin_store: crate::catalog::PinStoreProof::NotConsulted,
            winner: crate::catalog::FolderSelectionWinner {
                path: source.clone(),
                mtime: 7,
                file_size: 9,
                archive_row_key: None,
            },
        };
        writer
            .save_with_folder_proof(
                req.cache_key_override.as_deref().unwrap(),
                7,
                9,
                8,
                8,
                Some((16, 12)),
                Some((16000, 12000)),
                b"bytes",
                Some(crate::catalog::FolderThumbProvenance::AutoSelected),
                Some(&proof),
            )
            .unwrap();
        let reader = provider.resolve(&req, None).unwrap();
        assert!(reader.is_read_only());
        let row = reader
            .load_one(req.cache_key_override.as_deref().unwrap())
            .unwrap()
            .unwrap();
        assert_eq!(row.source_dims, Some((16, 12)));
        assert_eq!(row.layout_dims, Some((16000, 12000)));
        assert_eq!(row.selection_proof, Some(proof));
        // The clear may happen after resolve but before the handler reads its
        // row. The provider's continuation must apply the same schema boundary.
        assert_eq!(crate::catalog::delete_all_cache(&cache), 1);
        assert!(reader.is_retired());
        let creating = rusqlite::Connection::open(&path).unwrap();
        let mut cache_only = req.clone();
        cache_only.source_policy = LoadSourcePolicy::CacheOnly;
        assert!(
            provider
                .resume_cache_only_lookup(
                    &cache_only,
                    &reader,
                    req.cache_key_override.as_deref().unwrap(),
                    None,
                )
                .unwrap()
                .is_none()
        );
        drop(creating);
        let writer = CatalogWork::capture(&cache).open(&identity).unwrap();
        writer
            .save_with_folder_proof(
                req.cache_key_override.as_deref().unwrap(),
                7,
                9,
                8,
                8,
                Some((16, 12)),
                Some((16000, 12000)),
                b"bytes",
                row.folder_provenance,
                row.selection_proof.as_ref(),
            )
            .unwrap();
        let continued = provider
            .resume_cache_only_lookup(
                &cache_only,
                &reader,
                req.cache_key_override.as_deref().unwrap(),
                None,
            )
            .unwrap()
            .unwrap();
        assert_eq!(continued.layout_dims, row.layout_dims);
        assert_eq!(continued.selection_proof, row.selection_proof);
    }

    #[test]
    fn collection_lookup_ro_save_target_epoch_cache_dir_and_source_only() {
        let temp = tempfile::tempdir().unwrap();
        let source = image_source(temp.path());
        let cache = temp.path().join("one");
        let other_cache = temp.path().join("two");
        let provider = ThumbnailCatalogProvider::collection();
        let mut req = request(&source, &cache);
        let target = provider.save_target(&req);
        assert!(provider.resolve(&req, None).is_none());
        assert!(!cache.exists());
        assert!(target.open_if(false, false).is_none());
        assert!(!target.can_harvest(false, decision(CachePolicy::Off)));
        assert!(target.can_harvest(true, decision(CachePolicy::Off)));
        assert!(target.can_harvest(false, decision(CachePolicy::Auto)));
        let writer = provider.writer(&req).unwrap();
        assert!(!writer.is_read_only());
        assert!(provider.resolve(&req, None).unwrap().is_read_only());
        let other = request(&source, &other_cache);
        let other_writer = provider.writer(&other).unwrap();
        assert!(!Arc::ptr_eq(&writer, &other_writer));
        // A target captured before clear never adopts the successor's epoch.
        assert_eq!(crate::catalog::delete_all_cache(&cache), 1);
        assert!(target.open_if(true, true).is_none());
        assert!(provider.resolve(&req, None).is_none());
        assert_eq!(crate::catalog::cache_stats(&cache).0, 0);
        assert!(!other_writer.is_retired());
        req = request(&source, &cache);
        req.source_policy = LoadSourcePolicy::SourceOnly;
        assert!(provider.resolve(&req, None).is_none());
        let messages = run(&mut req, &provider, CachePolicy::Always, &RwLock::default());
        assert!(matches!(
            messages[0].payload,
            ThumbMsgPayload::SourcePixels(_)
        ));
        req.source_policy = LoadSourcePolicy::CacheOnly;
        assert!(provider.save_target(&req).open_if(true, true).is_none());
        let messages = run(&mut req, &provider, CachePolicy::Always, &RwLock::default());
        assert_eq!(
            messages[0].pixels().unwrap().origin,
            ThumbLoadOrigin::UpgradeableCache
        );
    }

    #[test]
    fn fixed_physical_and_smart_catalogs_keep_save_policy_and_existing_destination() {
        let temp = tempfile::tempdir().unwrap();
        let source = image_source(temp.path());
        // Existing physical/aggregate callers both supply an already-open catalog.
        for folder in [source.parent().unwrap(), Path::new("__smart__/test")] {
            let db = Arc::new(CatalogDb::open(&temp.path().join("cache"), folder).unwrap());
            let provider = ThumbnailCatalogProvider::FixedCatalog(Some(db.clone()));
            for (policy, force, saves) in [
                (CachePolicy::Off, false, false),
                (CachePolicy::Auto, false, false),
                (CachePolicy::Always, false, true),
                (CachePolicy::Off, true, true),
                (CachePolicy::Auto, false, true),
            ] {
                let mut req = request(&source, &temp.path().join("unused"));
                req.catalog_target = RequestCatalog::Fixed;
                req.source_policy = LoadSourcePolicy::SourceOnly;
                req.force_cache = force;
                req.cache_key_override = Some(format!("{policy:?}-{force}-{saves}"));
                let mut measured = decision(policy);
                if !saves {
                    measured.threshold_ms = u32::MAX;
                    measured.size_threshold = u64::MAX;
                }
                assert!(Arc::ptr_eq(&provider.resolve(&req, None).unwrap(), &db));
                assert!(provider.save_target(&req).can_harvest(force, measured));
                run_decided(&mut req, &provider, measured, &RwLock::default());
                assert_eq!(
                    db.load_one(req.cache_key_override.as_deref().unwrap())
                        .unwrap()
                        .is_some(),
                    saves
                );
            }
        }
        assert!(!temp.path().join("unused").exists());
    }

    #[test]
    fn collection_epub_fallback_keeps_parent_provider_and_captured_write_epoch() {
        let temp = tempfile::tempdir().unwrap();
        let decoded = image_source(&temp.path().join("converted"));
        let logical = temp.path().join("logical/book.epub");
        let cache = temp.path().join("cache");
        let unused = temp.path().join("fallback-must-not-adopt");
        let provider = ThumbnailCatalogProvider::collection();
        let mut outer = request(&logical, &cache);
        outer.pdf_page = Some(0);
        // A changed conversion stamp takes the real fallback recursion without
        // starting a PDF backend or converter. The fallback decodes a real PNG.
        outer.pdf_stamp_policy = PdfStampPolicy::Resolved(
            crate::pdf_loader::generation_target_for_test(&logical, 17, 4096),
        );
        outer.epub_pin_fallback = Some(Box::new(request(&decoded, &unused)));
        let messages = run(
            &mut outer,
            &provider,
            CachePolicy::Always,
            &RwLock::default(),
        );
        assert!(matches!(
            messages[0].payload,
            ThumbMsgPayload::SourcePixels(_)
        ));
        let identity =
            crate::catalog::collection_source_parent_identity(logical.parent().unwrap()).unwrap();
        let row_key = outer
            .epub_pin_fallback
            .as_ref()
            .unwrap()
            .cache_key_override
            .as_deref()
            .unwrap();
        let db = CatalogWork::capture(&cache)
            .open_existing(&identity)
            .unwrap()
            .unwrap();
        assert!(db.load_one(row_key).unwrap().is_some());
        assert_eq!(crate::catalog::cache_stats(&cache).0, 1);
        assert!(!unused.exists());
        assert_eq!(crate::catalog::delete_all_cache(&cache), 1);
        let messages = run(
            &mut outer,
            &provider,
            CachePolicy::Always,
            &RwLock::default(),
        );
        assert!(messages[0].pixels().is_some());
        assert_eq!(crate::catalog::cache_stats(&cache).0, 0);
        assert!(!unused.exists());
    }

    #[test]
    fn collection_raw_continuation_saves_by_measured_cost_to_original_parent() {
        let temp = tempfile::tempdir().unwrap();
        let logical = temp.path().join("logical/book.zip");
        let cache = temp.path().join("cache");
        let provider = ThumbnailCatalogProvider::collection();
        let mut req = request(&logical, &cache);
        req.path = temp.path().join("converted/half.cr2");
        req.source_policy = LoadSourcePolicy::SourceOnly;
        let developed = || LoadRequestSource::RawHalfDeveloped {
            image: image::DynamicImage::new_rgb8(8, 8),
            developed_dims: [16, 16],
            decode_ms: 500.0,
            folder_selection_proof: None,
        };
        req.raw_source = developed();
        let mut auto = decision(CachePolicy::Auto);
        auto.threshold_ms = 250;
        auto.size_threshold = u64::MAX;
        let messages = run_decided(&mut req, &provider, auto, &RwLock::default());
        assert!(matches!(
            messages[0].payload,
            ThumbMsgPayload::SourcePixels(_)
        ));
        let identity =
            crate::catalog::collection_source_parent_identity(logical.parent().unwrap()).unwrap();
        let db = CatalogWork::capture(&cache)
            .open_existing(&identity)
            .unwrap()
            .unwrap();
        let entry = db
            .load_one(req.cache_key_override.as_deref().unwrap())
            .unwrap()
            .unwrap();
        assert_eq!(entry.source_dims, Some((16, 16)));
        assert_eq!(crate::catalog::cache_stats(&cache).0, 1);
        assert_eq!(crate::catalog::delete_all_cache(&cache), 1);
        req.raw_source = developed();
        let messages = run_decided(&mut req, &provider, auto, &RwLock::default());
        assert!(messages[0].pixels().is_some());
        assert_eq!(crate::catalog::cache_stats(&cache).0, 0);
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
        let db = provider.writer(&req).unwrap();
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
            provider.resolve(&req, None).is_none(),
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
        let second_db = second.writer(&req).unwrap();
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
            let db = provider.writer(&req).unwrap();
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
        let db = provider.resolve(&req, None).unwrap();
        assert!(
            db.is_read_only(),
            "CacheOnly must not borrow a warm RW handle"
        );
        req.source_policy = LoadSourcePolicy::CacheOrSource;
        let db = provider.resolve(&req, None).unwrap();
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
        let db = provider.writer(&old).unwrap();
        let other_db = sibling.writer(&request(other, cache)).unwrap();
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
        assert!(provider.writer(&old).is_none());
        old.source_policy = LoadSourcePolicy::CacheOnly;
        assert!(provider.resolve(&old, None).is_none());
        assert!(!c_path.exists());
        let fresh = request(source, cache);
        let fresh_db = provider.writer(&fresh).unwrap();
        assert!(!fresh_db.is_retired());
        assert_eq!(crate::catalog::delete_old_cache(cache, 0), 2);
        assert!(!c_path.exists() && !d_path.exists());
        assert!(provider.writer(&fresh).is_none());
        let fresh = request(source, cache);
        assert!(provider.writer(&fresh).is_some());
        assert_eq!(crate::catalog::delete_all_cache(cache), 1);
        assert!(!c_path.exists());
    }

    #[test]
    fn collection_parent_cache_only_after_clear_reads_successor_not_old_memory() {
        let temp = tempfile::tempdir().unwrap();
        let source = Path::new(r"C:\photos\image.png");
        let mut req = request(source, temp.path());
        let provider = ThumbnailCatalogProvider::collection();
        let db = provider.writer(&req).unwrap();
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
        let successor = provider.writer(&fresh).unwrap();
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
        assert!(provider.writer(&req).is_none());
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
        let db = provider.writer(&req).unwrap();
        let sibling_db = sibling.writer(&req).unwrap();
        for idx in 0..40 {
            let source = PathBuf::from(format!(r"C:\parent{idx}\same.png"));
            let req = request(&source, temp.path());
            provider.writer(&req).unwrap();
            assert!(provider.resolve(&req, None).unwrap().is_read_only());
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
