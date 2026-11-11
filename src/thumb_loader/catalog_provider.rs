//! Worker-only lazy catalogs. The LRU never performs I/O under its lock.
use super::*;
use crate::catalog::{CatalogDb, CatalogWork};
use std::collections::VecDeque;
use std::path::PathBuf;

#[derive(Clone, Debug)]
pub(crate) struct ParentRow {
    pub(crate) source: PathBuf,
    pub(crate) folder: PathBuf,
    pub(crate) row_key: String,
}

/// Observation belongs to the accepted request, including RAW continuations.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum ParentObservedStamp {
    #[default]
    Unresolved,
    Observed(i64, i64),
    Unavailable,
}

#[derive(Clone)]
pub(crate) enum RequestCatalog {
    Fixed {
        /// Late source resolution retains its request admission as well as the
        /// Fixed connection's own admission. Ordinary Fixed work uses the latter.
        work: Option<CatalogWork>,
    },
    Parent {
        row: ParentRow,
        work: CatalogWork,
    },
    CollectionSourceParent {
        identity: PathBuf,
        source_parent: PathBuf,
        work: CatalogWork,
    },
}

impl Default for RequestCatalog {
    fn default() -> Self {
        Self::Fixed { work: None }
    }
}

impl RequestCatalog {
    pub(crate) fn for_pinned(&self) -> Self {
        match self {
            Self::Parent { work, .. } => Self::Fixed {
                work: Some(work.clone()),
            },
            _ => self.clone(),
        }
    }

    pub(crate) fn collection(source: &Path, work: CatalogWork) -> Self {
        match source
            .parent()
            .and_then(crate::catalog::collection_source_parent_identity)
        {
            Some(identity) => Self::CollectionSourceParent {
                identity,
                source_parent: source.parent().unwrap().to_path_buf(),
                work,
            },
            None => {
                crate::logger::log(format!(
                    "collection thumbnail: invalid source parent: {}",
                    source.display()
                ));
                Self::default()
            }
        }
    }

    pub(crate) fn is_retired(&self) -> bool {
        match self {
            Self::Fixed { work: None } => false,
            Self::Fixed { work: Some(work) } => !work.access().is_admitted(work.identity().1),
            Self::Parent { work, .. } | Self::CollectionSourceParent { work, .. } => {
                !work.access().is_admitted(work.identity().1)
            }
        }
    }

    fn lazy_scope(&self) -> Option<(LazyCatalogKind, &Path, &Path, &CatalogWork)> {
        match self {
            Self::Fixed { .. } => None,
            Self::Parent { row, work } => {
                Some((LazyCatalogKind::Parent, &row.folder, &row.folder, work))
            }
            Self::CollectionSourceParent {
                identity,
                source_parent,
                work,
            } => Some((LazyCatalogKind::Collection, identity, source_parent, work)),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum LazyCatalogKind {
    Collection,
    Parent,
}
impl LazyCatalogKind {
    fn label(self) -> &'static str {
        match self {
            Self::Collection => "collection",
            Self::Parent => "parent",
        }
    }
}

#[derive(Default)]
pub struct LazyCatalogs {
    idle: Mutex<VecDeque<ParentCatalog>>,
}
struct ParentCatalog {
    kind: LazyCatalogKind,
    identity: PathBuf,
    source_parent: PathBuf,
    cache_dir: PathBuf,
    admission: crate::catalog::CatalogAdmission,
    writable: bool,
    db: Arc<std::sync::OnceLock<Option<Arc<CatalogDb>>>>,
}

#[derive(Clone)]
pub struct ThumbnailCatalogProvider {
    fixed: Option<Arc<CatalogDb>>,
    lazy: Arc<LazyCatalogs>,
}

#[derive(Clone, Copy)]
pub struct ThumbnailSaveTarget<'a>(SaveTarget<'a>);
#[derive(Clone, Copy)]
enum SaveTarget<'a> {
    Unavailable,
    Fixed(&'a CatalogDb),
    Lazy {
        pool: &'a LazyCatalogs,
        kind: LazyCatalogKind,
        identity: &'a Path,
        source_parent: &'a Path,
        work: &'a CatalogWork,
        row: Option<(&'a str, i64, i64)>,
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
    pub(super) fn row_identity<'b>(self, key: &'b str, mtime: i64, size: i64) -> (&'b str, i64, i64)
    where
        'a: 'b,
    {
        match self.0 {
            SaveTarget::Lazy { row: Some(row), .. } => row,
            _ => (key, mtime, size),
        }
    }
    pub(super) fn is_parent(self) -> bool {
        matches!(
            self.0,
            SaveTarget::Lazy {
                kind: LazyCatalogKind::Parent,
                ..
            }
        )
    }
    pub(super) fn can_harvest(self, force: bool, decision: CacheDecision) -> bool {
        match self.0 {
            SaveTarget::Unavailable => false,
            SaveTarget::Fixed(_) => true,
            SaveTarget::Lazy { work, .. } => {
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
            SaveTarget::Lazy {
                pool,
                kind,
                identity,
                source_parent,
                work,
                ..
            } => {
                if !work.access().is_admitted(work.identity().1) {
                    return None;
                }
                pool.open(kind, identity, source_parent, work, true)
                    .map(SaveCatalog::Parent)
            }
        }
    }
}
impl From<Option<&Arc<CatalogDb>>> for ThumbnailCatalogProvider {
    fn from(value: Option<&Arc<CatalogDb>>) -> Self {
        Self::fixed(value.cloned())
    }
}
impl ThumbnailCatalogProvider {
    pub fn fixed(fixed: Option<Arc<CatalogDb>>) -> Self {
        Self {
            fixed,
            lazy: Arc::default(),
        }
    }
    pub(crate) fn collection() -> Self {
        Self::fixed(None)
    }
    pub(super) fn uses_collection_source_parents(&self, req: &LoadRequest) -> bool {
        matches!(
            req.catalog_target,
            RequestCatalog::CollectionSourceParent { .. }
        )
    }
    pub(super) fn uses_lazy_catalog(&self, req: &LoadRequest) -> bool {
        req.catalog_target.lazy_scope().is_some()
    }
    pub(super) fn lookup_identity<'a>(
        &self,
        req: &'a LoadRequest,
        key: &'a str,
    ) -> (String, i64, i64) {
        if let RequestCatalog::Parent { row, .. } = &req.catalog_target {
            let (mtime, size) = match req.parent_observed {
                ParentObservedStamp::Observed(mtime, size)
                    if req.source_policy != LoadSourcePolicy::CacheOnly =>
                {
                    (mtime, size)
                }
                _ => (req.mtime, req.file_size),
            };
            (
                representative_cache_key_for_policy(&row.row_key, &req.file_type_visibility),
                mtime,
                size,
            )
        } else {
            (key.to_owned(), req.mtime, req.file_size)
        }
    }
    pub(super) fn save_target<'a>(&'a self, req: &'a LoadRequest) -> ThumbnailSaveTarget<'a> {
        let target = match &req.catalog_target {
            RequestCatalog::Fixed { .. } => {
                if req.catalog_target.is_retired() {
                    return ThumbnailSaveTarget(SaveTarget::Unavailable);
                }
                return self.fixed.as_deref().filter(|db| !db.is_read_only()).into();
            }
            _ if !req.source_policy.allows_source() || req.uses_memory_pin_seed() => {
                SaveTarget::Unavailable
            }
            _ => {
                let (kind, identity, source_parent, work) =
                    req.catalog_target.lazy_scope().unwrap();
                let row = match &req.catalog_target {
                    RequestCatalog::Parent { row, .. } => match req.parent_observed {
                        ParentObservedStamp::Observed(mtime, size) => {
                            Some((row.row_key.as_str(), mtime, size))
                        }
                        _ => return ThumbnailSaveTarget(SaveTarget::Unavailable),
                    },
                    _ => None,
                };
                SaveTarget::Lazy {
                    pool: &self.lazy,
                    kind,
                    identity,
                    source_parent,
                    work,
                    row,
                }
            }
        };
        ThumbnailSaveTarget(target)
    }
    pub(crate) fn resolve(
        &self,
        req: &LoadRequest,
        cancel: Option<&Arc<AtomicBool>>,
    ) -> Option<Arc<CatalogDb>> {
        let Some((kind, identity, source_parent, work)) = req.catalog_target.lazy_scope() else {
            return (!req.catalog_target.is_retired()
                || req.source_policy == LoadSourcePolicy::CacheOnly)
                .then(|| self.fixed.clone())
                .flatten();
        };
        if !req.source_policy.allows_cache() || req.uses_memory_pin_seed() {
            return None;
        }
        let started = std::time::Instant::now();
        let access = work.access();
        let db = if !access.is_admitted(work.identity().1) {
            if req.source_policy != LoadSourcePolicy::CacheOnly {
                return None;
            }
            let admission = access.wait_read_admission_with(&|| {
                cancel.is_some_and(|flag| flag.load(Ordering::Acquire))
            })?;
            LazyCatalogs::open_catalog(
                kind,
                identity,
                source_parent,
                &CatalogWork::with_admission(work.identity().0, admission),
                false,
            )
        } else {
            self.lazy.open(kind, identity, source_parent, work, false)
        };
        if crate::perf::is_enabled() {
            crate::perf::event(
                "thumb",
                "thumb_catalog_open",
                None,
                req.input_seq,
                &[
                    (
                        "ms",
                        serde_json::json!(started.elapsed().as_secs_f64() * 1000.0),
                    ),
                    ("catalog_kind", serde_json::json!(kind.label())),
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
        if !self.uses_lazy_catalog(req) {
            return previous.resume_cache_only_after_maintenance(key, &canceled);
        }
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
impl LazyCatalogs {
    fn open_catalog(
        kind: LazyCatalogKind,
        identity: &Path,
        source_parent: &Path,
        work: &CatalogWork,
        writable: bool,
    ) -> Option<Arc<CatalogDb>> {
        #[cfg(test)]
        LAZY_CATALOG_OPEN_COUNTS.with(|counts| {
            let (reads, writes) = counts.get();
            counts.set((
                reads + usize::from(!writable),
                writes + usize::from(writable),
            ));
        });
        let result = if writable {
            work.open(identity).map(Some)
        } else {
            // Both lazy namespaces publish one complete required-column snapshot.
            work.open_existing_collection_parent(identity)
        };
        match result {
            Ok(db) => db.map(|db| Arc::new(db.with_cover_source_folder(source_parent))),
            Err(error) => {
                let access = work.access();
                if access.is_admitted(work.identity().1) {
                    access
                        .record_cache_error(format!("{} thumbnail catalog: {error}", kind.label()));
                }
                crate::logger::log(format!(
                    "{} thumbnail catalog open failed: {}: {error}",
                    kind.label(),
                    identity.display()
                ));
                None
            }
        }
    }
    fn open(
        &self,
        kind: LazyCatalogKind,
        identity: &Path,
        source_parent: &Path,
        work: &CatalogWork,
        writable: bool,
    ) -> Option<Arc<CatalogDb>> {
        let (slot, retired) = {
            let mut idle = self.idle.lock().unwrap();
            let pos = idle.iter().position(|entry| {
                entry.kind == kind
                    && entry.identity == identity
                    && entry.source_parent == source_parent
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
                    kind,
                    identity: identity.to_owned(),
                    source_parent: source_parent.to_owned(),
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
        let db = slot
            .get_or_init(|| Self::open_catalog(kind, identity, source_parent, work, writable))
            .clone();
        if db.is_none() {
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
thread_local! {
    static LAZY_CATALOG_OPEN_COUNTS: std::cell::Cell<(usize, usize)> = const { std::cell::Cell::new((0, 0)) };
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

    #[test]
    fn cover_identity_collection_readers_keep_source_drive_outside_synthetic_identity() {
        use crate::cover_identity::CoverIdentity;
        let temp = tempfile::tempdir().unwrap();
        let provider = ThumbnailCatalogProvider::collection();
        for drive in ['d', 'e'] {
            let source = PathBuf::from(format!("{drive}:/books/book.zip"));
            let req = request(&source, temp.path());
            let RequestCatalog::CollectionSourceParent {
                identity,
                source_parent,
                ..
            } = &req.catalog_target
            else {
                panic!("expected Collection source parent");
            };
            assert!(identity.starts_with("__collection_source_parent_v1__"));
            assert_eq!(source_parent, source.parent().unwrap());
            let cover = CoverIdentity::Page(format!("{drive}:/books/book.zip::nested/001.png"));
            let writer = provider.writer(&req).unwrap();
            writer
                .save("cover", 1, 2, 3, 4, None, b"pixels", &cover)
                .unwrap();
            assert_eq!(writer.load_one("cover").unwrap().unwrap().cover, cover);
            let reader = provider.resolve(&req, None).unwrap();
            assert!(reader.is_read_only());
            assert_eq!(reader.load_all().unwrap()["cover"].cover, cover);
            assert_eq!(reader.load_one("cover").unwrap().unwrap().cover, cover);
            assert_eq!(
                reader
                    .load_latest_with_prefix("cov")
                    .unwrap()
                    .unwrap()
                    .1
                    .cover,
                cover
            );
            let successor = reader
                .resume_cache_only_catalog_after_maintenance(&|| false)
                .unwrap()
                .unwrap();
            assert_eq!(successor.load_one("cover").unwrap().unwrap().cover, cover);
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
        let pool = &provider.lazy;
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
                &crate::cover_identity::CoverIdentity::Legacy,
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
                &row.cover,
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
            let provider = ThumbnailCatalogProvider::fixed(Some(db.clone()));
            for (policy, force, saves) in [
                (CachePolicy::Off, false, false),
                (CachePolicy::Auto, false, false),
                (CachePolicy::Always, false, true),
                (CachePolicy::Off, true, true),
                (CachePolicy::Auto, false, true),
            ] {
                let mut req = request(&source, &temp.path().join("unused"));
                req.catalog_target = RequestCatalog::default();
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
            &crate::cover_identity::CoverIdentity::Legacy,
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
            .save(
                "after sibling close",
                7,
                9,
                1,
                1,
                None,
                b"still live",
                &crate::cover_identity::CoverIdentity::Legacy,
            )
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
            .save(
                "same.jpg",
                7,
                9,
                1,
                1,
                None,
                b"C contents",
                &crate::cover_identity::CoverIdentity::Legacy,
            )
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
                &crate::cover_identity::CoverIdentity::Legacy,
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
            .save(
                "image.png",
                7,
                9,
                1,
                1,
                None,
                b"ambiguous old row",
                &crate::cover_identity::CoverIdentity::Legacy,
            )
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
        legacy
            .save(
                "old",
                1,
                1,
                1,
                1,
                None,
                b"old",
                &crate::cover_identity::CoverIdentity::Legacy,
            )
            .unwrap();
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
        db.save(
            &key,
            7,
            9,
            w,
            h,
            Some((8, 8)),
            &bytes,
            &crate::cover_identity::CoverIdentity::Legacy,
        )
        .unwrap();
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
            .save(
                &key,
                7,
                9,
                w,
                h,
                Some((8, 8)),
                &bytes,
                &crate::cover_identity::CoverIdentity::Legacy,
            )
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
        let pool = &provider.lazy;
        assert_eq!(pool.idle.lock().unwrap().len(), 16);
        drop(provider);
        assert!(!sibling_db.is_retired());
        sibling_db
            .save(
                "sibling",
                1,
                1,
                1,
                1,
                None,
                b"still writable",
                &crate::cover_identity::CoverIdentity::Legacy,
            )
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

    fn parent_request(source: &Path, cache: &Path) -> LoadRequest {
        let mut req = request(source, cache);
        req.catalog_target = RequestCatalog::Parent {
            row: ParentRow {
                source: source.to_owned(),
                folder: source.parent().unwrap().to_owned(),
                row_key: source.file_name().unwrap().to_str().unwrap().to_owned(),
            },
            work: CatalogWork::capture(cache),
        };
        req
    }

    fn valid_webp() -> Vec<u8> {
        let image = image::DynamicImage::new_rgb8(8, 6);
        crate::catalog::encode_thumb_webp(&image, 64, 75.0)
            .unwrap()
            .0
    }

    fn seed_parent(req: &LoadRequest, cache: &Path, stamp: (i64, i64)) -> CatalogDb {
        let RequestCatalog::Parent { row, .. } = &req.catalog_target else {
            unreachable!()
        };
        let db = CatalogDb::open(cache, &row.folder).unwrap();
        db.save(
            &row.row_key,
            stamp.0,
            stamp.1,
            8,
            6,
            Some((8, 6)),
            &valid_webp(),
            &crate::cover_identity::CoverIdentity::Legacy,
        )
        .unwrap();
        db
    }

    #[test]
    fn parent_generated_row_uses_observed_but_memory_uses_request_and_survives_offline() {
        let temp = tempfile::tempdir().unwrap();
        let cache = temp.path().join("cache");
        let source = image_source(temp.path());
        let fixed = Arc::new(CatalogDb::open(&cache, Path::new("__search_results__")).unwrap());
        let provider = ThumbnailCatalogProvider::fixed(Some(fixed.clone()));
        let mut req = parent_request(&source, &cache);
        req.file_size = 0;
        let map = RwLock::default();
        PARENT_STAT_COUNT.with(|count| count.set(0));
        let messages = run(&mut req, &provider, CachePolicy::Always, &map);
        assert!(matches!(
            messages[0].payload,
            ThumbMsgPayload::SourcePixels(_)
        ));
        assert_eq!(PARENT_STAT_COUNT.with(|count| count.get()), 1);
        let parent = CatalogDb::open(&cache, source.parent().unwrap()).unwrap();
        let entry = parent.load_one("image.png").unwrap().unwrap();
        assert_eq!(
            (entry.mtime, entry.file_size),
            crate::catalog::file_stamp(&std::fs::metadata(&source).unwrap())
        );
        let request_key = req.cache_key_override.as_ref().unwrap();
        assert!(fixed.load_one(request_key).unwrap().is_none());
        assert!(fixed.load_one("image.png").unwrap().is_none());
        assert!(!map.read().unwrap().contains_key("image.png"));
        assert_eq!(map.read().unwrap().get(request_key).unwrap().file_size, 0);
        std::fs::remove_file(&source).unwrap();
        let messages = run(&mut req, &provider, CachePolicy::Off, &map);
        assert_eq!(
            messages[0].pixels().unwrap().origin,
            ThumbLoadOrigin::UpgradeableCache
        );
        assert_eq!(PARENT_STAT_COUNT.with(|count| count.get()), 1);
    }

    #[test]
    fn parent_inverse_lookup_observed_hit_populates_request_memory_without_rewrite() {
        let temp = tempfile::tempdir().unwrap();
        let cache = temp.path().join("cache");
        let source = image_source(temp.path());
        let mut req = parent_request(&source, &cache);
        req.file_size = 0;
        let observed = crate::catalog::file_stamp(&std::fs::metadata(&source).unwrap());
        let db = seed_parent(&req, &cache, observed);
        let original = db.load_one("image.png").unwrap().unwrap().jpeg_data;
        let provider = ThumbnailCatalogProvider::fixed(None);
        let map = RwLock::default();
        let messages = run(&mut req, &provider, CachePolicy::Off, &map);
        assert_eq!(
            messages[0].pixels().unwrap().origin,
            ThumbLoadOrigin::UpgradeableCache
        );
        assert_eq!(
            db.load_one("image.png").unwrap().unwrap().jpeg_data,
            original
        );
        std::fs::remove_file(&source).unwrap();
        PARENT_STAT_COUNT.with(|count| count.set(0));
        let messages = run(&mut req, &provider, CachePolicy::Off, &map);
        assert_eq!(
            messages[0].pixels().unwrap().origin,
            ThumbLoadOrigin::UpgradeableCache
        );
        assert_eq!(PARENT_STAT_COUNT.with(|count| count.get()), 0);
    }

    #[test]
    fn parent_legacy_memory_hit_and_cache_only_never_stat_or_migrate() {
        let temp = tempfile::tempdir().unwrap();
        let cache = temp.path().join("cache");
        let source = temp.path().join("offline/image.png");
        let mut req = parent_request(&source, &cache);
        let map = RwLock::new(std::collections::HashMap::from([(
            req.cache_key_override.clone().unwrap(),
            crate::catalog::CacheEntry {
                cover: crate::cover_identity::CoverIdentity::Legacy,
                mtime: req.mtime,
                file_size: req.file_size,
                jpeg_data: valid_webp(),
                source_dims: Some((8, 6)),
                layout_dims: None,
                folder_provenance: None,
                selection_proof: None,
            },
        )]));
        let provider = ThumbnailCatalogProvider::fixed(None);
        PARENT_STAT_COUNT.with(|count| count.set(0));
        let messages = run(&mut req, &provider, CachePolicy::Always, &map);
        assert_eq!(
            messages[0].pixels().unwrap().origin,
            ThumbLoadOrigin::UpgradeableCache
        );
        assert!(!cache.exists());
        req.source_policy = LoadSourcePolicy::CacheOnly;
        let messages = run(&mut req, &provider, CachePolicy::Always, &RwLock::default());
        assert!(matches!(
            messages[0].payload,
            ThumbMsgPayload::CacheOnlyMiss
        ));
        assert_eq!(PARENT_STAT_COUNT.with(|count| count.get()), 0);
        assert!(!cache.exists());
    }

    #[test]
    fn parent_stat_failure_reads_request_signature_but_never_writes() {
        let temp = tempfile::tempdir().unwrap();
        let cache = temp.path().join("cache");
        let source = temp.path().join("offline/image.png");
        let mut req = parent_request(&source, &cache);
        seed_parent(&req, &cache, (req.mtime, req.file_size));
        let provider = ThumbnailCatalogProvider::fixed(None);
        let messages = run(&mut req, &provider, CachePolicy::Always, &RwLock::default());
        assert_eq!(
            messages[0].pixels().unwrap().origin,
            ThumbLoadOrigin::UpgradeableCache
        );
        observe_parent_stamp(&mut req);
        assert_eq!(req.parent_observed, ParentObservedStamp::Unavailable);
        assert!(provider.save_target(&req).open_if(true, true).is_none());
    }

    #[test]
    fn parent_source_only_and_auto_no_save_do_not_create_or_read_catalogs() {
        let temp = tempfile::tempdir().unwrap();
        let cache = temp.path().join("cache");
        let source = image_source(temp.path());
        let mut req = parent_request(&source, &cache);
        req.source_policy = LoadSourcePolicy::SourceOnly;
        let provider = ThumbnailCatalogProvider::fixed(None);
        let mut cheap = decision(CachePolicy::Auto);
        cheap.threshold_ms = u32::MAX;
        cheap.size_threshold = u64::MAX;
        run_decided(&mut req, &provider, cheap, &RwLock::default());
        assert!(!cache.exists());
        assert!(provider.lazy.idle.lock().unwrap().is_empty());
        let db = seed_parent(&req, &cache, (7, 9));
        let messages = run(&mut req, &provider, CachePolicy::Always, &RwLock::default());
        assert!(matches!(
            messages[0].payload,
            ThumbMsgPayload::SourcePixels(_)
        ));
        assert_ne!(db.load_one("image.png").unwrap().unwrap().file_size, 9);
    }

    #[test]
    fn parent_raw_continuation_inherits_observation_without_second_stat() {
        let temp = tempfile::tempdir().unwrap();
        let source = image_source(temp.path());
        let mut outer = parent_request(&source, &temp.path().join("cache"));
        PARENT_STAT_COUNT.with(|count| count.set(0));
        observe_parent_stamp(&mut outer);
        let mut derived = LoadRequest::default();
        derived.inherit_request_identity_from(&outer);
        std::fs::remove_file(&source).unwrap();
        observe_parent_stamp(&mut derived);
        assert_eq!(derived.parent_observed, outer.parent_observed);
        derived.path = source.clone();
        derived.cache_key_override = outer.cache_key_override.clone();
        derived.mtime = outer.mtime;
        derived.file_size = outer.file_size;
        derived.raw_source = LoadRequestSource::RawHalfDeveloped {
            image: image::DynamicImage::new_rgb8(8, 6),
            developed_dims: [80, 60],
            decode_ms: 2.0,
            folder_selection_proof: None,
        };
        let map = RwLock::default();
        let messages = run(
            &mut derived,
            &ThumbnailCatalogProvider::fixed(None),
            CachePolicy::Always,
            &map,
        );
        assert!(matches!(
            messages[0].payload,
            ThumbMsgPayload::SourcePixels(_)
        ));
        let RequestCatalog::Parent { row, work } = &derived.catalog_target else {
            unreachable!()
        };
        let saved = work
            .open(&row.folder)
            .unwrap()
            .load_one(&row.row_key)
            .unwrap()
            .unwrap();
        let ParentObservedStamp::Observed(mtime, size) = outer.parent_observed else {
            unreachable!()
        };
        assert_eq!((saved.mtime, saved.file_size), (mtime, size));
        assert_eq!(PARENT_STAT_COUNT.with(|count| count.get()), 1);
    }

    #[test]
    fn parent_epub_observation_uses_prepared_generation_without_source_stat() {
        let mut req = parent_request(
            Path::new(r"C:\offline\book.epub"),
            Path::new("unused-cache"),
        );
        req.pdf_page = Some(0);
        let target = crate::pdf_loader::generation_target_for_test(&req.path, 17, 4096);
        let mut prepared = stamp_resolved_pdf_request(&req, &target).unwrap();
        PARENT_STAT_COUNT.with(|count| count.set(0));
        observe_parent_stamp(&mut prepared);
        assert_eq!(
            prepared.parent_observed,
            ParentObservedStamp::Observed(17, 4096)
        );
        assert_eq!(PARENT_STAT_COUNT.with(|count| count.get()), 0);
    }

    #[test]
    fn parent_clear_keeps_cache_only_ro_continuation_and_rejects_old_saves() {
        let temp = tempfile::tempdir().unwrap();
        let cache = temp.path().join("cache");
        let source = image_source(temp.path());
        let mut req = parent_request(&source, &cache);
        observe_parent_stamp(&mut req);
        let provider = ThumbnailCatalogProvider::fixed(None);
        let target = provider.save_target(&req);
        seed_parent(&req, &cache, (7, 9));
        let reader = provider.resolve(&req, None).unwrap();
        crate::catalog::delete_all_cache(&cache);
        assert!(reader.is_retired());
        assert!(target.open_if(true, true).is_none());
        let successor = seed_parent(&req, &cache, (7, 9));
        req.source_policy = LoadSourcePolicy::CacheOnly;
        let messages = run(&mut req, &provider, CachePolicy::Always, &RwLock::default());
        assert_eq!(
            messages[0].pixels().unwrap().origin,
            ThumbLoadOrigin::UpgradeableCache
        );
        assert!(!successor.is_retired());
    }

    #[test]
    fn parent_same_names_use_distinct_dbs_and_lazy_kinds_do_not_alias() {
        let temp = tempfile::tempdir().unwrap();
        let cache = temp.path().join("cache");
        let provider = ThumbnailCatalogProvider::fixed(None);
        for folder in ["F01", "F02"] {
            let source = image_source(&temp.path().join(folder));
            let mut req = parent_request(&source, &cache);
            observe_parent_stamp(&mut req);
            let parent = provider.writer(&req).unwrap();
            let (identity, work) = match &req.catalog_target {
                RequestCatalog::Parent { row, work } => (&row.folder, work),
                _ => unreachable!(),
            };
            let collection = provider
                .lazy
                .open(LazyCatalogKind::Collection, identity, identity, work, true)
                .unwrap();
            assert!(!Arc::ptr_eq(&parent, &collection));
        }
        assert_eq!(provider.lazy.idle.lock().unwrap().len(), 4);
        assert_ne!(
            db_path_for(&cache, &temp.path().join("F01/source")),
            db_path_for(&cache, &temp.path().join("F02/source"))
        );
    }

    #[test]
    fn parent_catalog_open_failure_keeps_source_display_and_records_notice() {
        let temp = tempfile::tempdir().unwrap();
        let source = image_source(temp.path());
        let cache = temp.path().join("cache-is-a-file");
        std::fs::write(&cache, b"blocked").unwrap();
        let mut req = parent_request(&source, &cache);
        let messages = run(
            &mut req,
            &ThumbnailCatalogProvider::fixed(None),
            CachePolicy::Always,
            &RwLock::default(),
        );
        assert!(matches!(
            messages[0].payload,
            ThumbMsgPayload::SourcePixels(_)
        ));
        assert!(
            CatalogAccess::for_cache_dir(&cache)
                .take_cache_notice()
                .is_some()
        );
    }

    #[test]
    fn parent_pdf_meta_uses_admitted_writer_observed_stamp_and_password_rules() {
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("book.pdf");
        std::fs::write(&source, b"source attributes only").unwrap();
        let cache = temp.path().join("cache");
        let mut req = parent_request(&source, &cache);
        req.pdf_page = Some(0);
        req.cache_key_override = Some(format!("pdfthumb:{}", source.display()));
        if let RequestCatalog::Parent { row, .. } = &mut req.catalog_target {
            row.row_key = "pdfthumb:book.pdf".into();
        }
        observe_parent_stamp(&mut req);
        let provider = ThumbnailCatalogProvider::fixed(None);
        let target = provider.save_target(&req);
        assert!(target.open_if(false, false).is_none());
        let writer = target.open_if(false, true).unwrap();
        let (_, mtime, size) = target.row_identity("ignored", req.mtime, req.file_size);
        let db = writer.as_ref();
        save_rendered_pdf_meta(db, "book.pdf", mtime, size, 5, Some("session-password")).unwrap();
        assert!(db.get_pdf_meta("book.pdf", mtime, size).unwrap().is_none());
        save_rendered_pdf_meta(db, "book.pdf", mtime, size, 5, None).unwrap();
        assert_eq!(
            db.get_pdf_meta("book.pdf", mtime, size).unwrap(),
            Some((5, false))
        );
        db.set_pdf_meta("book.pdf", mtime, size, 5, true).unwrap();
        save_rendered_pdf_meta(db, "book.pdf", mtime, size, 6, None).unwrap();
        assert_eq!(
            db.get_pdf_meta("book.pdf", mtime, size).unwrap(),
            Some((6, false))
        );
        assert!(provider.resolve(&req, None).unwrap().is_read_only());
        crate::catalog::delete_all_cache(&cache);
        assert!(target.open_if(true, true).is_none());
    }

    #[test]
    fn parent_pdf_meta_catchup_writes_parent_writer_without_touching_view_catalog() {
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("book.pdf");
        std::fs::write(&source, b"source attributes only").unwrap();
        let cache = temp.path().join("cache");
        let mut req = parent_request(&source, &cache);
        observe_parent_stamp(&mut req);
        let fixed = Arc::new(CatalogDb::open(&cache, Path::new("__search_results__")).unwrap());
        let provider = ThumbnailCatalogProvider::fixed(Some(fixed.clone()));
        let writer = provider.save_target(&req).open_if(true, false).unwrap();
        process_meta_only_with(
            &source,
            writer.as_ref(),
            &Arc::new(AtomicBool::new(false)),
            |_, _, _| Ok(12),
        );
        let stamp = crate::catalog::file_stamp(&std::fs::metadata(&source).unwrap());
        assert_eq!(
            writer
                .as_ref()
                .get_pdf_meta("book.pdf", stamp.0, stamp.1)
                .unwrap(),
            Some((12, false))
        );
        assert!(
            fixed
                .get_pdf_meta("book.pdf", stamp.0, stamp.1)
                .unwrap()
                .is_none()
        );
    }

    fn fake_pdf_render(page_count: u32) -> crate::pdf_loader::RenderResult {
        crate::pdf_loader::RenderResult {
            image: image::DynamicImage::new_rgb8(12, 8),
            content_type: crate::pdf_loader::PdfPageContentType::Vector,
            page_count,
            page_size_points: crate::pdf_loader::PdfPageSizePoints {
                width: 120.0,
                height: 80.0,
            },
        }
    }

    fn parent_pdf_request(source: &Path, cache: &Path) -> LoadRequest {
        let mut req = parent_request(source, cache);
        req.pdf_page = Some(0);
        req.cache_key_override = Some(format!("pdfthumb:{}", source.display()));
        if let RequestCatalog::Parent { row, .. } = &mut req.catalog_target {
            row.row_key = format!("pdfthumb:{}", source.file_name().unwrap().to_str().unwrap());
        }
        req
    }

    fn pdf_memory(
        req: &LoadRequest,
    ) -> RwLock<std::collections::HashMap<String, crate::catalog::CacheEntry>> {
        RwLock::new(std::collections::HashMap::from([(
            req.cache_key_override.clone().unwrap(),
            crate::catalog::CacheEntry {
                cover: crate::cover_identity::CoverIdentity::Legacy,
                mtime: req.mtime,
                file_size: req.file_size,
                jpeg_data: valid_webp(),
                source_dims: Some((8, 6)),
                layout_dims: None,
                folder_provenance: None,
                selection_proof: None,
            },
        )]))
    }

    #[test]
    fn parent_pdf_memory_hit_does_not_stat_open_or_enqueue_catchup() {
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("book.pdf");
        std::fs::write(&source, b"source attributes only").unwrap();
        let (mtime, size) = crate::catalog::file_stamp(&std::fs::metadata(&source).unwrap());
        for (case, observation) in [
            ParentObservedStamp::Unresolved,
            ParentObservedStamp::Observed(mtime, size),
        ]
        .into_iter()
        .enumerate()
        {
            let cache = temp.path().join(format!("cache-{case}"));
            let provider = ThumbnailCatalogProvider::fixed(None);
            let mut req = parent_pdf_request(&source, &cache);
            req.file_size = 0;
            req.parent_observed = observation;
            let memory = pdf_memory(&req);
            PARENT_STAT_COUNT.with(|count| count.set(0));
            LAZY_CATALOG_OPEN_COUNTS.with(|counts| counts.set((0, 0)));
            let (messages, jobs) = with_test_pdf_meta_catchup_capture(|| {
                run(&mut req, &provider, CachePolicy::Always, &memory)
            });
            assert_eq!(
                messages[0].pixels().unwrap().origin,
                ThumbLoadOrigin::UpgradeableCache
            );
            assert!(jobs.is_empty(), "memory hits never enqueue Parent catch-up");
            assert_eq!(PARENT_STAT_COUNT.with(|count| count.get()), 0);
            assert_eq!(LAZY_CATALOG_OPEN_COUNTS.with(|counts| counts.get()), (0, 0));
            assert!(provider.lazy.idle.lock().unwrap().is_empty());
            assert!(!cache.exists());
        }
    }

    #[test]
    fn parent_pdf_observed_db_hit_enqueues_writable_parent_catchup_with_observed_meta() {
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("book.pdf");
        std::fs::write(&source, b"source attributes only").unwrap();
        let cache = temp.path().join("cache");
        let mut req = parent_pdf_request(&source, &cache);
        req.file_size = 0;
        let observed = crate::catalog::file_stamp(&std::fs::metadata(&source).unwrap());
        let parent = seed_parent(&req, &cache, observed);
        let fixed = Arc::new(CatalogDb::open(&cache, Path::new("__search_results__")).unwrap());
        let provider = ThumbnailCatalogProvider::fixed(Some(fixed.clone()));
        PARENT_STAT_COUNT.with(|count| count.set(0));
        LAZY_CATALOG_OPEN_COUNTS.with(|counts| counts.set((0, 0)));
        let (messages, jobs) = with_test_pdf_meta_catchup_capture(|| {
            run(&mut req, &provider, CachePolicy::Off, &RwLock::default())
        });
        assert_eq!(
            messages[0].pixels().unwrap().origin,
            ThumbLoadOrigin::UpgradeableCache
        );
        assert_eq!(PARENT_STAT_COUNT.with(|count| count.get()), 1);
        assert_eq!(LAZY_CATALOG_OPEN_COUNTS.with(|counts| counts.get()), (1, 1));
        assert_eq!(jobs.len(), 1);
        let (path, writer) = &jobs[0];
        assert_eq!(path, &source);
        assert!(!writer.is_read_only());
        assert!(!Arc::ptr_eq(writer, &fixed));
        process_meta_only_with(
            path,
            writer,
            &Arc::new(AtomicBool::new(false)),
            |_, _, _| Ok(12),
        );
        assert_eq!(
            parent
                .get_pdf_meta("book.pdf", observed.0, observed.1)
                .unwrap(),
            Some((12, false))
        );
        assert!(
            parent
                .get_pdf_meta("book.pdf", req.mtime, req.file_size)
                .unwrap()
                .is_none()
        );
        assert!(
            fixed
                .get_pdf_meta("book.pdf", observed.0, observed.1)
                .unwrap()
                .is_none()
        );
        // The list's size=0 must not make the next DB hit re-enqueue existing observed metadata.
        let (_, jobs) = with_test_pdf_meta_catchup_capture(|| {
            run(&mut req, &provider, CachePolicy::Off, &RwLock::default())
        });
        assert!(jobs.is_empty());
    }

    #[test]
    fn parent_pdf_stat_failure_ro_hit_does_not_enqueue_catchup() {
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("missing.pdf");
        let cache = temp.path().join("cache");
        let mut req = parent_pdf_request(&source, &cache);
        let parent = seed_parent(&req, &cache, (req.mtime, req.file_size));
        let provider = ThumbnailCatalogProvider::fixed(None);
        PARENT_STAT_COUNT.with(|count| count.set(0));
        LAZY_CATALOG_OPEN_COUNTS.with(|counts| counts.set((0, 0)));
        let (messages, jobs) = with_test_pdf_meta_catchup_capture(|| {
            run(&mut req, &provider, CachePolicy::Always, &RwLock::default())
        });
        assert_eq!(
            messages[0].pixels().unwrap().origin,
            ThumbLoadOrigin::UpgradeableCache
        );
        assert_eq!(PARENT_STAT_COUNT.with(|count| count.get()), 1);
        assert_eq!(LAZY_CATALOG_OPEN_COUNTS.with(|counts| counts.get()), (1, 0));
        assert!(jobs.is_empty());
        assert!(
            parent
                .get_pdf_meta("missing.pdf", req.mtime, req.file_size)
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn fixed_pdf_memory_hit_keeps_existing_catchup() {
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("book.pdf");
        std::fs::write(&source, b"source attributes only").unwrap();
        let cache = temp.path().join("cache");
        let mut req = parent_pdf_request(&source, &cache);
        req.catalog_target = RequestCatalog::default();
        (req.mtime, req.file_size) =
            crate::catalog::file_stamp(&std::fs::metadata(&source).unwrap());
        let fixed = Arc::new(CatalogDb::open(&cache, Path::new("__search_results__")).unwrap());
        let provider = ThumbnailCatalogProvider::fixed(Some(fixed.clone()));
        let memory = pdf_memory(&req);
        PARENT_STAT_COUNT.with(|count| count.set(0));
        LAZY_CATALOG_OPEN_COUNTS.with(|counts| counts.set((0, 0)));
        let (messages, jobs) = with_test_pdf_meta_catchup_capture(|| {
            run(&mut req, &provider, CachePolicy::Off, &memory)
        });
        assert_eq!(
            messages[0].pixels().unwrap().origin,
            ThumbLoadOrigin::UpgradeableCache
        );
        assert_eq!(PARENT_STAT_COUNT.with(|count| count.get()), 0);
        assert_eq!(LAZY_CATALOG_OPEN_COUNTS.with(|counts| counts.get()), (0, 0));
        assert_eq!(jobs.len(), 1);
        let (path, writer) = &jobs[0];
        assert!(Arc::ptr_eq(writer, &fixed));
        process_meta_only_with(
            path,
            writer,
            &Arc::new(AtomicBool::new(false)),
            |_, _, _| Ok(8),
        );
        assert_eq!(
            fixed
                .get_pdf_meta("book.pdf", req.mtime, req.file_size)
                .unwrap(),
            Some((8, false))
        );
        assert!(!db_path_for(&cache, source.parent().unwrap()).exists());
    }

    #[test]
    fn parent_pdf_worker_render_saves_observed_thumb_and_meta_only_to_parent() {
        for password in [None, Some("session-password")] {
            let temp = tempfile::tempdir().unwrap();
            let source = temp.path().join("book.pdf");
            std::fs::write(&source, b"fake backend source").unwrap();
            let cache = temp.path().join("cache");
            let fixed = Arc::new(CatalogDb::open(&cache, Path::new("__search_results__")).unwrap());
            let provider = ThumbnailCatalogProvider::fixed(Some(fixed.clone()));
            let mut req = parent_pdf_request(&source, &cache);
            req.file_size = 0;
            req.pdf_password = password.map(str::to_owned);
            let memory = RwLock::default();
            PARENT_STAT_COUNT.with(|count| count.set(0));
            let messages = with_test_pdf_render_result(fake_pdf_render(12), || {
                run(&mut req, &provider, CachePolicy::Always, &memory)
            });
            assert!(matches!(
                messages[0].payload,
                ThumbMsgPayload::SourcePixels(_)
            ));
            assert_eq!(PARENT_STAT_COUNT.with(|count| count.get()), 1);
            let stamp = crate::catalog::file_stamp(&std::fs::metadata(&source).unwrap());
            let db = CatalogDb::open(&cache, source.parent().unwrap()).unwrap();
            let row = db.load_one("pdfthumb:book.pdf").unwrap().unwrap();
            assert_eq!((row.mtime, row.file_size), stamp);
            assert_eq!(
                db.get_pdf_meta("book.pdf", stamp.0, stamp.1).unwrap(),
                password.is_none().then_some((12, false))
            );
            assert!(
                fixed
                    .load_one(req.cache_key_override.as_deref().unwrap())
                    .unwrap()
                    .is_none()
            );
            assert!(
                fixed
                    .get_pdf_meta("book.pdf", stamp.0, stamp.1)
                    .unwrap()
                    .is_none()
            );
            let memory = memory.read().unwrap();
            let row = memory
                .get(req.cache_key_override.as_ref().unwrap())
                .unwrap();
            assert_eq!((row.mtime, row.file_size), (7, 0));
        }
    }

    #[test]
    fn parent_epub_worker_legacy_memory_hit_then_source_only_saves_generation() {
        let fixture = crate::epub_cache::reconverted_for_worker_test();
        let temp = tempfile::tempdir().unwrap();
        let cache = temp.path().join("cache");
        let mut req = parent_pdf_request(&fixture.source, &cache);
        req.file_size = 0;
        req.pdf_stamp_policy = PdfStampPolicy::ResolveInWorker;
        let key = req.cache_key_override.clone().unwrap();
        let generation = (
            fixture.current.generation_id,
            fixture.current.pdf_size as i64,
        );
        let map = RwLock::new(std::collections::HashMap::from([(
            key.clone(),
            crate::catalog::CacheEntry {
                cover: crate::cover_identity::CoverIdentity::Legacy,
                mtime: generation.0,
                file_size: generation.1,
                jpeg_data: valid_webp(),
                source_dims: Some((8, 6)),
                layout_dims: None,
                folder_provenance: None,
                selection_proof: None,
            },
        )]));
        let provider = ThumbnailCatalogProvider::fixed(None);
        PARENT_STAT_COUNT.with(|count| count.set(0));
        let messages = run(&mut req, &provider, CachePolicy::Always, &map);
        assert_eq!(
            messages[0].pixels().unwrap().origin,
            ThumbLoadOrigin::UpgradeableCache
        );
        assert_eq!(PARENT_STAT_COUNT.with(|count| count.get()), 0);
        assert!(
            !cache.exists(),
            "displaying the legacy memory row must not migrate it"
        );
        req.source_policy = LoadSourcePolicy::SourceOnly;
        let messages =
            with_test_pdf_render_result(fake_pdf_render(fixture.current.page_count), || {
                run(&mut req, &provider, CachePolicy::Always, &map)
            });
        assert!(matches!(
            messages[0].payload,
            ThumbMsgPayload::SourcePixels(_)
        ));
        assert_eq!(PARENT_STAT_COUNT.with(|count| count.get()), 0);
        let db = CatalogDb::open(&cache, fixture.source.parent().unwrap()).unwrap();
        let row_key = format!(
            "pdfthumb:{}",
            fixture.source.file_name().unwrap().to_str().unwrap()
        );
        let row = db.load_one(&row_key).unwrap().unwrap();
        assert_eq!((row.mtime, row.file_size), generation);
        let memory = map.read().unwrap();
        let row = memory.get(&key).unwrap();
        assert_eq!((row.mtime, row.file_size), generation);
        assert!(memory.get(&row_key).is_none());
    }

    #[test]
    fn parent_source_only_worker_applies_kind_suffix_to_parent_and_request_keys() {
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("book.pdf");
        std::fs::write(&source, b"fake backend source").unwrap();
        let cache = temp.path().join("cache");
        let mut req = parent_pdf_request(&source, &cache);
        req.source_policy = LoadSourcePolicy::SourceOnly;
        req.file_type_visibility
            .excluded_categories
            .insert(crate::file_type_visibility::FileTypeCategory::Audio);
        let memory_key = representative_cache_key_for_policy(
            req.cache_key_override.as_deref().unwrap(),
            &req.file_type_visibility,
        );
        let parent_key =
            representative_cache_key_for_policy("pdfthumb:book.pdf", &req.file_type_visibility);
        assert!(parent_key.contains("|types:"));
        let map = RwLock::default();
        let messages = with_test_pdf_render_result(fake_pdf_render(2), || {
            run(
                &mut req,
                &ThumbnailCatalogProvider::fixed(None),
                CachePolicy::Always,
                &map,
            )
        });
        assert!(matches!(
            messages[0].payload,
            ThumbMsgPayload::SourcePixels(_)
        ));
        let db = CatalogDb::open(&cache, source.parent().unwrap()).unwrap();
        assert!(db.load_one(&parent_key).unwrap().is_some());
        assert!(db.load_one("pdfthumb:book.pdf").unwrap().is_none());
        let map = map.read().unwrap();
        assert!(map.contains_key(&memory_key));
        assert!(!map.contains_key(&parent_key));
    }

    #[cfg(debug_assertions)]
    #[test]
    fn parent_derived_key_mismatch_asserts_before_any_parent_write() {
        let temp = tempfile::tempdir().unwrap();
        let source = image_source(temp.path());
        let cache = temp.path().join("cache");
        let mut req = parent_request(&source, &cache);
        // Admission owns the key even if a derived branch clears parent_row before
        // its first worker invocation (the late color-filter pin regression).
        req.capture_parent_request_key();
        req.parent_row = None;
        req.cache_key_override = Some("different-pinned-key#pin:leaf".into());
        let provider = ThumbnailCatalogProvider::fixed(None);
        PARENT_STAT_COUNT.with(|count| count.set(0));
        LAZY_CATALOG_OPEN_COUNTS.with(|counts| counts.set((0, 0)));
        let map = RwLock::default();
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            run(&mut req, &provider, CachePolicy::Always, &map)
        }));
        assert!(
            result.is_err(),
            "derived request must preserve its outer identity"
        );
        assert!(!cache.exists());
        assert!(map.read().unwrap().is_empty());
        assert_eq!(PARENT_STAT_COUNT.with(|count| count.get()), 0);
        assert_eq!(LAZY_CATALOG_OPEN_COUNTS.with(|counts| counts.get()), (0, 0));
    }

    #[test]
    fn parent_late_pin_fixed_preserves_admission_and_cache_only_continuation() {
        let temp = tempfile::tempdir().unwrap();
        let source = image_source(temp.path());
        let cache = temp.path().join("cache");
        let mut req = parent_request(&source, &cache);
        let RequestCatalog::Parent { work, .. } = &req.catalog_target else {
            unreachable!()
        };
        let work = work.clone();
        let fixed_folder = Path::new("__search_results__");
        let old_fixed = Arc::new(work.open(fixed_folder).unwrap());
        req.capture_parent_request_key();
        req.catalog_target = req.catalog_target.for_pinned();
        req.cache_key_override = Some("pinned-row#pin:leaf".into());
        let RequestCatalog::Fixed {
            work: Some(pin_work),
        } = &req.catalog_target
        else {
            unreachable!()
        };
        assert_eq!(pin_work.identity(), work.identity());
        let map = RwLock::new(std::collections::HashMap::from([(
            req.cache_key_override.clone().unwrap(),
            crate::catalog::CacheEntry {
                cover: crate::cover_identity::CoverIdentity::Legacy,
                mtime: req.mtime,
                file_size: req.file_size,
                jpeg_data: valid_webp(),
                source_dims: Some((1, 1)),
                layout_dims: None,
                folder_provenance: None,
                selection_proof: None,
            },
        )]));
        crate::catalog::delete_all_cache(&cache);
        let successor = Arc::new(CatalogDb::open(&cache, fixed_folder).unwrap());
        let key = req.cache_key_override.clone().unwrap();
        successor
            .save(
                &key,
                req.mtime,
                req.file_size,
                8,
                6,
                Some((16, 12)),
                &valid_webp(),
                &crate::cover_identity::CoverIdentity::Legacy,
            )
            .unwrap();
        req.source_policy = LoadSourcePolicy::CacheOnly;
        let messages = run(
            &mut req,
            &ThumbnailCatalogProvider::fixed(Some(old_fixed)),
            CachePolicy::Always,
            &map,
        );
        assert_eq!(messages[0].pixels().unwrap().source_dims, Some((16, 12)));
        // Even if a caller supplies a successor Fixed connection, a source request
        // must not upgrade the old CatalogWork admission and save into it.
        req.source_policy = LoadSourcePolicy::SourceOnly;
        let provider = ThumbnailCatalogProvider::fixed(Some(successor.clone()));
        assert!(provider.resolve(&req, None).is_none());
        assert!(provider.save_target(&req).open_if(true, true).is_none());
        let messages = run(&mut req, &provider, CachePolicy::Always, &map);
        assert!(matches!(
            messages[0].payload,
            ThumbMsgPayload::SourcePixels(_)
        ));
        assert_eq!(
            successor.load_one(&key).unwrap().unwrap().source_dims,
            Some((16, 12))
        );
        assert!(!db_path_for(&cache, source.parent().unwrap()).exists());
    }
}
