//! Context-owned RAW requests and installed stages. Filesystem work belongs to
//! workers; the first validated info reply establishes the physical identity.

use super::*;
use crate::raw::{RawBrightness, RawDevelopSupport, RawError, RawPriority, RawTicket};

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct RawSourceIdentity {
    pub item_key: String,
    pub path: PathBuf,
    pub size: u64,
    pub mtime_ticks: u64,
}

impl RawSourceIdentity {
    /// Worker-only. Preserve NTFS's full timestamp, including for ZIP sources.
    pub(crate) fn read(item_key: String, path: PathBuf) -> Result<Self, RawError> {
        let stamp = crate::raw::RawSourceFingerprint::read(path)?;
        Ok(Self {
            item_key,
            path: stamp.path,
            size: stamp.size,
            mtime_ticks: stamp.mtime_ticks,
        })
    }

    pub(crate) fn validate(&self) -> Result<(), RawError> {
        self.fingerprint().validate()
    }

    pub(crate) fn fingerprint(&self) -> crate::raw::RawSourceFingerprint {
        crate::raw::RawSourceFingerprint {
            path: self.path.clone(),
            size: self.size,
            mtime_ticks: self.mtime_ticks,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RawInstalledStage {
    Nothing,
    PreviewShown,
    PreviewAbsent,
    Developed,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum RawPreviewPhase {
    NotRequested,
    Requested { request_id: u64 },
    Done,
    Absent,
    Failed(RawError),
}

pub(crate) enum RawDevelopPhase {
    Idle,
    Preparing {
        request_id: u64,
        cancel: Arc<Mutex<FsPageLoadTicket>>,
        highest_priority: RawPriority,
        brightness: RawBrightness,
    },
    Submitted {
        request_id: u64,
        ticket: Arc<RawTicket>,
        brightness: RawBrightness,
    },
    Done,
    Blocked(RawDevelopBlocked),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum RawDevelopBlocked {
    Unsupported,
    Failed {
        error: RawError,
        brightness: RawBrightness,
    },
}

impl RawDevelopPhase {
    pub(crate) fn cancel(&mut self) {
        match self {
            Self::Preparing { cancel, .. } => cancel.lock().unwrap().cancel(),
            Self::Submitted { ticket, .. } => ticket.cancel(),
            _ => return,
        }
        *self = Self::Idle;
    }

    pub(crate) fn promote(&mut self) {
        match self {
            Self::Preparing {
                highest_priority,
                cancel,
                ..
            } => {
                *highest_priority = RawPriority::High;
                cancel
                    .lock()
                    .unwrap()
                    .promote_to_high(FsPageLoadContract::Sequential);
            }
            Self::Submitted { ticket, .. } => ticket.promote_to_high(),
            _ => {}
        }
    }

    pub(crate) fn publish(&mut self, request_id: u64, ticket: Arc<RawTicket>) {
        if let Self::Preparing {
            request_id: current,
            highest_priority,
            brightness,
            ..
        } = self
            && *current == request_id
        {
            if *highest_priority == RawPriority::High {
                ticket.promote_to_high();
            }
            *self = Self::Submitted {
                request_id,
                ticket,
                brightness: *brightness,
            };
        } else {
            ticket.cancel();
        }
    }

    fn accepts(&self, request_id: u64, brightness: RawBrightness) -> bool {
        matches!(self, Self::Preparing { request_id: id, brightness: b, .. }
            | Self::Submitted { request_id: id, brightness: b, .. } if *id == request_id && *b == brightness)
    }
}

pub(crate) struct RawPageState {
    pub source: RawSourceIdentity,
    pub developed_dims: Option<[usize; 2]>,
    pub stage: RawInstalledStage,
    pub preview: RawPreviewPhase,
    pub develop: Arc<Mutex<RawDevelopPhase>>,
    pub preview_started_at: std::time::Instant,
    pub develop_started_at: std::time::Instant,
    pub presented: RawPresentation,
}

impl RawPageState {
    fn brightness_changed(&mut self) {
        if self.stage == RawInstalledStage::Developed {
            self.stage = RawInstalledStage::Nothing;
            self.preview = RawPreviewPhase::NotRequested;
            self.presented = RawPresentation::Nothing;
        }
        let mut develop = self.develop.lock().unwrap();
        develop.cancel();
        if matches!(
            *develop,
            RawDevelopPhase::Done | RawDevelopPhase::Blocked(RawDevelopBlocked::Failed { .. })
        ) {
            *develop = RawDevelopPhase::Idle;
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RawPresentationEvent {
    Preview { request_id: u64 },
    Developed { request_id: u64 },
}
pub(crate) enum RawPresentation {
    Nothing,
    Awaiting {
        event: RawPresentationEvent,
        started_at: std::time::Instant,
    },
    Presented(RawPresentationEvent),
}

/// Entry and state leave their generation maps together before snapshot rebuild.
pub(super) struct FsPageTransfer {
    pub entry: Option<FsCacheEntry>,
    pub raw: Option<RawPageState>,
}

pub enum RawPageResult {
    Info {
        developed_dims: [usize; 2],
        support: RawDevelopSupport,
    },
    Preview {
        pixels: Option<egui::ColorImage>,
    },
    Develop(Box<FsLoadResult>),
    Failed(RawError),
}

pub(crate) enum RawApplyOutcome {
    Rejected,
    Updated,
    Installed {
        developed: bool,
        high_res: Option<crate::panorama::HighResSource>,
    },
    Stale,
}

// Before the first worker stat, no physical fingerprint exists. Keep this
// mutually exclusive with a resolved page; never invent a zero timestamp.
pub(crate) enum RawPageRecord {
    Resolving {
        item_key: String,
        request_id: u64,
        started_at: std::time::Instant,
    },
    Page(RawPageState),
    SourceFailed {
        item_key: String,
        error: RawError,
    },
}

fn discard_record(record: &mut RawPageRecord) {
    if let RawPageRecord::Page(page) = record {
        page.develop.lock().unwrap().cancel();
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RawResultStage {
    Info,
    Preview,
    Develop(RawBrightness),
}

#[derive(Debug, Clone)]
pub struct RawResultTag {
    pub context: u64,
    pub generation: u64,
    pub idx: usize,
    pub request_id: u64,
    pub source: RawSourceIdentity,
    pub stage: RawResultStage,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RawPageLoadState {
    PreviewNotRequested,
    PreviewPending,
    PreviewShown,
    PreviewAbsent,
    Developed,
    Terminal,
}

impl RawPageLoadState {
    pub(crate) fn waiting_for_display(self) -> bool {
        matches!(
            self,
            Self::PreviewNotRequested | Self::PreviewPending | Self::PreviewAbsent
        )
    }
    pub(crate) fn terminal(self) -> bool {
        self == Self::Terminal
    }
}

pub(crate) struct RawPageStore {
    pub pages: ItemsGenerationMap<RawPageRecord>,
    next_request_id: u64,
    pub tx: mpsc::Sender<FsUploadResult>,
    pub rx: mpsc::Receiver<FsUploadResult>,
    pub demand: HashMap<usize, RawPriority>,
}

impl RawPageStore {
    pub(crate) fn new() -> Self {
        let (tx, rx) = mpsc::channel();
        Self {
            pages: ItemsGenerationMap::with_discard("raw_pages", discard_record),
            next_request_id: 1,
            tx,
            rx,
            demand: HashMap::new(),
        }
    }
    pub(crate) fn next_id(&mut self) -> u64 {
        let id = self.next_request_id;
        self.next_request_id = id.checked_add(1).expect("RAW request ID exhausted");
        id
    }
    pub(crate) fn page(&self, idx: usize) -> Option<&RawPageState> {
        match self.pages.get(&idx)? {
            RawPageRecord::Page(page) => Some(page),
            _ => None,
        }
    }
    pub(crate) fn page_mut(&mut self, idx: usize) -> Option<&mut RawPageState> {
        match self.pages.get_mut(&idx)? {
            RawPageRecord::Page(page) => Some(page),
            _ => None,
        }
    }
    pub(crate) fn begin_preview(&mut self, idx: usize, item_key: String) -> u64 {
        let id = self.next_id();
        if let Some(page) = self.page_mut(idx) {
            page.preview = RawPreviewPhase::Requested { request_id: id };
            page.preview_started_at = std::time::Instant::now();
        } else {
            self.pages.insert(
                idx,
                RawPageRecord::Resolving {
                    item_key,
                    request_id: id,
                    started_at: std::time::Instant::now(),
                },
            );
        }
        id
    }
    pub(crate) fn source_failed(&mut self, idx: usize, request_id: u64, error: RawError) {
        if let Some(RawPageRecord::Resolving {
            item_key,
            request_id: id,
            ..
        }) = self.pages.get(&idx)
            && *id == request_id
        {
            let item_key = item_key.clone();
            self.pages
                .insert(idx, RawPageRecord::SourceFailed { item_key, error });
        }
    }

    pub(crate) fn preview_disconnected(&mut self, idx: usize) {
        if matches!(self.pages.get(&idx), Some(RawPageRecord::Resolving { .. })) {
            self.discard(idx);
        } else if let Some(page) = self.page_mut(idx)
            && matches!(page.preview, RawPreviewPhase::Requested { .. })
        {
            page.preview = RawPreviewPhase::NotRequested;
        }
    }
    pub(crate) fn resolve_source(&mut self, tag: &RawResultTag, context: u64) -> bool {
        if tag.context != context || !self.pages.accepts_generation(tag.idx, tag.generation) {
            return false;
        }
        if let Some(RawPageRecord::Resolving {
            item_key,
            request_id,
            started_at,
        }) = self.pages.get(&tag.idx)
        {
            if *request_id != tag.request_id || *item_key != tag.source.item_key {
                return false;
            }
            let started_at = *started_at;
            self.pages.insert(
                tag.idx,
                RawPageRecord::Page(RawPageState {
                    source: tag.source.clone(),
                    developed_dims: None,
                    stage: RawInstalledStage::Nothing,
                    preview: RawPreviewPhase::Requested {
                        request_id: tag.request_id,
                    },
                    develop: Arc::new(Mutex::new(RawDevelopPhase::Idle)),
                    preview_started_at: started_at,
                    develop_started_at: started_at,
                    presented: RawPresentation::Nothing,
                }),
            );
        }
        self.accepts(tag, context)
    }
    pub(crate) fn accepts(&self, tag: &RawResultTag, context: u64) -> bool {
        if tag.context != context || !self.pages.accepts_generation(tag.idx, tag.generation) {
            return false;
        }
        let Some(page) = self.page(tag.idx) else {
            return false;
        };
        if page.source != tag.source {
            return false;
        }
        match tag.stage {
            RawResultStage::Info | RawResultStage::Preview => matches!(page.preview,
                RawPreviewPhase::Requested { request_id } if request_id == tag.request_id),
            RawResultStage::Develop(brightness) => page
                .develop
                .lock()
                .unwrap()
                .accepts(tag.request_id, brightness),
        }
    }
    pub(crate) fn classify(&self, idx: usize) -> RawPageLoadState {
        let Some(record) = self.pages.get(&idx) else {
            return RawPageLoadState::PreviewNotRequested;
        };
        let page = match record {
            RawPageRecord::Resolving { .. } => return RawPageLoadState::PreviewPending,
            RawPageRecord::SourceFailed { .. } => return RawPageLoadState::Terminal,
            RawPageRecord::Page(page) => page,
        };
        if page.stage == RawInstalledStage::Developed {
            return RawPageLoadState::Developed;
        }
        if page.stage == RawInstalledStage::PreviewShown {
            return RawPageLoadState::PreviewShown;
        }
        if matches!(*page.develop.lock().unwrap(), RawDevelopPhase::Blocked(_))
            && matches!(
                page.preview,
                RawPreviewPhase::Absent | RawPreviewPhase::Failed(_)
            )
        {
            return RawPageLoadState::Terminal;
        }
        match page.preview {
            RawPreviewPhase::NotRequested => RawPageLoadState::PreviewNotRequested,
            RawPreviewPhase::Requested { .. } => RawPageLoadState::PreviewPending,
            RawPreviewPhase::Absent | RawPreviewPhase::Failed(_) => RawPageLoadState::PreviewAbsent,
            RawPreviewPhase::Done => RawPageLoadState::PreviewShown,
        }
    }

    /// The sole writer for RAW fullscreen entries. Validate before allocating a
    /// texture; dimensions and terminal failures pass the same identity gate.
    pub(crate) fn apply_result(
        &mut self,
        tag: &RawResultTag,
        result: RawPageResult,
        context: u64,
        ctx: &egui::Context,
        cache: &mut ItemsGenerationMap<FsCacheEntry>,
        early_dims: &mut ItemsGenerationMap<[usize; 2]>,
        load_seq: u64,
    ) -> RawApplyOutcome {
        if !self.resolve_source(tag, context) {
            return RawApplyOutcome::Rejected;
        }
        if matches!(result, RawPageResult::Failed(RawError::Stale)) {
            return RawApplyOutcome::Stale;
        }
        let page = self.page_mut(tag.idx).unwrap();
        let mut high_res = None;
        let entry = match result {
            RawPageResult::Info {
                developed_dims,
                support,
            } => {
                page.developed_dims = Some(developed_dims);
                early_dims.insert_for_generation(tag.idx, tag.generation, developed_dims);
                if matches!(support, RawDevelopSupport::Unsupported(_)) {
                    *page.develop.lock().unwrap() =
                        RawDevelopPhase::Blocked(RawDevelopBlocked::Unsupported);
                }
                return RawApplyOutcome::Updated;
            }
            RawPageResult::Preview { pixels } => {
                page.preview = if pixels.is_some() {
                    RawPreviewPhase::Done
                } else {
                    RawPreviewPhase::Absent
                };
                if page.stage == RawInstalledStage::Developed {
                    return RawApplyOutcome::Updated;
                }
                page.stage = if pixels.is_some() {
                    RawInstalledStage::PreviewShown
                } else {
                    RawInstalledStage::PreviewAbsent
                };
                page.presented = if pixels.is_some() {
                    RawPresentation::Awaiting {
                        event: RawPresentationEvent::Preview {
                            request_id: tag.request_id,
                        },
                        started_at: page.preview_started_at,
                    }
                } else {
                    RawPresentation::Nothing
                };
                FsCacheEntry::RawPreview {
                    preview: pixels.map(|pixels| {
                        let tex = ctx.load_texture(
                            format!("raw_preview_{}", tag.idx),
                            pixels.clone(),
                            egui::TextureOptions::LINEAR,
                        );
                        crate::fs_animation::RawPreviewTexture {
                            tex,
                            pixels: Arc::new(pixels),
                        }
                    }),
                    developed_dims: page.developed_dims.expect("info precedes preview"),
                    load_seq,
                }
            }
            RawPageResult::Develop(result) => {
                let (ci, source_dims) = match *result {
                    FsLoadResult::Static {
                        ci, source_dims, ..
                    } => (ci, source_dims),
                    FsLoadResult::StaticPanorama {
                        ci,
                        source_dims,
                        high_res: source,
                    } => {
                        high_res = Some(source);
                        (ci, source_dims)
                    }
                    _ => unreachable!("RAW development produces a static raster"),
                };
                let tex = ctx.load_texture(
                    format!("fs_{}", tag.idx),
                    ci.clone(),
                    egui::TextureOptions::LINEAR,
                );
                page.developed_dims = Some(source_dims);
                page.stage = RawInstalledStage::Developed;
                // No extra preview retention after development.
                page.preview = RawPreviewPhase::NotRequested;
                *page.develop.lock().unwrap() = RawDevelopPhase::Done;
                page.presented = RawPresentation::Awaiting {
                    event: RawPresentationEvent::Developed {
                        request_id: tag.request_id,
                    },
                    started_at: page.develop_started_at,
                };
                FsCacheEntry::Static {
                    tex,
                    pixels: Arc::new(ci),
                    source_dims: Some(source_dims),
                    load_seq,
                    animation: StaticAnimationState::Still,
                }
            }
            RawPageResult::Failed(error) => {
                if error == RawError::Cancelled {
                    match tag.stage {
                        RawResultStage::Develop(_) => {
                            *page.develop.lock().unwrap() = RawDevelopPhase::Idle
                        }
                        _ => page.preview = RawPreviewPhase::NotRequested,
                    }
                    return RawApplyOutcome::Updated;
                }
                match tag.stage {
                    RawResultStage::Develop(brightness) => {
                        *page.develop.lock().unwrap() = RawDevelopPhase::Blocked(
                            if matches!(error, RawError::Unsupported(_)) {
                                RawDevelopBlocked::Unsupported
                            } else {
                                RawDevelopBlocked::Failed { error, brightness }
                            },
                        );
                    }
                    _ => {
                        page.preview = RawPreviewPhase::Failed(error);
                        if page.stage != RawInstalledStage::Developed {
                            page.stage = RawInstalledStage::PreviewAbsent;
                        }
                    }
                }
                return RawApplyOutcome::Updated;
            }
        };
        cache.insert_for_generation(tag.idx, tag.generation, entry);
        RawApplyOutcome::Installed {
            developed: page.stage == RawInstalledStage::Developed,
            high_res,
        }
    }
    pub(crate) fn fallback_allowed(&self, idx: usize) -> bool {
        self.page(idx)
            .is_some_and(|page| page.preview == RawPreviewPhase::Done)
    }
    pub(crate) fn has_pending(&self) -> bool {
        self.pages.values().any(|record| match record {
            RawPageRecord::Resolving { .. } => true,
            RawPageRecord::Page(page) => {
                matches!(page.preview, RawPreviewPhase::Requested { .. })
                    || matches!(
                        *page.develop.lock().unwrap(),
                        RawDevelopPhase::Preparing { .. } | RawDevelopPhase::Submitted { .. }
                    )
            }
            RawPageRecord::SourceFailed { .. } => false,
        })
    }
    pub(crate) fn park(&mut self) {
        let resolving: Vec<_> = self
            .pages
            .iter()
            .filter_map(|(&idx, record)| {
                matches!(record, RawPageRecord::Resolving { .. }).then_some(idx)
            })
            .collect();
        for idx in resolving {
            self.pages.remove(&idx);
        }
        for record in self.pages.values_mut() {
            if let RawPageRecord::Page(page) = record {
                if matches!(page.preview, RawPreviewPhase::Requested { .. }) {
                    page.preview = RawPreviewPhase::NotRequested;
                }
                page.develop.lock().unwrap().cancel();
            }
        }
    }
    pub(crate) fn cancel_development_all(&self) {
        for record in self.pages.values() {
            if let RawPageRecord::Page(page) = record {
                page.develop.lock().unwrap().cancel();
            }
        }
    }
    pub(crate) fn clear(&mut self) {
        self.pages.clear();
        self.demand.clear();
        while self.rx.try_recv().is_ok() {}
    }
    pub(crate) fn set_items_generation(&mut self, generation: u64) {
        self.pages.set_items_generation(generation);
        self.demand.clear();
    }
    pub(crate) fn discard(&mut self, idx: usize) {
        if let Some(mut record) = self.pages.remove(&idx) {
            discard_record(&mut record);
        }
    }
}

impl Drop for RawPageStore {
    fn drop(&mut self) {
        self.clear();
    }
}

const RAW_DEVELOP_FORWARD: usize = 2;
const RAW_DEVELOP_BACK: usize = 1;

fn raw_development_demand(
    order: &[usize],
    current: usize,
    displayed: &[usize],
    is_raw: impl Fn(usize) -> bool,
) -> HashMap<usize, RawPriority> {
    let mut demand = HashMap::new();
    if let Some(pos) = order.iter().position(|&idx| idx == current) {
        for &idx in &order
            [pos.saturating_sub(RAW_DEVELOP_BACK)..(pos + RAW_DEVELOP_FORWARD + 1).min(order.len())]
        {
            if is_raw(idx) {
                demand.insert(idx, RawPriority::Normal);
            }
        }
    }
    for &idx in displayed {
        if is_raw(idx) {
            demand.insert(idx, RawPriority::High);
        }
    }
    demand
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::raw::{RawPreviewUnavailableReason, RawUnsupportedReason};

    struct Fixture {
        store: RawPageStore,
        cache: ItemsGenerationMap<FsCacheEntry>,
        dims: ItemsGenerationMap<[usize; 2]>,
        ctx: egui::Context,
        tag: RawResultTag,
    }

    impl Fixture {
        fn new() -> Self {
            let mut store = RawPageStore::new();
            store.set_items_generation(7);
            let id = store.begin_preview(0, "camera.dng".into());
            let mut cache = ItemsGenerationMap::new("test_raw_cache");
            cache.set_items_generation(7);
            let mut dims = ItemsGenerationMap::new("test_raw_dims");
            dims.set_items_generation(7);
            Self {
                store,
                cache,
                dims,
                ctx: egui::Context::default(),
                tag: RawResultTag {
                    context: 11,
                    generation: 7,
                    idx: 0,
                    request_id: id,
                    source: RawSourceIdentity {
                        item_key: "camera.dng".into(),
                        path: "camera.dng".into(),
                        size: 50,
                        mtime_ticks: 200,
                    },
                    stage: RawResultStage::Info,
                },
            }
        }
        fn apply(&mut self, result: RawPageResult) -> RawApplyOutcome {
            self.store.apply_result(
                &self.tag,
                result,
                11,
                &self.ctx,
                &mut self.cache,
                &mut self.dims,
                1,
            )
        }
        fn info(&mut self, support: RawDevelopSupport) {
            assert!(matches!(
                self.apply(RawPageResult::Info {
                    developed_dims: [12000, 8000],
                    support
                }),
                RawApplyOutcome::Updated
            ));
        }
        fn preview(&mut self, pixels: bool) {
            self.tag.stage = RawResultStage::Preview;
            let pixels = pixels.then(|| egui::ColorImage::filled([3, 2], egui::Color32::GRAY));
            assert!(matches!(
                self.apply(RawPageResult::Preview { pixels }),
                RawApplyOutcome::Installed {
                    developed: false,
                    ..
                }
            ));
        }
        fn preparing(&mut self, brightness: RawBrightness) -> Arc<AtomicBool> {
            self.tag.request_id = self.store.next_id();
            self.tag.stage = RawResultStage::Develop(brightness);
            let scheduler = FsPageLoadScheduler::with_limits(1, 0);
            let ticket = scheduler.request(
                11,
                0,
                FsPageLoadPriority::High,
                FsPageLoadContract::Sequential,
                None,
                0,
            );
            let cancel = ticket.cancel_token();
            *self.store.page(0).unwrap().develop.lock().unwrap() = RawDevelopPhase::Preparing {
                request_id: self.tag.request_id,
                cancel: Arc::new(Mutex::new(ticket)),
                highest_priority: RawPriority::Normal,
                brightness,
            };
            cancel
        }
        fn developed(&mut self) {
            self.preparing(RawBrightness::MatchPreview);
            assert!(matches!(
                self.apply(RawPageResult::Develop(Box::new(FsLoadResult::Static {
                    ci: egui::ColorImage::filled([6, 4], egui::Color32::WHITE),
                    source_dims: [12000, 8000],
                    animation: StaticAnimationState::Still,
                }))),
                RawApplyOutcome::Installed {
                    developed: true,
                    ..
                }
            ));
        }
    }

    #[test]
    fn validated_preview_and_developed_have_distinct_readiness_and_monotonic_installation() {
        let mut f = Fixture::new();
        assert_eq!(f.store.classify(0), RawPageLoadState::PreviewPending);
        assert!(!f.store.fallback_allowed(0));
        f.info(RawDevelopSupport::Supported);
        assert_eq!(f.dims.get(&0), Some(&[12000, 8000]));
        assert!(!f.store.fallback_allowed(0));
        let mut late_preview = f.tag.clone();
        late_preview.stage = RawResultStage::Preview;
        f.preview(true);
        assert_eq!(f.store.classify(0), RawPageLoadState::PreviewShown);
        assert!(f.store.fallback_allowed(0));
        f.developed();
        assert_eq!(f.store.classify(0), RawPageLoadState::Developed);
        assert!(!f.store.accepts(&late_preview, 11));
        assert!(matches!(
            f.cache.get(&0),
            Some(FsCacheEntry::Static {
                source_dims: Some([12000, 8000]),
                ..
            })
        ));
        assert_eq!(
            f.store.page(0).unwrap().preview,
            RawPreviewPhase::NotRequested
        );
    }

    #[test]
    fn every_identity_component_rejects_before_dimensions_or_texture_upload() {
        let mut f = Fixture::new();
        for bad in 0..6 {
            let mut tag = f.tag.clone();
            match bad {
                0 => tag.context += 1,
                1 => tag.generation += 1,
                2 => tag.request_id += 1,
                3 => tag.source.item_key.push('x'),
                4 => tag.source.size += 1,
                _ => tag.source.mtime_ticks += 1,
            }
            if bad >= 4 {
                f.info(RawDevelopSupport::Supported);
            }
            assert!(matches!(
                f.store.apply_result(
                    &tag,
                    RawPageResult::Info {
                        developed_dims: [1, 1],
                        support: RawDevelopSupport::Supported
                    },
                    11,
                    &f.ctx,
                    &mut f.cache,
                    &mut f.dims,
                    1
                ),
                RawApplyOutcome::Rejected
            ));
            assert!(!f.cache.contains_key(&0));
        }
        f.preparing(RawBrightness::None);
        let mut old = f.tag.clone();
        old.stage = RawResultStage::Develop(RawBrightness::MatchPreview);
        assert!(!f.store.accepts(&old, 11));
    }

    #[test]
    fn absent_corrupt_and_orientation_rejected_preview_never_admit_warm_thumbnail() {
        for reason in [
            RawPreviewUnavailableReason::NoSupportedCandidate,
            RawPreviewUnavailableReason::DecodeFailed,
            RawPreviewUnavailableReason::OrientationMismatch,
        ] {
            let mut f = Fixture::new();
            f.info(RawDevelopSupport::Supported);
            f.tag.stage = RawResultStage::Preview;
            assert!(matches!(
                f.apply(RawPageResult::Failed(RawError::NoUsablePreview(reason))),
                RawApplyOutcome::Updated
            ));
            assert_eq!(f.store.classify(0), RawPageLoadState::PreviewAbsent);
            assert!(!f.store.fallback_allowed(0));
            f.preparing(RawBrightness::None);
            f.apply(RawPageResult::Failed(RawError::OutOfMemory));
            assert_eq!(f.store.classify(0), RawPageLoadState::Terminal);
            assert!(!f.store.fallback_allowed(0));
        }
    }

    #[test]
    fn unsupported_preview_remains_displayable_but_absent_is_terminal() {
        for pixels in [false, true] {
            let mut f = Fixture::new();
            f.info(RawDevelopSupport::Unsupported(
                RawUnsupportedReason::Decoder,
            ));
            f.preview(pixels);
            assert_eq!(
                f.store.classify(0),
                if pixels {
                    RawPageLoadState::PreviewShown
                } else {
                    RawPageLoadState::Terminal
                }
            );
            f.store.page_mut(0).unwrap().brightness_changed();
            assert!(matches!(
                *f.store.page(0).unwrap().develop.lock().unwrap(),
                RawDevelopPhase::Blocked(RawDevelopBlocked::Unsupported)
            ));
        }
    }

    #[test]
    fn brightness_preserves_every_preview_axis_and_invalidates_only_development_identity() {
        for preview in [
            RawPreviewPhase::NotRequested,
            RawPreviewPhase::Requested { request_id: 1 },
            RawPreviewPhase::Done,
            RawPreviewPhase::Absent,
            RawPreviewPhase::Failed(RawError::Corrupt("jpeg".into())),
        ] {
            for stage in [
                RawInstalledStage::Nothing,
                RawInstalledStage::PreviewShown,
                RawInstalledStage::PreviewAbsent,
            ] {
                for phase in 0..4 {
                    let mut f = Fixture::new();
                    f.info(RawDevelopSupport::Supported);
                    let cancel = f.preparing(RawBrightness::MatchPreview);
                    let page = f.store.page_mut(0).unwrap();
                    page.preview = preview.clone();
                    page.stage = stage;
                    match phase {
                        0 => *page.develop.lock().unwrap() = RawDevelopPhase::Idle,
                        1 => {}
                        2 => *page.develop.lock().unwrap() = RawDevelopPhase::Done,
                        _ => {
                            *page.develop.lock().unwrap() =
                                RawDevelopPhase::Blocked(RawDevelopBlocked::Failed {
                                    error: RawError::OutOfMemory,
                                    brightness: RawBrightness::MatchPreview,
                                })
                        }
                    }
                    page.brightness_changed();
                    assert_eq!(page.preview, preview);
                    assert_eq!(page.stage, stage);
                    assert!(matches!(
                        *page.develop.lock().unwrap(),
                        RawDevelopPhase::Idle
                    ));
                    if phase == 1 {
                        assert!(cancel.load(Ordering::Acquire));
                    }
                    assert!(!f.store.accepts(&f.tag, 11));
                }
            }
        }
    }

    #[test]
    fn preview_upload_survives_brightness_change_and_developed_requests_preview_again() {
        let mut f = Fixture::new();
        f.info(RawDevelopSupport::Supported);
        f.store.page_mut(0).unwrap().brightness_changed();
        f.preview(true);
        f.developed();
        f.store.page_mut(0).unwrap().brightness_changed();
        assert_eq!(f.store.classify(0), RawPageLoadState::PreviewNotRequested);
        assert_eq!(f.store.page(0).unwrap().stage, RawInstalledStage::Nothing);
        assert!(!f.store.accepts(&f.tag, 11));
    }

    #[test]
    fn park_keeps_done_and_blocked_but_cancels_preparation_and_preview_requests() {
        for done in [false, true] {
            let mut f = Fixture::new();
            f.info(RawDevelopSupport::Supported);
            let cancel = f.preparing(RawBrightness::None);
            if done {
                *f.store.page(0).unwrap().develop.lock().unwrap() = RawDevelopPhase::Done;
            }
            f.store.park();
            assert_eq!(
                f.store.page(0).unwrap().preview,
                RawPreviewPhase::NotRequested
            );
            assert!(
                matches!(
                    *f.store.page(0).unwrap().develop.lock().unwrap(),
                    RawDevelopPhase::Idle
                ) == !done
            );
            if !done {
                assert!(cancel.load(Ordering::Acquire));
            }
            assert!(!f.store.accepts(&f.tag, 11));
        }
        let mut f = Fixture::new();
        f.info(RawDevelopSupport::Unsupported(
            RawUnsupportedReason::Decoder,
        ));
        f.store.park();
        assert!(matches!(
            *f.store.page(0).unwrap().develop.lock().unwrap(),
            RawDevelopPhase::Blocked(_)
        ));
    }

    #[test]
    fn current_stale_requests_rebuild_and_superseded_stale_is_rejected() {
        let mut f = Fixture::new();
        f.info(RawDevelopSupport::Supported);
        f.preparing(RawBrightness::None);
        let stale = f.tag.clone();
        assert!(matches!(
            f.apply(RawPageResult::Failed(RawError::Stale)),
            RawApplyOutcome::Stale
        ));
        f.store.discard(0);
        assert_eq!(f.store.classify(0), RawPageLoadState::PreviewNotRequested);
        let id = f.store.begin_preview(0, "camera.dng".into());
        f.tag.request_id = id;
        f.tag.source.mtime_ticks += 1;
        f.tag.stage = RawResultStage::Info;
        f.info(RawDevelopSupport::Supported);
        assert!(matches!(
            f.store.apply_result(
                &stale,
                RawPageResult::Failed(RawError::Stale),
                11,
                &f.ctx,
                &mut f.cache,
                &mut f.dims,
                2
            ),
            RawApplyOutcome::Rejected
        ));
        assert_eq!(f.store.page(0).unwrap().source, f.tag.source);
    }

    #[test]
    fn cancel_generation_discard_and_drop_never_mutate_sibling_owner_or_channel() {
        let mut sibling = Fixture::new();
        sibling.info(RawDevelopSupport::Supported);
        let sibling_cancel = sibling.preparing(RawBrightness::None);
        for action in 0..4 {
            let mut f = Fixture::new();
            f.info(RawDevelopSupport::Supported);
            let cancel = f.preparing(RawBrightness::None);
            match action {
                0 => f.store.park(),
                1 => f.store.set_items_generation(8),
                2 => f.store.discard(0),
                _ => drop(f),
            }
            assert!(cancel.load(Ordering::Acquire));
            assert!(!sibling_cancel.load(Ordering::Acquire));
            assert!(sibling.store.accepts(&sibling.tag, 11));
            assert!(sibling.store.rx.try_recv().is_err());
        }
    }

    #[test]
    fn demand_includes_distant_cover_and_all_visible_continuous_pages_at_high() {
        let order = (0..12).collect::<Vec<_>>();
        let demand = raw_development_demand(&order, 4, &[4, 10, 0, 1, 8], |idx| idx != 5);
        for idx in [4, 10, 0, 1, 8] {
            assert_eq!(demand.get(&idx), Some(&RawPriority::High));
        }
        for idx in [3, 6] {
            assert_eq!(demand.get(&idx), Some(&RawPriority::Normal));
        }
        assert!(!demand.contains_key(&5));
        assert!(!demand.contains_key(&7));
        for prefetch_forward in [0, 1] {
            let mut keep = (3..=4 + prefetch_forward + 1).collect::<HashSet<_>>();
            keep.extend(demand.keys());
            for idx in [0, 1, 3, 4, 6, 8, 10] {
                assert!(keep.contains(&idx));
            }
        }
    }

    pub(crate) fn app_with_raw_and_jpeg() -> super::super::tests::phase_c_support::AppTestEnv {
        let mut app = setup_app_for_test();
        app.items = vec![
            GridItem::Image("camera.dng".into()),
            GridItem::Image("other.jpg".into()),
        ];
        app.visible_indices = vec![0, 1];
        let mut f = Fixture::new();
        f.info(RawDevelopSupport::Supported);
        let Some(RawPageRecord::Page(mut page)) = f.store.pages.remove(&0) else {
            unreachable!()
        };
        page.source.item_key = app.page_path_key(0).unwrap();
        app.raw_pages.pages.insert(0, RawPageRecord::Page(page));
        app
    }

    pub(crate) fn app_with_raw_and_jpeg_after_generation_change()
    -> super::super::tests::phase_c_support::AppTestEnv {
        let mut app = app_with_raw_and_jpeg();
        let transfer = app.take_fs_page_for_snapshot(0);
        app.items_generation = 1;
        app.fs_cache.set_items_generation(1);
        app.raw_pages.set_items_generation(1);
        app.restore_fs_page_from_snapshot(0, transfer);
        app
    }

    #[test]
    fn warm_half_thumbnail_cannot_be_drawn_or_used_for_a_rendition_until_preview_validation() {
        let mut app = app_with_raw_and_jpeg();
        let ctx = egui::Context::default();
        let thumb = ctx.load_texture(
            "warm_half",
            egui::ColorImage::filled([2, 2], egui::Color32::GRAY),
            egui::TextureOptions::LINEAR,
        );
        app.thumbnails = vec![
            ThumbnailState::Loaded {
                tex: thumb,
                origin: crate::thumb_loader::ThumbLoadOrigin::SourceGenerated {
                    evaluated_display_px: 2,
                },
                from_edit_preview: false,
                rendered_at_px: 2,
                source_dims: Some((12000, 8000)),
                layout_dims: None,
            },
            ThumbnailState::Pending,
        ];
        app.thumb_pixels.insert(
            0,
            Arc::new(egui::ColorImage::filled([2, 2], egui::Color32::GRAY)),
        );
        for preview in [
            RawPreviewPhase::Requested { request_id: 1 },
            RawPreviewPhase::Absent,
            RawPreviewPhase::Failed(RawError::NoUsablePreview(
                RawPreviewUnavailableReason::DecodeFailed,
            )),
            RawPreviewPhase::Failed(RawError::NoUsablePreview(
                RawPreviewUnavailableReason::OrientationMismatch,
            )),
        ] {
            app.raw_pages.page_mut(0).unwrap().preview = preview;
            assert!(!app.raw_fullscreen_fallback_allowed(0));
            assert!(app.fs_thumbnail_texture_for_display(0).is_none());
            assert!(app.ensure_passthrough_rendition(&ctx, 0).is_none());
        }
        app.raw_pages.page_mut(0).unwrap().preview = RawPreviewPhase::Done;
        app.raw_pages.page_mut(0).unwrap().stage = RawInstalledStage::PreviewShown;
        assert!(app.raw_fullscreen_fallback_allowed(0));
        assert!(app.fs_thumbnail_texture_for_display(0).is_some());
    }

    #[test]
    fn raw_edit_gate_precedes_all_mode_and_in_tool_page_mutations() {
        let mut app = app_with_raw_and_jpeg();
        app.fullscreen_idx = Some(1);
        let ctx = egui::Context::default();
        assert!(app.raw_edit_target_gate(1));
        assert!(!app.raw_edit_target_gate(0));
        assert!(app.enter_sns_split_mode(0).is_err());
        app.enter_export_crop_mode(0);
        app.enter_erase_mode(0);
        app.enter_text_mode(0);
        app.enter_conceal_mode(0);
        assert!(!app.text_mode);
        assert!(!app.erase_mode);
        assert!(!app.conceal_mode);
        assert!(app.sns_split.is_none());
        assert_eq!(app.fullscreen_idx, Some(1));
        app.fullscreen_idx = Some(0);
        app.enter_local_adjust_mode();
        assert!(!app.local_adjust_mode);
        assert_eq!(app.fullscreen_idx, Some(0));
        app.fullscreen_idx = Some(1);
        app.switch_erase_target_in_spread(&ctx, 0);
        app.switch_conceal_target_in_spread(0);
        app.switch_text_target_in_spread(0);
        app.switch_local_adjust_target_in_spread(0);
        assert_eq!(app.fullscreen_idx, Some(1));
        assert!(app.raw_edit_target_gate(1));
        app.raw_pages.page_mut(0).unwrap().stage = RawInstalledStage::Developed;
        assert!(app.raw_edit_target_gate(0));
    }

    #[test]
    fn snapshot_moves_entry_and_owner_atomically_and_rejects_old_generation_and_wrong_source() {
        let mut app = app_with_raw_and_jpeg();
        app.fs_cache.insert(
            0,
            FsCacheEntry::RawPreview {
                preview: None,
                developed_dims: [12000, 8000],
                load_seq: 1,
            },
        );
        app.raw_pages.page_mut(0).unwrap().preview = RawPreviewPhase::Absent;
        app.raw_pages.page_mut(0).unwrap().stage = RawInstalledStage::PreviewAbsent;
        let transfer = app.take_fs_page_for_snapshot(0);
        assert!(app.raw_pages.page(0).is_none());
        assert!(!app.fs_cache.contains_key(&0));
        let generation = app.items_generation + 1;
        app.items_generation = generation;
        app.fs_cache.set_items_generation(generation);
        app.raw_pages.set_items_generation(generation);
        app.restore_fs_page_from_snapshot(0, transfer);
        assert!(app.raw_pages.page(0).is_some());
        assert!(app.fs_cache.contains_key(&0));
        let transfer = app.take_fs_page_for_snapshot(0);
        app.items[0] = GridItem::Image("different.dng".into());
        app.restore_fs_page_from_snapshot(0, transfer);
        assert!(app.raw_pages.page(0).is_none());
        assert!(!app.fs_cache.contains_key(&0));
    }

    #[test]
    fn brightness_and_overwrite_reject_running_raw_ai_but_keep_unrelated_jpeg_completion() {
        let mut app = app_with_raw_and_jpeg();
        app.settings.retained_final_ai_cache_max_entries = 10;
        app.settings.retained_final_ai_cache_max_mib = 10;
        let key = |app: &App, idx| FinalAiKey {
            edit_key: app.current_edit_result_key(idx),
            color_ai_hash: 42,
            bg: 0,
        };
        let raw = app
            .retained_final_ai_key_for(0, key(&app, 0), [6, 4])
            .unwrap();
        let jpeg = app
            .retained_final_ai_key_for(1, key(&app, 1), [6, 4])
            .unwrap();
        let epoch = app.retained_final_ai_epoch;
        let pixels = Arc::new(egui::ColorImage::filled([6, 4], egui::Color32::WHITE));
        assert!(app.insert_retained_final_ai_with_key(
            0,
            raw.clone(),
            epoch,
            Arc::clone(&pixels),
            false
        ));
        app.settings.raw_brightness = RawBrightness::None;
        app.raw_brightness_changed();
        assert_eq!(epoch, app.retained_final_ai_epoch);
        assert!(!app.insert_retained_final_ai_with_key(
            0,
            raw.clone(),
            epoch,
            Arc::clone(&pixels),
            false
        ));
        assert!(app.insert_retained_final_ai_with_key(
            1,
            jpeg.clone(),
            epoch,
            Arc::clone(&pixels),
            false
        ));
        let new_raw = app
            .retained_final_ai_key_for(0, key(&app, 0), [6, 4])
            .unwrap();
        assert_ne!(new_raw, raw);
        assert!(app.insert_retained_final_ai_with_key(
            0,
            new_raw.clone(),
            epoch,
            Arc::clone(&pixels),
            false
        ));
        app.invalidate_raw_source_for_idx(0);
        app.raw_pages.page_mut(0).unwrap().source.mtime_ticks += 1;
        let overwritten = app
            .retained_final_ai_key_for(0, key(&app, 0), [6, 4])
            .unwrap();
        assert_ne!(overwritten, new_raw);
        assert!(!app.insert_retained_final_ai_with_key(
            0,
            new_raw,
            epoch,
            Arc::clone(&pixels),
            false
        ));
        assert!(app.insert_retained_final_ai_with_key(1, jpeg, epoch, pixels, false));
        assert_eq!(epoch, app.retained_final_ai_epoch);
    }

    #[cfg(windows)]
    #[test]
    fn brightness_transaction_reaches_parked_owner_without_changing_jpeg_context() {
        let mut app = app_with_raw_and_jpeg();
        let original = app.projected_viewer_context_id();
        let mut f = Fixture::new();
        f.info(RawDevelopSupport::Supported);
        f.developed();
        let Some(RawPageRecord::Page(page)) = f.store.pages.remove(&0) else {
            unreachable!()
        };
        let parked = app.build_window_context_for_test(92001, |context| {
            context.items = vec![GridItem::Image("camera.dng".into())];
            let mut page = page;
            page.source.item_key = context.page_path_key(0).unwrap();
            context.raw_pages.pages.insert(0, RawPageRecord::Page(page));
            context.fs_cache.insert(0, f.cache.remove(&0).unwrap());
        });
        let epoch = app.retained_final_ai_epoch;
        app.settings.raw_brightness = RawBrightness::None;
        app.raw_brightness_changed();
        assert_eq!(app.projected_viewer_context_id(), original);
        assert_eq!(app.retained_final_ai_epoch, epoch);
        app.with_viewer_context(parked, |context| {
            assert_eq!(
                context.raw_pages.classify(0),
                RawPageLoadState::PreviewNotRequested
            );
            assert!(!context.fs_cache.contains_key(&0));
            assert!(matches!(
                *context.raw_pages.page(0).unwrap().develop.lock().unwrap(),
                RawDevelopPhase::Idle
            ));
        })
        .unwrap();
        assert!(app.raw_pages.page(0).is_some());
        assert_eq!(app.source_dims_for_idx(0), Some((12000.0, 8000.0)));
    }

    #[test]
    fn raw_develop_window_survives_zero_or_one_forward_prefetch() {
        for forward in [0, 1] {
            let mut app = setup_app_for_test();
            app.items = (0..8)
                .map(|idx| GridItem::Image(format!("page-{idx}.dng").into()))
                .collect();
            app.visible_indices = (0..8).collect();
            app.fullscreen_idx = Some(2);
            app.settings.prefetch_forward = forward;
            app.settings.prefetch_back = 0;
            for idx in 0..8 {
                let mut f = Fixture::new();
                f.info(RawDevelopSupport::Supported);
                f.developed();
                let Some(RawPageRecord::Page(mut page)) = f.store.pages.remove(&0) else {
                    unreachable!()
                };
                page.source.item_key = app.page_path_key(idx).unwrap();
                app.raw_pages.pages.insert(idx, RawPageRecord::Page(page));
                app.fs_cache.insert(idx, f.cache.remove(&0).unwrap());
            }
            app.update_prefetch_window(2);
            for idx in [1, 2, 3, 4] {
                assert!(app.fs_cache.contains_key(&idx));
                assert!(app.raw_pages.page(idx).is_some());
            }
            assert!(app.raw_pages.page(7).is_none());
            assert!(!app.fs_cache.contains_key(&7));
            assert_eq!(app.raw_pages.demand.get(&2), Some(&RawPriority::High));
            assert_eq!(app.raw_pages.demand.get(&4), Some(&RawPriority::Normal));
        }
    }

    #[test]
    fn worker_fingerprint_detects_overwrite_before_open_and_after_processing() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("camera.dng");
        std::fs::write(&path, [1; 3]).unwrap();
        let identity = RawSourceIdentity::read("camera.dng".into(), path.clone()).unwrap();
        assert_eq!(identity.validate(), Ok(()));
        let source = crate::raw::RawOwnedSource::Validated {
            source: Box::new(crate::raw::RawOwnedSource::Path(path.clone())),
            fingerprint: identity.fingerprint(),
        };
        std::fs::write(&path, [2; 4]).unwrap();
        assert_eq!(identity.validate(), Err(RawError::Stale));
        assert_eq!(source.validate(), Err(RawError::Stale));
        let fresh = RawSourceIdentity::read("camera.dng".into(), path.clone()).unwrap();
        std::fs::remove_file(path).unwrap();
        assert_eq!(fresh.validate(), Err(RawError::Stale));
    }

    #[test]
    fn submitted_park_completion_is_rejected_and_new_request_can_be_promoted() {
        let mut f = Fixture::new();
        f.info(RawDevelopSupport::Supported);
        f.preparing(RawBrightness::None);
        let executor = crate::raw::RawDevelopExecutor::new(1).unwrap();
        let (started_tx, started_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        let blocker = executor.block_one_slot_for_test(started_tx, release_rx);
        started_rx
            .recv_timeout(std::time::Duration::from_secs(5))
            .unwrap();
        let (tx, _rx) = mpsc::channel();
        let ticket = Arc::new(executor.submit_thumbnail_half(
            crate::raw::RawOwnedSource::Bytes(Arc::from([0u8; 1])),
            RawPriority::Normal,
            tx,
        ));
        f.store
            .page(0)
            .unwrap()
            .develop
            .lock()
            .unwrap()
            .publish(f.tag.request_id, Arc::clone(&ticket));
        assert_eq!(ticket.state(), Some(crate::raw::RawTicketState::Queued));
        let old = f.tag.clone();
        f.store.park();
        assert!(!f.store.accepts(&old, 11));
        assert!(matches!(
            *f.store.page(0).unwrap().develop.lock().unwrap(),
            RawDevelopPhase::Idle
        ));
        f.preparing(RawBrightness::None);
        f.store.page(0).unwrap().develop.lock().unwrap().promote();
        let (tx, _rx) = mpsc::channel();
        let late_ticket = Arc::new(executor.submit_thumbnail_half(
            crate::raw::RawOwnedSource::Bytes(Arc::from([0u8; 1])),
            RawPriority::Normal,
            tx,
        ));
        f.store
            .page(0)
            .unwrap()
            .develop
            .lock()
            .unwrap()
            .publish(old.request_id, Arc::clone(&late_ticket));
        assert_ne!(
            late_ticket.state(),
            Some(crate::raw::RawTicketState::Queued)
        );
        f.store.discard(0);
        release_tx.send(()).unwrap();
        drop(blocker);
    }

    #[test]
    fn color_and_lut_preview_only_present_faithful_renditions_in_single_spread_and_continuous() {
        for lut in [false, true] {
            for mode in 0..3 {
                let mut app = app_with_raw_and_jpeg();
                let ctx = egui::Context::default();
                let transfer = app.take_fs_page_for_snapshot(0);
                app.items_generation = 1;
                app.fs_cache.set_items_generation(1);
                app.raw_pages.set_items_generation(1);
                app.restore_fs_page_from_snapshot(0, transfer);
                app.fullscreen_idx = Some(0);
                if mode == 1 {
                    app.spread_mode = crate::settings::SpreadMode::Ltr;
                }
                if mode == 2 {
                    app.reading_flow = crate::settings::ReadingFlow::Vertical;
                }
                if lut {
                    let builtin = crate::creative_lut::BuiltinCreativeLut::WarmFilm;
                    app.creative_lut_library =
                        crate::creative_lut::CreativeLutLibrary::from_builtin_for_test(builtin);
                    app.settings.global_preset.creative_lut =
                        crate::creative_lut::CreativeLutSelection {
                            id: Some(crate::creative_lut::BuiltinCreativeLut::WarmFilm.id()),
                            strength: 1.0,
                        };
                } else {
                    app.settings.global_preset.colorize.mode =
                        crate::colorize::ColorizeMode::AllImages;
                }
                assert!(app.colorize_display_requires_final_effect(0));
                let pixels = Arc::new(egui::ColorImage::filled(
                    [3, 2],
                    egui::Color32::from_rgb(50, 50, 50),
                ));
                let preview = ctx.load_texture(
                    "camera_preview",
                    pixels.as_ref().clone(),
                    egui::TextureOptions::LINEAR,
                );
                let thumbnail = ctx.load_texture(
                    "catalog_preview",
                    pixels.as_ref().clone(),
                    egui::TextureOptions::LINEAR,
                );
                app.thumbnails = vec![
                    ThumbnailState::Loaded {
                        tex: thumbnail,
                        origin: crate::thumb_loader::ThumbLoadOrigin::SourceGenerated {
                            evaluated_display_px: 3,
                        },
                        from_edit_preview: false,
                        rendered_at_px: 3,
                        source_dims: Some((12000, 8000)),
                        layout_dims: None,
                    },
                    ThumbnailState::Pending,
                ];
                app.thumb_pixels.insert(0, Arc::clone(&pixels));
                app.fs_cache.insert(
                    0,
                    FsCacheEntry::RawPreview {
                        preview: Some(crate::fs_animation::RawPreviewTexture {
                            tex: preview.clone(),
                            pixels,
                        }),
                        developed_dims: [12000, 8000],
                        load_seq: 1,
                    },
                );
                let page = app.raw_pages.page_mut(0).unwrap();
                page.preview = RawPreviewPhase::Done;
                page.stage = RawInstalledStage::PreviewShown;
                assert!(app.current_raw_source_pixels(0).is_none());
                assert!(app.resolve_fs_display_tex(0, true).is_none());
                assert_eq!(app.raw_navigation_display_ready(0, false), Some(false));
                app.fs_nav_locked_gen = Some(0);
                app.poll_fs_nav_lock(&ctx);
                assert_eq!(app.fs_nav_locked_gen, Some(0));
                let rendition = app.resolve_fs_processed_texture(&ctx, 0, false).unwrap();
                assert_ne!(rendition.id(), preview.id());
                assert!(app.is_passthrough_rendition_texture(&rendition));
                assert_eq!(
                    app.resolve_fs_display_tex(0, true).unwrap().id(),
                    rendition.id()
                );
                assert_eq!(app.raw_navigation_display_ready(0, false), Some(true));
                app.poll_fs_nav_lock(&ctx);
                assert!(app.fs_nav_locked_gen.is_none());
                assert!(app.current_raw_source_pixels(0).is_none());
                assert!(!app.current_final_composite_is_complete(0));
                app.settings.raw_brightness = RawBrightness::None;
                app.raw_brightness_changed();
                assert_eq!(app.raw_pages.classify(0), RawPageLoadState::PreviewShown);
                assert!(app.current_raw_source_pixels(0).is_none());
                assert_ne!(
                    app.resolve_fs_processed_texture(&ctx, 0, false)
                        .unwrap()
                        .id(),
                    preview.id()
                );
            }
        }
    }

    #[test]
    fn terminal_raw_target_is_load_failed_even_with_a_warm_thumbnail() {
        let mut app = app_with_raw_and_jpeg();
        let page = app.raw_pages.page_mut(0).unwrap();
        page.preview = RawPreviewPhase::Absent;
        page.stage = RawInstalledStage::PreviewAbsent;
        *page.develop.lock().unwrap() = RawDevelopPhase::Blocked(RawDevelopBlocked::Unsupported);
        assert_eq!(app.fs_page_load_state(0), FsPageLoadState::LoadFailed);
        assert_eq!(app.ensure_fs_page_load(0), FsPageLoadState::LoadFailed);
        assert_eq!(app.raw_navigation_display_ready(0, false), Some(false));
        assert!(!app.raw_fullscreen_fallback_allowed(0));
        assert!(app.fs_pending.is_empty());
    }

    #[test]
    fn blocked_development_keeps_valid_preview_displayable_but_ends_comparison_wait() {
        for failure in [
            RawDevelopBlocked::Unsupported,
            RawDevelopBlocked::Failed {
                error: RawError::OutOfMemory,
                brightness: RawBrightness::MatchPreview,
            },
        ] {
            let mut app = app_with_raw_and_jpeg();
            let ctx = egui::Context::default();
            let page = app.raw_pages.page_mut(0).unwrap();
            page.preview = RawPreviewPhase::Done;
            page.stage = RawInstalledStage::PreviewShown;
            *page.develop.lock().unwrap() = RawDevelopPhase::Blocked(failure);
            assert!(matches!(
                app.fs_page_load_state(0),
                FsPageLoadState::DisplayReady(_)
            ));
            assert!(app.raw_fullscreen_fallback_allowed(0));
            assert!(app.raw_development_unavailable(0));
            app.compare_pin_load_pending = Some(ComparePinLoadPending {
                source_idx: 0,
                source_key: app.metadata_cache_key(0).unwrap(),
            });
            app.poll_compare_pin_load_pending(&ctx);
            assert!(app.compare_pin_load_pending.is_none());
            assert!(app.fs_pending.is_empty());
            assert!(!app.raw_development_unavailable(1));
        }
    }

    #[test]
    fn jpeg_anchor_cannot_enter_tools_for_undeveloped_raw_spread_partner() {
        let mut app = app_with_raw_and_jpeg();
        let ctx = egui::Context::default();
        app.raw_pages.page_mut(0).unwrap().developed_dims = Some([8000, 12000]);
        let pixels = egui::ColorImage::filled([2, 3], egui::Color32::GRAY);
        let tex = ctx.load_texture("jpeg_partner", pixels.clone(), egui::TextureOptions::LINEAR);
        app.fs_cache.insert(
            1,
            FsCacheEntry::Static {
                tex,
                pixels: Arc::new(pixels),
                source_dims: Some([8000, 12000]),
                load_seq: 1,
                animation: StaticAnimationState::Still,
            },
        );
        app.spread_mode = crate::settings::SpreadMode::Ltr;
        app.fullscreen_idx = Some(1);
        assert_eq!(app.plan_page_edit_pivot(1).0, 0);
        assert!(app.enter_sns_split_mode(1).is_err());
        app.enter_export_crop_mode(1);
        app.enter_erase_mode(1);
        app.enter_text_mode(1);
        app.enter_conceal_mode(1);
        assert_eq!(app.fullscreen_idx, Some(1));
        assert_eq!(app.spread_mode, crate::settings::SpreadMode::Ltr);
        assert!(!app.text_mode && !app.erase_mode && !app.conceal_mode && !app.export_crop_mode);
        assert!(app.sns_split.is_none());
    }

    #[test]
    fn developed_brightness_transaction_holds_processed_pixels_in_every_display_mode() {
        for lut in [false, true] {
            for mode in 0..3 {
                let mut app = app_with_raw_and_jpeg();
                let ctx = egui::Context::default();
                app.fullscreen_idx = Some(0);
                if mode == 1 {
                    app.spread_mode = crate::settings::SpreadMode::Ltr;
                }
                if mode == 2 {
                    app.reading_flow = crate::settings::ReadingFlow::Vertical;
                    app.fs_vertical_cache_keep_set = HashSet::from([0, 1]);
                }
                if lut {
                    let builtin = crate::creative_lut::BuiltinCreativeLut::WarmFilm;
                    app.creative_lut_library =
                        crate::creative_lut::CreativeLutLibrary::from_builtin_for_test(builtin);
                    app.settings.global_preset.creative_lut =
                        crate::creative_lut::CreativeLutSelection {
                            id: Some(builtin.id()),
                            strength: 1.0,
                        };
                } else {
                    app.settings.global_preset.colorize.mode =
                        crate::colorize::ColorizeMode::AllImages;
                }
                app.raw_pages.page_mut(0).unwrap().developed_dims = Some([8000, 12000]);
                let mut presented = None;
                for idx in 0..2 {
                    let raw = Arc::new(egui::ColorImage::filled([2, 3], egui::Color32::RED));
                    let source = ctx.load_texture(
                        format!("source_{idx}"),
                        raw.as_ref().clone(),
                        egui::TextureOptions::LINEAR,
                    );
                    app.fs_cache.insert(
                        idx,
                        FsCacheEntry::Static {
                            tex: source.clone(),
                            pixels: Arc::clone(&raw),
                            source_dims: Some([8000, 12000]),
                            load_seq: 1,
                            animation: StaticAnimationState::Still,
                        },
                    );
                    let key = app.current_edit_result_key(idx);
                    app.edit_result_cache.insert(
                        key,
                        EditResultEntry {
                            pixels: raw,
                            texture: Some(source),
                        },
                    );
                    let processed =
                        Arc::new(egui::ColorImage::filled([2, 3], egui::Color32::GREEN));
                    let final_tex = ctx.load_texture(
                        format!("processed_{idx}"),
                        processed.as_ref().clone(),
                        egui::TextureOptions::LINEAR,
                    );
                    let final_key =
                        app.final_composite_key_for_pixels(key, [2, 3], app.effective_params(idx));
                    app.final_composite_cache.insert(
                        final_key,
                        FinalCompositeEntry {
                            pixels: processed,
                            texture: final_tex.clone(),
                            complete: true,
                        },
                    );
                    if idx == 0 {
                        presented = Some(final_tex.id());
                    }
                }
                let page = app.raw_pages.page_mut(0).unwrap();
                page.stage = RawInstalledStage::Developed;
                *page.develop.lock().unwrap() = RawDevelopPhase::Done;
                assert_eq!(
                    app.resolve_fs_display_tex(0, false).unwrap().id(),
                    presented.unwrap()
                );
                app.settings.raw_brightness = RawBrightness::None;
                app.raw_brightness_changed();
                assert!(app.fs_cache.get(&0).is_none());
                assert!(app.fs_cache.get(&1).is_some());
                if mode == 2 {
                    assert_eq!(
                        app.continuous_page_transition_texture(0)
                            .unwrap()
                            .source_texture()
                            .id(),
                        presented.unwrap()
                    );
                } else {
                    assert_eq!(
                        app.fs_holdover_tex.as_ref().unwrap().primary_texture_id(),
                        presented.unwrap()
                    );
                }
            }
        }
    }
}

#[derive(Clone, Copy)]
struct RawPanoramaIntent {
    xmp: bool,
    base_only: bool,
    approved: u64,
}

fn raw_developed_raster(
    image: image::DynamicImage,
    intent: Option<RawPanoramaIntent>,
) -> FsLoadResult {
    let dims = [image.width() as usize, image.height() as usize];
    let pixels = dims[0] as u64 * dims[1] as u64;
    let tee = dims.iter().any(|&d| d > MAX_TEXTURE_DIM)
        && intent.is_some_and(|intent| {
            let aspect = dims[0] as f32 / dims[1].max(1) as f32;
            !intent.base_only
                && (intent.xmp
                    || (crate::panorama::ASPECT_LOW..=crate::panorama::ASPECT_HIGH)
                        .contains(&aspect))
                && (pixels <= crate::panorama::PANO_SETTLE_MAX_PIXELS
                    || pixels <= intent.approved.saturating_mul(125) / 100)
        });
    if tee {
        let rgba = image.into_rgba8();
        let (w, h) = (rgba.width(), rgba.height());
        let scale = MAX_TEXTURE_DIM as f64 / w.max(h) as f64;
        let small = crate::fast_resize::resize_rgba8_exact(
            &rgba,
            ((w as f64 * scale).round() as u32).max(1),
            ((h as f64 * scale).round() as u32).max(1),
            crate::fast_resize::Quality::Bilinear,
        );
        let ci = egui::ColorImage::from_rgba_unmultiplied(
            [small.width() as usize, small.height() as usize],
            small.as_raw(),
        );
        FsLoadResult::StaticPanorama {
            ci,
            source_dims: dims,
            high_res: crate::panorama::HighResSource::Decoded {
                rgba: Arc::new(rgba.into_raw()),
                w,
                h,
            },
        }
    } else {
        let raster = CanonicalStaticImage {
            image,
            source_dims: dims,
            animation: CanonicalStaticAnimation::Still,
        }
        .into_gpu_raster();
        FsLoadResult::Static {
            ci: raster.pixels,
            source_dims: dims,
            animation: StaticAnimationState::Still,
        }
    }
}

impl App {
    pub(super) fn take_fs_page_for_snapshot(&mut self, idx: usize) -> FsPageTransfer {
        let raw = match self.raw_pages.pages.remove(&idx) {
            Some(RawPageRecord::Page(mut page)) => {
                page.develop.lock().unwrap().cancel();
                if matches!(page.preview, RawPreviewPhase::Requested { .. }) {
                    page.preview = RawPreviewPhase::NotRequested;
                }
                Some(page)
            }
            _ => None,
        };
        let entry = self.fs_cache.remove(&idx);
        if let Some(pending) = self.fs_pending.remove(&idx) {
            pending.cancel();
        }
        self.fs_early_dims.remove(&idx);
        self.fs_margin_bbox_cache.remove(&idx);
        self.fs_upload_backlog.retain(|entry| entry.idx != idx);
        FsPageTransfer { entry, raw }
    }

    pub(super) fn restore_fs_page_from_snapshot(&mut self, idx: usize, transfer: FsPageTransfer) {
        if let Some(page) = transfer.raw {
            // Exact snapshot remapping proves the item; the physical fingerprint
            // remains the worker-validated source, never a UI-thread stat.
            if self.page_path_key(idx).as_deref() != Some(page.source.item_key.as_str()) {
                return;
            }
            self.raw_pages.pages.insert(idx, RawPageRecord::Page(page));
        } else if self.is_raw_page(idx) {
            return;
        }
        if let Some(entry) = transfer.entry {
            self.fs_cache.insert(idx, entry);
        }
    }

    pub(super) fn apply_raw_fs_result(
        &mut self,
        ctx: &egui::Context,
        tag: RawResultTag,
        result: RawPageResult,
        seq: u64,
    ) {
        if self.page_path_key(tag.idx).as_deref() != Some(tag.source.item_key.as_str()) {
            return;
        }
        let context = self.fs_page_load_context_serial();
        if !self.raw_pages.resolve_source(&tag, context) {
            return;
        }
        if matches!(result, RawPageResult::Develop(_)) {
            self.capture_final_effect_source_reload_holdover(tag.idx);
        }
        let outcome = self.raw_pages.apply_result(
            &tag,
            result,
            context,
            ctx,
            &mut self.fs_cache,
            &mut self.fs_early_dims,
            seq,
        );
        match outcome {
            RawApplyOutcome::Updated => {
                if let Some([w, h]) = self
                    .raw_pages
                    .page(tag.idx)
                    .and_then(|page| page.developed_dims)
                {
                    self.record_page_dims_for_spread(tag.idx, (w as u32, h as u32));
                }
            }
            RawApplyOutcome::Stale => {
                self.invalidate_raw_source_for_idx(tag.idx);
                self.discard_fs_page(tag.idx);
            }
            RawApplyOutcome::Installed {
                developed,
                high_res,
            } => {
                if self.items.get(tag.idx).is_some() {
                    if let Some(entry) = self.fs_cache.get(&tag.idx) {
                        let tex = match entry {
                            FsCacheEntry::Static { tex, .. } => Some(tex),
                            FsCacheEntry::RawPreview { preview, .. } => {
                                preview.as_ref().map(|p| &p.tex)
                            }
                            _ => None,
                        };
                        if let Some(tex) = tex {
                            self.bind_display_texture_identity(tex, self.item_id(tag.idx));
                        }
                    }
                }
                self.bump_input_generation_for_fs_cache_reload(tag.idx);
                self.record_fs_cache_page_dims_for_spread(tag.idx);
                self.fs_margin_bbox_cache.remove(&tag.idx);
                if developed {
                    if let Some(FsCacheEntry::Static { pixels, .. }) = self.fs_cache.get(&tag.idx) {
                        let pixels = Arc::clone(pixels);
                        self.apply_sync_adjustment(ctx, tag.idx, &pixels);
                    }
                    if let Some(high_res) = high_res
                        && let Some(key) = self.metadata_cache_key(tag.idx)
                    {
                        if self.fullscreen_idx == Some(tag.idx) {
                            self.pano_high_res_source.insert(key.clone(), high_res);
                        }
                        let dims = self
                            .raw_pages
                            .page(tag.idx)
                            .unwrap()
                            .developed_dims
                            .unwrap();
                        let state = if dims[0] as u64 * dims[1] as u64
                            <= crate::panorama::PANO_SETTLE_MAX_PIXELS
                        {
                            crate::panorama::PanoramaQualityState::SettleReady
                        } else {
                            crate::panorama::PanoramaQualityState::SettleApproved
                        };
                        self.pano_quality_state.insert(key, state);
                    }
                    self.auto_apply_saved_mask(ctx, tag.idx);
                    self.maybe_update_pano_quality_state_from_static(tag.idx);
                }
            }
            _ => {}
        }
    }

    pub(crate) fn discard_fs_page(&mut self, idx: usize) {
        self.raw_pages.discard(idx);
        self.fs_cache.remove(&idx);
        if let Some(pending) = self.fs_pending.remove(&idx) {
            pending.cancel();
        }
        self.fs_early_dims.remove(&idx);
        self.fs_upload_backlog.retain(|entry| entry.idx != idx);
    }

    pub(crate) fn invalidate_raw_source_for_idx(&mut self, idx: usize) {
        // Reserve a new identity at the transaction boundary, even before demand
        // starts replacement work. Preview requests retain their independent ID.
        self.raw_pages.next_id();
        self.capture_final_effect_source_reload_holdover(idx);
        self.bump_input_generation_for_fs_cache_reload(idx);
        self.erase_base_cache.remove(&idx);
        self.erase_base_tex_cache.remove(&idx);
        self.conceal_base_cache.remove(&idx);
        if matches!(self.fs_cache.get(&idx), Some(FsCacheEntry::Static { .. })) {
            self.fs_cache.remove(&idx);
        }
        if let Some(key) = self.metadata_cache_key(idx) {
            self.pano_high_res_source.remove(&key);
            self.pano_quality_state.remove(&key);
            if let Some(request) = self.pano_high_res_pending.remove(&key) {
                request.cancel.store(true, Ordering::Relaxed);
            }
            self.pano_high_res_failed.remove(&key);
        }
        self.fs_upload_backlog.retain(|entry| {
            entry.idx != idx
                || !matches!(
                    entry.result,
                    FsLoadResult::Raw {
                        tag: RawResultTag {
                            stage: RawResultStage::Develop(_),
                            ..
                        },
                        ..
                    }
                )
        });
    }

    fn raw_brightness_changed_here(&mut self) {
        let indices: Vec<_> = self.raw_pages.pages.keys().copied().collect();
        for idx in indices {
            // Capture the presented unit before removing its input or derived pixels.
            self.invalidate_raw_source_for_idx(idx);
            if let Some(page) = self.raw_pages.page_mut(idx) {
                page.brightness_changed();
            }
        }
    }

    pub(crate) fn raw_brightness_changed(&mut self) {
        self.raw_brightness_changed_here();
        #[cfg(windows)]
        for context in self.other_viewer_context_ids() {
            self.with_viewer_context(context, |app| app.raw_brightness_changed_here())
                .expect(
                    "enumerated viewer context must remain available during RAW source transaction",
                );
        }
    }

    pub(crate) fn reconcile_raw_demand(
        &mut self,
        current: usize,
        displayed: &[usize],
    ) -> HashSet<usize> {
        let order = self.get_still_image_indices();
        let mut demand =
            raw_development_demand(&order, current, displayed, |idx| self.is_raw_page(idx));
        if let Some(pending) = &self.compare_pin_load_pending
            && self.is_raw_page(pending.source_idx)
        {
            demand.insert(pending.source_idx, RawPriority::High);
        }
        for (&idx, record) in self.raw_pages.pages.iter() {
            if let RawPageRecord::Page(page) = record {
                let mut phase = page.develop.lock().unwrap();
                match demand.get(&idx) {
                    None => phase.cancel(),
                    Some(RawPriority::High) => phase.promote(),
                    _ => {}
                }
            }
        }
        self.raw_pages.demand = demand.clone();
        for &idx in demand.keys() {
            if self.raw_pages.classify(idx) == RawPageLoadState::PreviewNotRequested {
                self.ensure_fs_page_load(idx);
            }
        }
        demand.into_keys().collect()
    }

    pub(super) fn start_demanded_raw_development(&mut self, ctx: &egui::Context) {
        let Some(current) = self.fullscreen_idx else {
            return;
        };
        let decision = self.fs_page_turn_decision_for_frame(ctx, current);
        let target = self
            .fs_navigation_rendition_target_pages(current)
            .unwrap_or_default();
        let mut demand = self
            .raw_pages
            .demand
            .iter()
            .map(|(&idx, &priority)| (idx, priority))
            .collect::<Vec<_>>();
        let order = self.get_still_image_indices();
        let pos = order.iter().position(|&idx| idx == current).unwrap_or(0);
        let mut submit_order = vec![current];
        submit_order.extend(interleaved_prefetch_targets(
            &order,
            pos,
            order.len(),
            RAW_DEVELOP_FORWARD,
            RAW_DEVELOP_BACK,
        ));
        demand.sort_by_key(|(idx, priority)| {
            (
                *priority != RawPriority::High,
                submit_order
                    .iter()
                    .position(|i| i == idx)
                    .unwrap_or(usize::MAX),
                *idx,
            )
        });
        for (idx, priority) in demand {
            if decision.admits_backlog_upload(target.contains(&idx)) {
                self.start_raw_develop(ctx, idx, priority);
            }
        }
    }

    pub(crate) fn is_raw_page(&self, idx: usize) -> bool {
        match self.items.get(idx) {
            Some(GridItem::Image(path)) => crate::raw_format::is_raw_path(path),
            Some(GridItem::ZipImage { entry_name, .. }) => {
                crate::raw_format::is_raw_path(Path::new(entry_name))
            }
            _ => false,
        }
    }

    pub(crate) fn raw_page_load_state(&self, idx: usize) -> Option<RawPageLoadState> {
        self.is_raw_page(idx).then(|| self.raw_pages.classify(idx))
    }

    pub(crate) fn raw_fullscreen_fallback_allowed(&self, idx: usize) -> bool {
        !self.is_raw_page(idx) || self.raw_pages.fallback_allowed(idx)
    }

    pub(crate) fn raw_edit_target_gate(&self, idx: usize) -> bool {
        !self.is_raw_page(idx) || self.raw_pages.classify(idx) == RawPageLoadState::Developed
    }

    pub(crate) fn raw_development_blocked(&self, idx: usize) -> bool {
        self.is_raw_page(idx)
            && self.raw_pages.page(idx).is_some_and(|page| {
                matches!(*page.develop.lock().unwrap(), RawDevelopPhase::Blocked(_))
            })
    }

    pub(crate) fn raw_development_unavailable(&self, idx: usize) -> bool {
        self.is_raw_page(idx)
            && (self.raw_pages.classify(idx).terminal() || self.raw_development_blocked(idx))
    }

    pub(crate) fn raw_edit_target_entry_allowed(&mut self, idx: usize) -> bool {
        if self.raw_edit_target_gate(idx) {
            return true;
        }
        let unavailable = self.raw_development_blocked(idx);
        self.set_fullscreen_nav_noop(if unavailable {
            crate::ui_fullscreen::FsNavNoOpReason::RawDevelopmentUnavailable
        } else {
            crate::ui_fullscreen::FsNavNoOpReason::RawDevelopmentPending
        });
        false
    }

    pub(crate) fn fs_page_layout_uses_source_size(
        &self,
        idx: usize,
        texture: &egui::TextureHandle,
    ) -> bool {
        self.is_raw_page(idx)
            || self.is_passthrough_rendition_texture(texture)
            || matches!(self.items.get(idx), Some(GridItem::PdfPage { .. }))
    }

    pub(super) fn start_raw_develop_for_compare(&mut self, ctx: &egui::Context, idx: usize) {
        self.start_raw_develop(ctx, idx, RawPriority::High);
    }

    pub(super) fn start_raw_preview(
        &mut self,
        idx: usize,
        path: PathBuf,
        entry: Option<String>,
        purpose: FsLoadPurpose,
        contract: FsPageLoadContract,
    ) {
        if self.raw_pages.classify(idx) != RawPageLoadState::PreviewNotRequested {
            return;
        }
        let item_key = self.page_path_key(idx).expect("RAW page identity");
        let expected = self.raw_pages.page(idx).map(|page| page.source.clone());
        let request_id = self.raw_pages.begin_preview(idx, item_key.clone());
        let context = self.fs_page_load_context_serial();
        let generation = self.items_generation;
        let priority = if self.page_is_displayed_now(idx) {
            FsPageLoadPriority::High
        } else {
            FsPageLoadPriority::Normal
        };
        let ticket = self.fs_page_load_scheduler.request(
            context,
            idx,
            priority,
            contract,
            Some(item_key.clone()),
            self.input_seq,
        );
        let cancel = ticket.cancel_token();
        let waiter = ticket.waiter();
        let (tx, rx) = mpsc::channel();
        self.fs_pending.insert(
            idx,
            FsPendingValue::scheduled(ticket, rx, self.input_seq, purpose),
        );
        let provenance = self.relative_page_provenance_for_idx(idx);
        std::thread::spawn(move || {
            let Some(_permit) = waiter.acquire_cancellable() else {
                return;
            };
            let identity = match expected.map_or_else(
                || RawSourceIdentity::read(item_key, path.clone()),
                |identity| identity.validate().map(|()| identity),
            ) {
                Ok(identity) => identity,
                Err(error) => {
                    let _ = tx.send(FsLoadResult::RawSourceFailed {
                        context,
                        generation,
                        request_id,
                        error,
                    });
                    return;
                }
            };
            let mut tag = RawResultTag {
                context,
                generation,
                idx,
                request_id,
                source: identity.clone(),
                stage: RawResultStage::Info,
            };
            let source = (|| {
                identity.validate()?;
                let verified = provenance
                    .as_ref()
                    .map(|p| p.read_verified())
                    .transpose()
                    .map_err(|e| RawError::Io(e.to_string()))?;
                let canonical = match entry.as_deref() {
                    Some(entry_name) => CanonicalImageSource::ArchiveEntry {
                        archive_path: &path,
                        entry_name,
                    },
                    None => CanonicalImageSource::File {
                        path: &path,
                        verified_bytes: verified.as_deref(),
                    },
                };
                let source = resolve_canonical_source(canonical, Some(&cancel))
                    .map_err(|e| RawError::Io(format!("{e:?}")))?
                    .with_raw_fingerprint(identity.fingerprint());
                let info = source.raw_info()?;
                let _ = tx.send(FsLoadResult::Raw {
                    tag: tag.clone(),
                    result: RawPageResult::Info {
                        developed_dims: info.developed_dims.map(|d| d as usize),
                        support: info.develop_support,
                    },
                });
                match crate::canonical_image_loader::decode_canonical_resolved(
                    source,
                    CanonicalDecodeOptions::fullscreen_cancellable(
                        AnimationPolicy::FirstFrameOnly,
                        &cancel,
                        RawStage::Preview,
                    ),
                ) {
                    Ok(CanonicalImageDecode::RawPreview { preview, .. }) => {
                        let pixels = preview.map(|preview| {
                            CanonicalStaticImage {
                                source_dims: [
                                    preview.image.width() as usize,
                                    preview.image.height() as usize,
                                ],
                                image: preview.image,
                                animation: CanonicalStaticAnimation::Still,
                            }
                            .into_gpu_raster()
                            .pixels
                        });
                        Ok(RawPageResult::Preview { pixels })
                    }
                    Err(CanonicalDecodeError::Raw(error)) => Err(error),
                    Err(error) => Err(RawError::Io(format!("{error:?}"))),
                    _ => unreachable!("RAW preview routing"),
                }
            })();
            if cancel.load(Ordering::Acquire) {
                return;
            }
            tag.stage = RawResultStage::Preview;
            let _ = tx.send(FsLoadResult::Raw {
                tag,
                result: source.unwrap_or_else(RawPageResult::Failed),
            });
        });
    }

    fn start_raw_develop(&mut self, ctx: &egui::Context, idx: usize, priority: RawPriority) {
        let request_id = self.raw_pages.next_id();
        let Some(page) = self.raw_pages.page(idx) else {
            return;
        };
        let phase = Arc::clone(&page.develop);
        if !matches!(*phase.lock().unwrap(), RawDevelopPhase::Idle) {
            return;
        }
        let identity = page.source.clone();
        let context = self.fs_page_load_context_serial();
        let generation = self.items_generation;
        let brightness = self.settings.raw_brightness;
        let entry = match self.items.get(idx) {
            Some(GridItem::ZipImage { entry_name, .. }) => Some(entry_name.clone()),
            _ => None,
        };
        let provenance = self.relative_page_provenance_for_idx(idx);
        let intent = if self.fullscreen_idx == Some(idx) && entry.is_none() {
            let key = self.metadata_cache_key(idx);
            Some(RawPanoramaIntent {
                xmp: key
                    .as_ref()
                    .and_then(|key| self.xmp_panorama_info.get(key))
                    .and_then(|v| v.as_ref())
                    .is_some_and(|info| info.is_equirectangular()),
                base_only: key
                    .as_ref()
                    .and_then(|key| self.pano_quality_state.get(key))
                    .is_some_and(|state| {
                        matches!(state, crate::panorama::PanoramaQualityState::BaseOnly)
                    }),
                approved: self.pano_session_approved_max_pixels,
            })
        } else {
            None
        };
        let fs_priority = if priority == RawPriority::High {
            FsPageLoadPriority::High
        } else {
            FsPageLoadPriority::Normal
        };
        let ticket = self.fs_page_load_scheduler.request(
            context,
            idx,
            fs_priority,
            FsPageLoadContract::Sequential,
            Some(identity.item_key.clone()),
            self.input_seq,
        );
        let cancel = ticket.cancel_token();
        let waiter = ticket.waiter();
        let ticket = Arc::new(Mutex::new(ticket));
        *phase.lock().unwrap() = RawDevelopPhase::Preparing {
            request_id,
            cancel: Arc::clone(&ticket),
            highest_priority: priority,
            brightness,
        };
        self.raw_pages.page_mut(idx).unwrap().develop_started_at = std::time::Instant::now();
        let tx = self.raw_pages.tx.clone();
        let executor = Arc::clone(&self.raw_develop_executor);
        let seq = self.input_seq;
        let repaint = ctx.clone();
        std::thread::spawn(move || {
            let tag = RawResultTag {
                context,
                generation,
                idx,
                request_id,
                source: identity.clone(),
                stage: RawResultStage::Develop(brightness),
            };
            let Some(permit) = waiter.acquire_cancellable() else {
                let _ = tx.send(FsUploadResult::new(
                    idx,
                    FsLoadResult::Raw {
                        tag,
                        result: RawPageResult::Failed(RawError::Cancelled),
                    },
                    seq,
                    FsLoadPurpose::for_page(true),
                ));
                repaint.request_repaint();
                return;
            };
            let result = (|| {
                identity.validate()?;
                let verified = provenance
                    .as_ref()
                    .map(|p| p.read_verified())
                    .transpose()
                    .map_err(|e| RawError::Io(e.to_string()))?;
                let canonical = match entry.as_deref() {
                    Some(entry_name) => CanonicalImageSource::ArchiveEntry {
                        archive_path: &identity.path,
                        entry_name,
                    },
                    None => CanonicalImageSource::File {
                        path: &identity.path,
                        verified_bytes: verified.as_deref(),
                    },
                };
                let source = resolve_canonical_source(canonical, Some(&cancel))
                    .map_err(|e| RawError::Io(format!("{e:?}")))?
                    .with_raw_fingerprint(identity.fingerprint());
                identity.validate()?;
                // Source resolution uses D1 admission; executor queueing and LibRaw do not.
                drop(permit);
                ticket.lock().unwrap().disarm();
                let priority = match &*phase.lock().unwrap() {
                    RawDevelopPhase::Preparing {
                        request_id: current,
                        highest_priority,
                        ..
                    } if *current == request_id => *highest_priority,
                    _ => return Err(RawError::Cancelled),
                };
                let publish = |ticket| phase.lock().unwrap().publish(request_id, ticket);
                let output = crate::canonical_image_loader::decode_canonical_resolved(
                    source,
                    CanonicalDecodeOptions::fullscreen_cancellable(
                        AnimationPolicy::FirstFrameOnly,
                        &cancel,
                        RawStage::Full,
                    )
                    .with_raw_runtime(RawDecodeRuntime {
                        executor: &executor,
                        brightness,
                        priority,
                        on_submitted: Some(&publish),
                    }),
                );
                identity.validate()?;
                match output {
                    Ok(CanonicalImageDecode::Static(image)) => Ok(RawPageResult::Develop(
                        Box::new(raw_developed_raster(image.image, intent)),
                    )),
                    Err(CanonicalDecodeError::Raw(error)) => Err(error),
                    Err(error) => Err(RawError::Io(format!("{error:?}"))),
                    _ => unreachable!("RAW development routing"),
                }
            })();
            if cancel.load(Ordering::Acquire) {
                return;
            }
            let _ = tx.send(FsUploadResult::new(
                idx,
                FsLoadResult::Raw {
                    tag,
                    result: result.unwrap_or_else(RawPageResult::Failed),
                },
                seq,
                FsLoadPurpose::for_page(true),
            ));
            repaint.request_repaint();
        });
    }

    pub(crate) fn raw_current_developing(&self, idx: usize) -> bool {
        self.raw_pages.page(idx).is_some_and(|page| {
            matches!(
                *page.develop.lock().unwrap(),
                RawDevelopPhase::Preparing { .. } | RawDevelopPhase::Submitted { .. }
            )
        })
    }

    pub(crate) fn raw_loading_label(&self, idx: usize) -> Option<String> {
        use crate::ui_raw::RawLoadingStatus as Status;
        if !self.is_raw_page(idx) {
            return None;
        }
        if self.raw_development_blocked(idx)
            && self.raw_pages.classify(idx) == RawPageLoadState::PreviewShown
        {
            return Some(crate::ui_raw::RAW_BLOCKED_PREVIEW_NOTICE.into());
        }
        let status = if let Some(page) = self.raw_pages.page(idx) {
            match &*page.develop.lock().unwrap() {
                RawDevelopPhase::Preparing { .. } => Status::Reading,
                RawDevelopPhase::Submitted { ticket, .. } => match ticket.state() {
                    Some(crate::raw::RawTicketState::Queued) => Status::Queued,
                    Some(crate::raw::RawTicketState::Running) => {
                        let progress = ticket.progress().load(Ordering::Relaxed);
                        if progress > 0 {
                            Status::Developing(progress)
                        } else {
                            Status::Reading
                        }
                    }
                    _ => Status::Reading,
                },
                RawDevelopPhase::Idle => Status::Queued,
                RawDevelopPhase::Done => return None,
                RawDevelopPhase::Blocked(RawDevelopBlocked::Unsupported) => Status::Unsupported,
                RawDevelopPhase::Blocked(RawDevelopBlocked::Failed { .. }) => Status::Failed,
            }
        } else if self.raw_pages.classify(idx).terminal() {
            Status::SourceFailed
        } else {
            Status::Reading
        };
        Some(status.label())
    }

    pub(crate) fn observe_raw_presentation(&mut self, idx: usize) {
        let context = self.fs_page_load_context_serial();
        let Some(page) = self.raw_pages.page_mut(idx) else {
            return;
        };
        let RawPresentation::Awaiting { event, started_at } = page.presented else {
            return;
        };
        let (name, request_id) = match event {
            RawPresentationEvent::Preview { request_id } => ("preview_presented", request_id),
            RawPresentationEvent::Developed { request_id } => ("develop_presented", request_id),
        };
        crate::perf::event(
            "raw",
            name,
            Some(&page.source.item_key),
            request_id,
            &[
                ("context", context.into()),
                ("request_id", request_id.into()),
                (
                    "ms",
                    started_at
                        .elapsed()
                        .as_secs_f64()
                        .mul_add(1000.0, 0.0)
                        .into(),
                ),
            ],
        );
        page.presented = RawPresentation::Presented(event);
    }

    pub(crate) fn raw_prefetch_indicator(&mut self, current: usize) -> Option<FsPrefetchIndicator> {
        let order = self.get_still_image_indices();
        let pos = order.iter().position(|&idx| idx == current)?;
        let state = |idx| match self.raw_pages.classify(idx) {
            RawPageLoadState::Developed => FsPrefetchPageState::Ready,
            _ if self.raw_current_developing(idx) => FsPrefetchPageState::Active,
            _ => FsPrefetchPageState::Missing,
        };
        let behind = order[pos.saturating_sub(1)..pos]
            .iter()
            .copied()
            .filter(|&idx| self.is_raw_page(idx))
            .map(state)
            .collect::<Vec<_>>();
        let ahead = order[pos + 1..(pos + 3).min(order.len())]
            .iter()
            .copied()
            .filter(|&idx| self.is_raw_page(idx))
            .map(state)
            .collect::<Vec<_>>();
        if behind.is_empty() && ahead.is_empty() {
            return None;
        }
        let count = |expected| {
            behind
                .iter()
                .chain(&ahead)
                .filter(|&&s| s == expected)
                .count()
        };
        Some(FsPrefetchIndicator {
            ready_count: count(FsPrefetchPageState::Ready),
            active_count: count(FsPrefetchPageState::Active),
            missing_count: count(FsPrefetchPageState::Missing),
            behind,
            ahead,
        })
    }
}
