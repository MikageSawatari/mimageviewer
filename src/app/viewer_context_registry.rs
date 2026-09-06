use super::*;
use std::collections::HashMap;

/// A payload's identity, independent of its window and of the current main binding.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub(crate) struct ViewerContextId(u64);

impl ViewerContextId {
    pub(in crate::app) fn serial(self) -> u64 {
        self.0
    }

    #[cfg(not(windows))]
    pub(in crate::app) const fn single_context() -> Self {
        Self(0)
    }

    #[cfg(test)]
    pub(crate) const fn for_test(serial: u64) -> Self {
        Self(serial)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ContextResidence {
    Mounted,
    AtRest,
    Building,
    Retiring,
    Retired,
    Unknown,
}

enum Projection {
    Mounted(ViewerContextId),
    Building {
        reserved: ViewerContextId,
        previous: ViewerContextId,
        pending_bind: Option<u64>,
    },
}

enum Slot<P> {
    AtRest(P),
    Retiring(P),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::app) enum ForkPolicy {
    LiveMediaPark { window_id: u64 },
    MaterializedStillOpen,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum TableOp {
    ReplaceProjectionWithFreshEmpty,
    ForkProjectionIntoTransient(ForkPolicy),
    DepositInto(ViewerContextId),
    WithdrawFrom(ViewerContextId),
    RestoreProjectionAndDropDisplacedEmpty,
    DropTransientAsRetired(ViewerContextId),
}

#[derive(Debug, PartialEq, Eq)]
pub(in crate::app) enum BindError {
    WindowOwnedBy(ViewerContextId),
    ContextOwnedBy(u64),
    WrongOrigin(Option<ViewerContextId>),
    NotBindable(ContextResidence),
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) struct MountError {
    pub(crate) id: ViewerContextId,
    pub(crate) residence: ContextResidence,
}

#[derive(Debug, PartialEq, Eq)]
pub(in crate::app) enum RetireError {
    /// App is mid-build; nothing can be retired.
    Building,
    /// The id is not at rest.
    NotAtRest(ContextResidence),
    /// The main context cannot be retired. Promote first.
    IsMain,
}

enum PendingTransition {
    BeginBuild {
        reserved: ViewerContextId,
        previous: ViewerContextId,
    },
    CommitBuild {
        reserved: ViewerContextId,
        previous: ViewerContextId,
        pending_bind: Option<u64>,
    },
    AbortBuild {
        reserved: ViewerContextId,
        previous: ViewerContextId,
    },
    Mount {
        from: ViewerContextId,
        to: ViewerContextId,
    },
    Fork {
        policy: ForkPolicy,
        new_id: ViewerContextId,
        from: ViewerContextId,
    },
    Promote {
        stashed: ViewerContextId,
        fresh: ViewerContextId,
    },
    Retire {
        id: ViewerContextId,
    },
}

/// Logical ownership table for a caller-owned projection payload.
///
/// Executing a planned operation sequence may panic. Such a panic does not provide a complete
/// rollback: a payload already deposited in a slot can remain there. The protocol guarantees only
/// that replacement keeps a payload in the projection at every instant (I1b), and that bindings are
/// not published before a successful finalizer (I8).
struct ContextTable<P> {
    projection: Projection,
    slots: HashMap<ViewerContextId, Slot<P>>,
    main: ViewerContextId,
    window_of: HashMap<ViewerContextId, u64>,
    context_of: HashMap<u64, ViewerContextId>,
    next_serial: u64,
    pending: Option<PendingTransition>,
}

impl<P> ContextTable<P> {
    fn new() -> Self {
        // Detached context serials historically start at one. Keep the initial projection at
        // serial zero so the first reserved detached identity preserves that generation encoding.
        let main = ViewerContextId(0);
        Self {
            projection: Projection::Mounted(main),
            slots: HashMap::new(),
            main,
            window_of: HashMap::new(),
            context_of: HashMap::new(),
            next_serial: 1,
            pending: None,
        }
    }

    fn allocate_id(&mut self) -> ViewerContextId {
        let id = ViewerContextId(self.next_serial);
        self.next_serial = self
            .next_serial
            .checked_add(1)
            .expect("viewer context serial exhausted");
        id
    }

    fn main(&self) -> ViewerContextId {
        assert!(self.pending.is_none());
        self.main
    }

    fn mounted_id(&self) -> Option<ViewerContextId> {
        assert!(self.pending.is_none());
        match self.projection {
            Projection::Mounted(id) => Some(id),
            Projection::Building { .. } => None,
        }
    }

    fn projected_id(&self) -> ViewerContextId {
        assert!(self.pending.is_none());
        match self.projection {
            Projection::Mounted(id) => id,
            Projection::Building { reserved, .. } => reserved,
        }
    }

    fn residence(&self, id: ViewerContextId) -> ContextResidence {
        assert!(self.pending.is_none());
        self.residence_core(id)
    }

    fn residence_core(&self, id: ViewerContextId) -> ContextResidence {
        match &self.projection {
            Projection::Mounted(mounted) if *mounted == id => ContextResidence::Mounted,
            Projection::Building { reserved, .. } if *reserved == id => ContextResidence::Building,
            _ => match self.slots.get(&id) {
                Some(Slot::Retiring(_)) => ContextResidence::Retiring,
                Some(Slot::AtRest(_)) => ContextResidence::AtRest,
                None if id.serial() <= self.next_serial - 1 => ContextResidence::Retired,
                None => ContextResidence::Unknown,
            },
        }
    }

    fn locate_window_context(&self, window_id: u64) -> Option<(ViewerContextId, ContextResidence)> {
        assert!(self.pending.is_none());
        self.context_of
            .get(&window_id)
            .copied()
            .map(|id| (id, self.residence_core(id)))
    }

    fn window_binding_probe(&self, window_id: u64) -> Option<(ViewerContextId, ContextResidence)> {
        self.context_of
            .get(&window_id)
            .copied()
            .map(|id| (id, self.residence_core(id)))
    }

    fn window_for_context(&self, id: ViewerContextId) -> Option<u64> {
        assert!(self.pending.is_none());
        self.window_of.get(&id).copied()
    }

    fn ids(&self) -> Vec<ViewerContextId> {
        assert!(self.pending.is_none());
        let mut ids = Vec::with_capacity(self.slots.len() + 1);
        ids.extend(self.slots.keys().copied());
        ids.push(match self.projection {
            Projection::Mounted(id) => id,
            Projection::Building { reserved, .. } => reserved,
        });
        ids.sort_unstable();
        ids.dedup();
        ids
    }

    /// Every context except the one currently projected onto `App`.
    ///
    /// Traversals want "all the others", and asking that as `id != mounted_id()` is wrong
    /// while a context is being built: `mounted_id` is `None` then, so the reserved id
    /// survives the filter and the traversal tries to mount the payload it is already
    /// standing on. That fails with `Building` and gets logged as an error instead of being
    /// recognised as self.
    fn other_ids(&self) -> Vec<ViewerContextId> {
        let projected = self.projected_id();
        let mut ids = self.ids();
        ids.retain(|id| *id != projected);
        ids
    }

    fn bind_window(&mut self, id: ViewerContextId, window_id: u64) -> Result<(), BindError> {
        assert!(self.pending.is_none());
        self.bind_core(id, window_id)
    }

    fn unbind_window(&mut self, window_id: u64) -> Option<ViewerContextId> {
        assert!(self.pending.is_none());
        self.unbind_window_core(window_id)
    }

    fn bind_core(&mut self, id: ViewerContextId, window_id: u64) -> Result<(), BindError> {
        let residence = self.residence_core(id);
        if !matches!(
            residence,
            ContextResidence::Mounted | ContextResidence::AtRest
        ) {
            return Err(BindError::NotBindable(residence));
        }

        if let Some(owner) = self.context_of.get(&window_id).copied() {
            if owner != id {
                return Err(BindError::WindowOwnedBy(owner));
            }
        }
        if let Some(existing_window) = self.window_of.get(&id).copied() {
            if existing_window != window_id {
                return Err(BindError::ContextOwnedBy(existing_window));
            }
        }

        self.window_of.insert(id, window_id);
        self.context_of.insert(window_id, id);
        Ok(())
    }

    fn unbind_window_core(&mut self, window_id: u64) -> Option<ViewerContextId> {
        let id = self.context_of.remove(&window_id)?;
        assert_eq!(self.window_of.remove(&id), Some(window_id));
        Some(id)
    }

    #[cfg(test)]
    fn unbind_context_core(&mut self, id: ViewerContextId) -> Option<u64> {
        let window_id = self.window_of.remove(&id)?;
        assert_eq!(self.context_of.remove(&window_id), Some(id));
        Some(window_id)
    }

    fn transfer_core(
        &mut self,
        window_id: u64,
        from: ViewerContextId,
        to: ViewerContextId,
    ) -> Result<(), BindError> {
        let actual = self.context_of.get(&window_id).copied();
        if actual != Some(from) {
            return Err(BindError::WrongOrigin(actual));
        }

        for id in [from, to] {
            let residence = self.residence_core(id);
            if !matches!(
                residence,
                ContextResidence::Mounted | ContextResidence::AtRest
            ) {
                return Err(BindError::NotBindable(residence));
            }
        }

        if let Some(existing_window) = self.window_of.get(&to).copied() {
            if existing_window != window_id {
                return Err(BindError::ContextOwnedBy(existing_window));
            }
        }

        assert_eq!(self.window_of.remove(&from), Some(window_id));
        self.window_of.insert(to, window_id);
        self.context_of.insert(window_id, to);
        Ok(())
    }

    fn plan_begin_build(&mut self) -> (ViewerContextId, Vec<TableOp>) {
        assert!(self.pending.is_none());
        let previous = match self.projection {
            Projection::Mounted(id) => id,
            Projection::Building { .. } => panic!("cannot begin a build while already building"),
        };
        let reserved = self.allocate_id();
        self.pending = Some(PendingTransition::BeginBuild { reserved, previous });
        (
            reserved,
            vec![
                TableOp::ReplaceProjectionWithFreshEmpty,
                TableOp::DepositInto(previous),
            ],
        )
    }

    fn finish_begin_build(&mut self) {
        let (reserved, previous) = match self.pending.as_ref() {
            Some(PendingTransition::BeginBuild { reserved, previous }) => (*reserved, *previous),
            _ => panic!("finish_begin_build called without its pending transition"),
        };
        assert!(matches!(
            &self.projection,
            Projection::Mounted(id) if *id == previous
        ));
        assert!(matches!(self.slots.get(&previous), Some(Slot::AtRest(_))));
        assert!(!self.slots.contains_key(&reserved));
        self.projection = Projection::Building {
            reserved,
            previous,
            pending_bind: None,
        };
        self.pending = None;
    }

    fn reserve_window_binding_for_build(&mut self, window_id: u64) {
        assert!(self.pending.is_none());
        match &mut self.projection {
            Projection::Building { pending_bind, .. } => match *pending_bind {
                None => *pending_bind = Some(window_id),
                Some(existing) if existing == window_id => {}
                Some(_) => panic!("a build cannot reserve more than one window"),
            },
            Projection::Mounted(_) => {
                panic!("window binding can only be reserved while building")
            }
        }
    }

    fn plan_commit_build(&mut self) -> Vec<TableOp> {
        assert!(self.pending.is_none());
        let (reserved, previous, pending_bind) = match &self.projection {
            Projection::Building {
                reserved,
                previous,
                pending_bind,
            } => (*reserved, *previous, *pending_bind),
            Projection::Mounted(_) => panic!("cannot commit when no build is active"),
        };
        self.pending = Some(PendingTransition::CommitBuild {
            reserved,
            previous,
            pending_bind,
        });
        vec![
            TableOp::ReplaceProjectionWithFreshEmpty,
            TableOp::DepositInto(reserved),
            TableOp::WithdrawFrom(previous),
            TableOp::RestoreProjectionAndDropDisplacedEmpty,
        ]
    }

    fn finish_commit_build(&mut self) -> ViewerContextId {
        let (reserved, previous, pending_bind) = match self.pending.as_ref() {
            Some(PendingTransition::CommitBuild {
                reserved,
                previous,
                pending_bind,
            }) => (*reserved, *previous, *pending_bind),
            _ => panic!("finish_commit_build called without its pending transition"),
        };
        match &self.projection {
            Projection::Building {
                reserved: projected_reserved,
                previous: projected_previous,
                pending_bind: projected_bind,
            } => {
                assert_eq!(*projected_reserved, reserved);
                assert_eq!(*projected_previous, previous);
                assert_eq!(*projected_bind, pending_bind);
            }
            Projection::Mounted(_) => panic!("build projection disappeared before commit"),
        }
        assert!(matches!(self.slots.get(&reserved), Some(Slot::AtRest(_))));
        assert!(!self.slots.contains_key(&previous));

        self.projection = Projection::Mounted(previous);
        if let Some(window_id) = pending_bind {
            self.bind_core(reserved, window_id)
                .unwrap_or_else(|error| panic!("build binding publication failed: {error:?}"));
        }
        self.pending = None;
        reserved
    }

    fn plan_abort_build(&mut self) -> Vec<TableOp> {
        assert!(self.pending.is_none());
        let (reserved, previous) = match &self.projection {
            Projection::Building {
                reserved, previous, ..
            } => (*reserved, *previous),
            Projection::Mounted(_) => panic!("cannot abort when no build is active"),
        };
        self.pending = Some(PendingTransition::AbortBuild { reserved, previous });
        vec![
            TableOp::ReplaceProjectionWithFreshEmpty,
            TableOp::DepositInto(reserved),
            TableOp::WithdrawFrom(previous),
            TableOp::RestoreProjectionAndDropDisplacedEmpty,
            TableOp::WithdrawFrom(reserved),
            TableOp::DropTransientAsRetired(reserved),
        ]
    }

    fn finish_abort_build(&mut self) {
        let (reserved, previous) = match self.pending.as_ref() {
            Some(PendingTransition::AbortBuild { reserved, previous }) => (*reserved, *previous),
            _ => panic!("finish_abort_build called without its pending transition"),
        };
        match &self.projection {
            Projection::Building {
                reserved: projected_reserved,
                previous: projected_previous,
                ..
            } => {
                assert_eq!(*projected_reserved, reserved);
                assert_eq!(*projected_previous, previous);
            }
            Projection::Mounted(_) => panic!("build projection disappeared before abort"),
        }
        assert!(!self.slots.contains_key(&reserved));
        assert!(!self.slots.contains_key(&previous));
        self.projection = Projection::Mounted(previous);
        self.pending = None;
    }

    fn plan_mount(&mut self, id: ViewerContextId) -> Result<Vec<TableOp>, MountError> {
        assert!(self.pending.is_none());
        let current = match self.projection {
            Projection::Mounted(current) => current,
            Projection::Building { .. } => {
                return Err(MountError {
                    id,
                    residence: ContextResidence::Building,
                });
            }
        };
        let residence = self.residence_core(id);
        match residence {
            ContextResidence::Mounted => {
                self.pending = Some(PendingTransition::Mount {
                    from: current,
                    to: id,
                });
                Ok(Vec::new())
            }
            ContextResidence::AtRest => {
                self.pending = Some(PendingTransition::Mount {
                    from: current,
                    to: id,
                });
                Ok(vec![
                    TableOp::ReplaceProjectionWithFreshEmpty,
                    TableOp::DepositInto(current),
                    TableOp::WithdrawFrom(id),
                    TableOp::RestoreProjectionAndDropDisplacedEmpty,
                ])
            }
            residence => Err(MountError { id, residence }),
        }
    }

    fn finish_mount(&mut self) {
        let (from, to) = match self.pending.as_ref() {
            Some(PendingTransition::Mount { from, to }) => (*from, *to),
            _ => panic!("finish_mount called without its pending transition"),
        };
        assert!(matches!(
            &self.projection,
            Projection::Mounted(id) if *id == from
        ));
        if from != to {
            assert!(matches!(self.slots.get(&from), Some(Slot::AtRest(_))));
            assert!(!self.slots.contains_key(&to));
        }
        self.projection = Projection::Mounted(to);
        self.pending = None;
    }

    fn plan_fork(&mut self, policy: ForkPolicy) -> (ViewerContextId, Vec<TableOp>) {
        assert!(self.pending.is_none());
        let from = match self.projection {
            Projection::Mounted(id) => id,
            Projection::Building { .. } => panic!("cannot fork while building"),
        };
        if let ForkPolicy::LiveMediaPark { window_id } = policy {
            assert_eq!(self.window_of.get(&from).copied(), Some(window_id));
        }
        let new_id = self.allocate_id();
        self.pending = Some(PendingTransition::Fork {
            policy,
            new_id,
            from,
        });
        (
            new_id,
            vec![
                TableOp::ForkProjectionIntoTransient(policy),
                TableOp::DepositInto(new_id),
            ],
        )
    }

    fn finish_fork(&mut self) -> ViewerContextId {
        let (policy, new_id, from) = match self.pending.as_ref() {
            Some(PendingTransition::Fork {
                policy,
                new_id,
                from,
            }) => (*policy, *new_id, *from),
            _ => panic!("finish_fork called without its pending transition"),
        };
        assert!(matches!(
            &self.projection,
            Projection::Mounted(id) if *id == from
        ));
        assert!(matches!(self.slots.get(&new_id), Some(Slot::AtRest(_))));
        if let ForkPolicy::LiveMediaPark { window_id } = policy {
            self.transfer_core(window_id, from, new_id)
                .unwrap_or_else(|error| panic!("fork binding transfer failed: {error:?}"));
        }
        self.pending = None;
        new_id
    }

    fn plan_promote(&mut self) -> Vec<TableOp> {
        assert!(self.pending.is_none());
        let stashed = match self.projection {
            Projection::Mounted(id) => id,
            Projection::Building { .. } => panic!("cannot promote while building"),
        };
        assert_eq!(stashed, self.main);
        let fresh = self.allocate_id();
        self.pending = Some(PendingTransition::Promote { stashed, fresh });
        vec![
            TableOp::ReplaceProjectionWithFreshEmpty,
            TableOp::DepositInto(stashed),
        ]
    }

    fn finish_promote(&mut self) -> ViewerContextId {
        let (stashed, fresh) = match self.pending.as_ref() {
            Some(PendingTransition::Promote { stashed, fresh }) => (*stashed, *fresh),
            _ => panic!("finish_promote called without its pending transition"),
        };
        assert!(matches!(
            &self.projection,
            Projection::Mounted(id) if *id == stashed
        ));
        assert!(matches!(self.slots.get(&stashed), Some(Slot::AtRest(_))));
        assert!(!self.slots.contains_key(&fresh));
        self.projection = Projection::Mounted(fresh);
        self.main = fresh;
        self.pending = None;
        stashed
    }

    fn begin_retire(&mut self, id: ViewerContextId) -> Result<(), RetireError> {
        assert!(self.pending.is_none());
        if matches!(self.projection, Projection::Building { .. }) {
            return Err(RetireError::Building);
        }
        let residence = self.residence_core(id);
        if residence != ContextResidence::AtRest {
            return Err(RetireError::NotAtRest(residence));
        }
        if id == self.main {
            return Err(RetireError::IsMain);
        }
        let payload = match self.slots.remove(&id) {
            Some(Slot::AtRest(payload)) => payload,
            Some(Slot::Retiring(_)) | None => unreachable!("residence and slot disagreed"),
        };
        assert!(self.slots.insert(id, Slot::Retiring(payload)).is_none());
        Ok(())
    }

    fn plan_finish_retire(&mut self, id: ViewerContextId) -> Vec<TableOp> {
        assert!(self.pending.is_none());
        assert!(matches!(self.slots.get(&id), Some(Slot::Retiring(_))));
        self.pending = Some(PendingTransition::Retire { id });
        vec![
            TableOp::WithdrawFrom(id),
            TableOp::DropTransientAsRetired(id),
        ]
    }

    fn finish_retire(&mut self) {
        let id = match self.pending.as_ref() {
            Some(PendingTransition::Retire { id }) => *id,
            _ => panic!("finish_retire called without its pending transition"),
        };
        assert!(!self.slots.contains_key(&id));
        self.pending = None;
    }

    fn deposit(&mut self, id: ViewerContextId, payload: P) {
        assert!(!self.slots.contains_key(&id));
        assert!(self.slots.insert(id, Slot::AtRest(payload)).is_none());
    }

    fn withdraw(&mut self, id: ViewerContextId) -> P {
        match self.slots.remove(&id) {
            Some(Slot::AtRest(payload) | Slot::Retiring(payload)) => payload,
            None => panic!("cannot withdraw a context without a slot"),
        }
    }

    fn retiring_slot_mut(&mut self, id: ViewerContextId) -> Option<&mut P> {
        assert!(self.pending.is_none());
        match self.slots.get_mut(&id) {
            Some(Slot::Retiring(payload)) => Some(payload),
            Some(Slot::AtRest(_)) | None => None,
        }
    }

    fn at_rest(&self, id: ViewerContextId) -> Option<&P> {
        assert!(self.pending.is_none());
        match self.slots.get(&id) {
            Some(Slot::AtRest(payload)) => Some(payload),
            Some(Slot::Retiring(_)) | None => None,
        }
    }
}

#[cfg(windows)]
pub(in crate::app) struct ViewerContextRegistry {
    table: ContextTable<Box<ViewerContextBundle>>,
}

#[cfg(windows)]
impl ViewerContextRegistry {
    pub(in crate::app) fn new() -> Self {
        Self {
            table: ContextTable::new(),
        }
    }
}

#[cfg(windows)]
pub(in crate::app) enum BuildOutcome {
    Commit,
    Abort(&'static str),
}

#[cfg(windows)]
#[derive(Debug, PartialEq, Eq)]
pub(in crate::app) enum RetireContextError {
    Mount(MountError),
    Retire(RetireError),
}

// --- Production ViewerContextBundle ownership primitives (stage ②-b) ---

#[cfg(windows)]
pub(in crate::app) struct ViewerContextBundle {
    address: String,
    current_folder: Option<PathBuf>,
    favorite_view_context: FavoriteViewContextState,
    navigation_scope: ViewerNavigationScope,
    archive_source_override: Option<PathBuf>,
    zip_nav: Option<crate::zip_tree::ZipNavState>,
    stack_mode_requested: bool,
    stack_view: Option<crate::filename_stack::StackView>,
    stack_showing_flat: bool,
    stack_active_rule: Option<String>,
    stack_script_error: Option<String>,
    stack_toggle_select_path: Option<PathBuf>,
    items: Vec<GridItem>,
    items_generation: u64,
    visible_indices: Vec<usize>,
    /// `items` と同じ添字の正規化済み basename と、その worker lifecycle。
    /// query / tokens / debounce は App 全体で同じ絞り込み条件を使うため swap しないが、
    /// 導出 cache は generation 空間を共有しない viewer context と一緒に所有する。
    /// failed generation も別 context の同値 generation の build を抑止しないようここに含める。
    facet_name_cache: Vec<Box<str>>,
    facet_name_cache_generation: Option<u64>,
    facet_name_cache_pending: Option<facet_name_filter::FacetNameCachePending>,
    facet_name_cache_failed_generation: Option<u64>,
    thumbnails: Vec<ThumbnailState>,
    image_metas: Vec<Option<(i64, i64)>>,
    video_thumb_overrides: std::collections::HashMap<String, PathBuf>,
    auto_aspect: crate::auto_aspect::AutoAspectState,
    selected: Option<usize>,
    grid_click_selection_anchor: Option<GridClickSelectionAnchor>,
    scroll_offset_y: f32,
    scroll_to_selected: bool,
    pending_grid_scroll: Option<GridScrollIntent>,
    requested: std::collections::HashMap<usize, bool>,
    idle_upgrade_cache_bypass_ineligible: std::collections::HashSet<usize>,
    keep_range: (usize, usize),
    keep_set: std::collections::HashSet<usize>,
    still_seek_thumbnail_pages: std::collections::HashSet<usize>,
    still_seek_thumbnail_pages_shared: Arc<std::sync::RwLock<std::collections::HashSet<usize>>>,
    thumbnail_eviction_generation: Option<u64>,
    details_thumb_suppression_applied: bool,
    details_hover_thumb_idx: Option<usize>,
    details_hover_thumb_viewport_open: bool,
    texture_backlog: Vec<crate::thumb_loader::ThumbMsg>,
    details_order: Vec<usize>,
    details_order_revision: u64,
    details_cell_content_revisions: DetailsCellContentRevisions,
    details_tag_prewarm_indices: Vec<usize>,
    details_lazy_meta: std::collections::HashMap<String, DetailsLazyMeta>,
    details_meta_pending: Option<DetailsMetaPending>,
    details_lazy_visible_revision: u64,
    details_image_dims_state: LazyColumnState,
    metadata_cache: std::collections::HashMap<String, Option<crate::png_metadata::AiMetadata>>,
    exif_cache: std::collections::HashMap<String, Option<crate::exif_reader::ExifInfo>>,
    xmp_cache: std::collections::HashMap<String, Option<crate::xmp_reader::XmpTweetInfo>>,
    xmp_panorama_info:
        std::collections::HashMap<String, Option<crate::xmp_reader::XmpPanoramaInfo>>,
    metadata_pending: Option<MetadataLoadPending>,
    /// 一覧ごとの tags.db 表示キャッシュ。detached context の実フォルダ load が
    /// main 一覧のタグを消さないよう、item 列と同じ ownership で交換する。
    tags_cache: std::collections::HashMap<String, Vec<String>>,
    tag_prewarm_pending: Option<crate::tag_prewarm::TagPrewarmPending>,
    tag_prewarm_queued: std::collections::HashSet<usize>,
    tag_legacy_seed_pending: Option<crate::tag_legacy_seed_worker::LegacySeedPending>,
    pending_finalize: std::collections::HashSet<usize>,
    // ── per-context ロード複合体 (review-v2.3.0 P2-8/P2-9) ──
    // thumb channel (tx/rx)・cancel_token・ワーカーキュー 2 本は `start_loading_items` が
    // ロードごとに作り直す「現用セット」で、コンテキストに属する。bundle に含めないと
    // (a) detached book context の load_zip/pdf_as_folder が main の cancel_token を flip し、
    //     main の動画サムネ抽出 (再リクエスト経路なし) を恒久停止させる (P2-9)、
    // (b) channel/token が global なせいで bundle 済み bookkeeping (requested 等) が swap 後に
    //     信用できず、swap のたびに clear → 毎フレーム再エンキュー → サムネ重複デコード
    //     churn になる (P2-8)。
    tx: mpsc::Sender<ThumbMsg>,
    rx: mpsc::Receiver<ThumbMsg>,
    cancel_token: Arc<AtomicBool>,
    reload_queue: Option<Arc<NotifyQueue>>,
    heavy_io_queue: Option<Arc<NotifyQueue>>,
    // worker が out-of-keep skip / 優先度計算に読む共有 atomic も per-context にする。
    // global のままだと (a) detached ロードの初期化 (0,0 store) が main の keep range を
    // 一瞬潰して可視サムネの skip churn を起こし、(b) detached の queue 項目が以後
    // main の keep range で gate され続ける (review-v2.3.0 hunt P3)。
    scroll_hint: Arc<AtomicUsize>,
    visible_end_shared: Arc<AtomicUsize>,
    keep_start_shared: Arc<AtomicUsize>,
    keep_end_shared: Arc<AtomicUsize>,
    last_vis_range: (usize, usize),
    vis_settle_at: Option<std::time::Instant>,
    vis_first_logged: bool,
    vis_all_logged: bool,
    folder_nav_pending: Option<FolderNavPending>,
    folder_pane_open_pending: Option<FolderPaneOpenPending>,
    pending_folder_nav_steps: i32,
    pending_folder_nav_mode: FolderNavMode,
    search_filter: Option<std::collections::HashSet<usize>>,
    search_filter_origin_folder: Option<PathBuf>,
    checked: std::collections::HashSet<usize>,
    rotation_cache: std::collections::HashMap<usize, crate::rotation_db::Rotation>,
    page_dims_cache: crate::page_dims::PageDimsCache,
    spread_display_units_cache: crate::ui_fullscreen::SpreadDisplayUnitsCache,
    rating_cache: std::collections::HashMap<usize, u8>,
    /// 現在の viewer context だけに効く評価 filter の一時解除 anchor。
    ///
    /// `effective_rating_filter` と snapshot / folder navigation の restore が同じ
    /// mounted projection を読み書きする。ここを bundle 外に置くと、別 context の
    /// snapshot release が owner の suppression を消費し、表示 filter も sibling へ漏れる。
    rating_filter_suppressed_at: Option<(PathBuf, [bool; 6])>,
    /// App-global な path rating 更新をこの context の idx cache へ反映済みの世代。
    rating_session_write_seen_generation: u64,
    metadata_import_refresh_index: Option<MetadataImportRefreshIndex>,
    current_folder_rating_cache: Option<u8>,
    current_folder_last_mtime: Option<std::time::SystemTime>,
    current_folder_signature: Option<u64>,
    folder_pin_map: std::collections::HashMap<String, crate::folder_thumb_pins::FolderPinSource>,
    converted_archive_cache_paths: std::collections::HashMap<String, ConvertedArchiveSourceState>,
    converted_archive_pin_root_states:
        std::collections::HashMap<String, ConvertedArchivePinRootState>,
    converted_archive_cache_paths_pending: Option<ConvertedArchiveCachePathsPending>,
    current_color_cache_map: Option<
        Arc<std::sync::RwLock<std::collections::HashMap<String, crate::catalog::CacheEntry>>>,
    >,
    current_color_catalog: Option<Arc<crate::catalog::CatalogDb>>,
    /// VST3 startup load 完了まで start_fs_load を保留している、この context の media idx。
    vst3_deferred_media_open: Option<usize>,
    fullscreen_idx: Option<usize>,
    /// 分割表示中に、今どちら側を見ているか。分割していなければ `Full`。
    ///
    /// **元ページ (`fullscreen_idx`) と同じ context 所有**にする。片方だけ App 側に
    /// 置くと、context を切り替えたときに前の viewer の左右が次の viewer へ残る。
    /// 永続化はしない (開き直しは分割方向の最初の半分へ着地する)。
    fullscreen_page_slice: crate::page_split::PageSlice,
    viewer_session: ViewerSession,
    native_video_in_window_active: bool,
    video_audio_mode: Option<usize>,
    video_audio_vst: Option<VideoAudioVstState>,
    video_audio_mode_entry_target: Option<(
        crate::video::NativeVideoPlacement,
        windows::Win32::Foundation::RECT,
        u64,
    )>,
    video_audio_exit_pending: Option<VideoAudioExitPending>,
    panorama_state: Option<crate::panorama::PanoramaState>,
    /// 360 で見ているという意図 (+ 選んだ投影方式)。フルスクリーンを閉じても残る
    /// ので、App グローバルに置くと別ウィンドウの 360 が混ざる (backlog §1.145)。
    panorama_intent: crate::panorama::PanoramaSessionIntent,
    /// 右情報パネルの表示状態 (明示 open / ロック / ホバー latch)。
    /// App-global に置くと別ウィンドウの操作で他方のパネルが開閉する (backlog §1.158)。
    fs_info_panel: crate::ui_helpers::FullscreenInfoPanelState,
    pano_toast_shown_for_current_fs: bool,
    analysis_mode: bool,
    analysis_hover_color: Option<[u8; 4]>,
    analysis_pinned_color: Option<[u8; 4]>,
    analysis_grayscale: bool,
    analysis_mosaic_grid: bool,
    analysis_filter_mag: u8,
    analysis_guide_drag: Option<(egui::Pos2, egui::Pos2, u8)>,
    view_trim_mode: bool,
    view_trim_apply_mode: crate::view_trim::ViewTrimApplyMode,
    view_trim_page_apply_root_idx: Option<usize>,
    view_trim_page_spread_separate: bool,
    view_trim_book_settings: crate::view_trim::ViewTrimBookSettings,
    view_trim_page_overrides:
        std::collections::HashMap<usize, crate::view_trim::ViewTrimPageOverride>,
    view_trim_dirty_page_overrides: std::collections::HashSet<usize>,
    view_trim_save_pending: bool,
    fs_cache: ItemsGenerationMap<FsCacheEntry>,
    fs_lanczos_cache: crate::gpu_lanczos::GpuLanczosCache,
    fs_margin_bbox_cache: std::collections::HashMap<usize, (u64, usize, Option<egui::Rect>)>,
    input_generation: std::collections::HashMap<usize, u64>,
    fs_pending: ItemsGenerationMap<FsPendingValue>,
    fullscreen_pdf_promotion: FullscreenPdfPromotionState,
    /// この viewer context の実描画先から得た PDF 初回レンダターゲット。
    fs_pdf_display_target: Option<crate::pdf_loader::PdfDisplayTarget>,
    fs_early_dims: ItemsGenerationMap<[usize; 2]>,
    fs_upload_backlog: FsUploadBacklog,
    top_level_grid_view: top_level_grid_view::TopLevelGridView,
    /// snapshot 表示中に退避してある「元の一覧」。
    ///
    /// `activate_snapshot` は `items` / `thumbnails` / `visible_indices` /
    /// `scroll_offset_y` / `selected` / `zip_nav` の **6 つとも本 bundle の field** を
    /// ここへ退避する。したがって退避先も同じ context が所有していなければならない。
    /// 直上の `top_level_grid_view` (= どの top-level surface を表示中か) と対であり、
    /// **片方だけ per-context だと「表示は context ごと・取り消しは App 共有」**という
    /// 所有境界の食い違いになる。実際、App-global だった頃は
    /// ① 2 つ目の context が snapshot を張ると 1 つ目の退避を上書きして復元不能にし、
    /// ② 両者が同じフォルダを指していると解除時に他 context の一覧を書き戻した
    /// (監査 A2b の既知の指摘、docs/detached-rework-plan.md §9.5)。
    snapshot: Option<crate::snapshot::SnapshotState>,
    /// Ctrl+G / Ctrl+S の検索由来 snapshot が synthetic サブ展開へ戻るための fallback。
    /// search state 自体は App の UI projection だが、この restore payload は snapshot と
    /// 一緒に fork / mount / retire されなければ sibling の `take()` で失われる。
    global_search_subfolder_restore:
        Option<super::subfolder_expansion::SubfolderExpansionRestoreState>,
    favsearch_subfolder_restore: Option<super::subfolder_expansion::SubfolderExpansionRestoreState>,
    items_are_global_search_view: bool,
    items_are_tag_view: bool,
    items_are_reading_history_view: bool,
    items_are_bookmark_view: bool,
    items_are_rating_view: bool,
    items_are_subfolder_expansion_view: bool,
    items_are_smart_folder_view: bool,
    items_are_drive_list: bool,
    reading_history_return_from: Option<PathBuf>,
    bookmark_view_state: Option<BookmarkViewState>,
    bookmark_open_pending: Option<crate::bookmark_browser::PendingBookmarkOpen>,
    fs_open_intent_from_grid: bool,
    video_presentation_transition: PresentationTransitionOwner,
    fs_zoom: f32,
    fs_pan: egui::Vec2,
    fs_zoom_active: bool,
    fs_zoom_aiming: bool,
    fs_zoom_factor: f32,
    fs_zoom_pdf_rerender_idx: Option<usize>,
    fs_zoom_pdf_rerender_zoom: f32,
    fs_pan_drag_start: Option<(egui::Pos2, egui::Vec2)>,
    fs_vertical_scroll: f32,
    fs_seek_drag_active: bool,
    fs_seek_gesture: crate::ui_fullscreen::StillSeekGesture,
    fs_seek_overlay_visible: bool,
    fs_vertical_cache_keep_set: std::collections::HashSet<usize>,
    continuous_page_transitions: std::collections::HashMap<usize, ContinuousPageTransition>,
    fs_free_rotation: f32,
    fs_rotation_drag_start: Option<(egui::Pos2, f32)>,
    analysis_zoom: f32,
    analysis_pan: egui::Vec2,
    analysis_pan_drag_start: Option<(egui::Pos2, egui::Vec2)>,
    analysis_overlay_cache: Option<(
        egui::TextureHandle,
        u8,
        Option<[u8; 4]>,
        f32,
        egui::Vec2,
        usize,
    )>,
    analysis_hist_cache: Option<(f32, egui::Vec2, usize, [u32; 360], [u32; 256], [u32; 256])>,
    analysis_sv_cache: Option<(f32, egui::Vec2, usize, egui::TextureHandle)>,
    spread_mode: crate::settings::SpreadMode,
    spread_shift_anchor_idx: Option<usize>,
    reading_flow: crate::settings::ReadingFlow,
    reading_direction: crate::settings::ReadingDirection,
    slideshow_playing: bool,
    slideshow_next_at: std::time::Instant,
    slideshow_anchor_idx: Option<usize>,
    continuous_reading_scroll_transition: Option<ContinuousReadingScrollTransition>,
    slideshow_scroll_range_cache: Option<(usize, f32, f32)>,
    pdf_password_request: Option<PdfPasswordRequest>,
    pdf_current_password: Option<String>,
    pdf_password_pending_save: Option<(PathBuf, String)>,
    pdf_enumerate_pending: Option<(
        PathBuf,
        Option<String>,
        crate::pdf_loader::PdfEnumerateHandle,
    )>,
    zip_enumerate_pending: Option<ZipEnumeratePending>,
    fs_nav_after_pdf_enumerate: Option<DeferredFsReopen>,
    pending_auto_fs_open: bool,
    pending_return_to_parent: bool,
    pdf_placeholder_count: Option<u32>,
    viewer_navigation_caches: crate::ui_fullscreen::ViewerNavigationCaches,
    fs_nav_locked_gen: Option<u64>,
    fs_nav_dropped_block_signature: Option<String>,
    fs_nav_dropped_block_count: u32,
    fs_load_skip_signature: Option<String>,
    fs_holdover_tex: Option<FsHoldover>,
    fs_boundary_hint: Option<crate::ui_fullscreen::FsBoundaryHint>,
    virtual_folder_writeback: Option<VirtualFolderWriteback>,
    pdf_prefetch_grace_until: Option<std::time::Instant>,
    thumb_pixels: std::collections::HashMap<usize, std::sync::Arc<egui::ColorImage>>,
    thumb_edit_preview_layers: std::collections::HashMap<
        usize,
        std::sync::Arc<Vec<crate::edit_preview_cache::CachedAnnotationLayer>>,
    >,
    thumb_edit_preview_keys: std::collections::HashMap<usize, String>,
    thumb_adjust_tex: std::collections::HashMap<usize, egui::TextureHandle>,
    passthrough_rendition_cache: PassthroughRenditionCache,
    adjustment_page_params: std::collections::HashMap<usize, crate::adjustment::AdjustParams>,
    local_adjust_page_layers:
        std::collections::HashMap<usize, local_adjust_core::LocalAdjustmentLayers>,
    local_adjust_pages: std::collections::HashSet<usize>,
    local_adjust_selected_layers: std::collections::HashMap<usize, usize>,
    local_adjust_generation: std::collections::HashMap<usize, u64>,
    local_adjust_cache: std::collections::HashMap<LocalAdjustResultKey, LocalAdjustCacheEntry>,
    local_adjust_pending: std::collections::HashMap<usize, LocalAdjustRenderPending>,
    export_crop_page_settings: std::collections::HashMap<usize, crate::export_crop::CropSettings>,
    export_crop_pages: std::collections::HashSet<usize>,
    mask_pages: std::collections::HashSet<usize>,
    comic_pages: std::collections::HashSet<usize>,
    conceal_pages: std::collections::HashSet<usize>,
    erase_mask_generation: std::collections::HashMap<usize, u64>,
    conceal_mask_generation: std::collections::HashMap<usize, u64>,
    edit_result_cache: std::collections::HashMap<EditResultKey, EditResultEntry>,
    final_ai_cache: std::collections::HashMap<FinalAiKey, FinalAiEntry>,
    final_ai_pending: std::collections::HashMap<FinalAiKey, FinalAiPending>,
    final_ai_failed: std::collections::HashSet<FinalAiKey>,
    final_composite_cache: FinalCompositeCache,
    final_effect_pending: std::collections::HashMap<FinalCompositeKey, FinalEffectPending>,
    adjustment_cache: std::collections::HashMap<usize, FsCacheEntry>,
    erase_result_cache: std::collections::HashMap<EraseResultKey, EraseResultCacheEntry>,
    erase_preview_cache: std::collections::HashMap<usize, ErasePreviewCacheEntry>,
    erase_base_cache: std::collections::HashMap<usize, std::sync::Arc<egui::ColorImage>>,
    conceal_base_cache: std::collections::HashMap<usize, std::sync::Arc<egui::ColorImage>>,
    conceal_cache: std::collections::HashMap<usize, ConcealCacheEntry>,
    comic_cache: std::collections::HashMap<usize, ComicCacheEntry>,
    comic_bake_pending: std::collections::HashMap<usize, ComicBakePending>,
    erase_inpaint_pending: std::collections::HashMap<
        crate::ui_erase::EraseInpaintPendingKey,
        crate::ui_erase::EraseInpaintPending,
    >,
    ai_classify_cache: std::collections::HashMap<usize, crate::ai::ImageCategory>,
    normalize_ui_states:
        std::collections::HashMap<usize, crate::video::normalize_types::NormalizeUiState>,
    normalize_auto_scan_suppressed: std::collections::HashSet<usize>,
    music_bookmarks: Vec<crate::video_bookmarks::VideoBookmarkMeta>,
    music_bookmarks_loaded_for: Option<PathBuf>,
    last_loop_pos: std::collections::HashMap<usize, (f64, u64)>,
}

#[cfg(windows)]
#[derive(Clone, Copy)]
enum ContextRefSource<'a> {
    Mounted(&'a App),
    AtRest(&'a ViewerContextBundle),
}

/// 1 つの context への読み取り。マウント中なら App のミラーフィールド、
/// そうでなければ bundle を読む。呼び出し側はどちらか知らなくてよい。
#[cfg(windows)]
#[derive(Clone, Copy)]
pub(in crate::app) struct ContextRef<'a> {
    source: ContextRefSource<'a>,
}

#[cfg(windows)]
impl<'a> ContextRef<'a> {
    pub(in crate::app) fn mounted(app: &'a App) -> Self {
        Self {
            source: ContextRefSource::Mounted(app),
        }
    }

    pub(in crate::app) fn at_rest(bundle: &'a ViewerContextBundle) -> Self {
        Self {
            source: ContextRefSource::AtRest(bundle),
        }
    }

    pub(in crate::app) fn fullscreen_idx(self) -> Option<usize> {
        match self.source {
            ContextRefSource::Mounted(app) => app.fullscreen_idx,
            ContextRefSource::AtRest(bundle) => bundle.fullscreen_idx,
        }
    }

    pub(in crate::app) fn items(self) -> &'a [GridItem] {
        match self.source {
            ContextRefSource::Mounted(app) => &app.items,
            ContextRefSource::AtRest(bundle) => &bundle.items,
        }
    }

    pub(in crate::app) fn fs_cache(self) -> &'a ItemsGenerationMap<FsCacheEntry> {
        match self.source {
            ContextRefSource::Mounted(app) => &app.fs_cache,
            ContextRefSource::AtRest(bundle) => &bundle.fs_cache,
        }
    }

    pub(in crate::app) fn viewer_session_last_sync_stamp(self) -> Option<&'a ViewerSyncStamp> {
        match self.source {
            ContextRefSource::Mounted(app) => app.last_viewer_sync_stamp.as_ref(),
            ContextRefSource::AtRest(bundle) => bundle.viewer_session.last_sync_stamp.as_ref(),
        }
    }

    pub(in crate::app) fn viewer_session_detached_window_id(self) -> Option<u64> {
        match self.source {
            ContextRefSource::Mounted(app) => app.detached_viewer_window_id,
            ContextRefSource::AtRest(bundle) => bundle.viewer_session.detached_window_id,
        }
    }

    pub(in crate::app) fn pdf_password_request(self) -> Option<&'a PdfPasswordRequest> {
        match self.source {
            ContextRefSource::Mounted(app) => app.pdf_password_request.as_ref(),
            ContextRefSource::AtRest(bundle) => bundle.pdf_password_request.as_ref(),
        }
    }

    pub(in crate::app) fn current_folder(self) -> Option<&'a Path> {
        match self.source {
            ContextRefSource::Mounted(app) => app.current_folder.as_deref(),
            ContextRefSource::AtRest(bundle) => bundle.current_folder.as_deref(),
        }
    }

    pub(in crate::app) fn items_generation(self) -> u64 {
        match self.source {
            ContextRefSource::Mounted(app) => app.items_generation,
            ContextRefSource::AtRest(bundle) => bundle.items_generation,
        }
    }

    pub(in crate::app) fn video_audio_mode(self) -> Option<usize> {
        match self.source {
            ContextRefSource::Mounted(app) => app.video_audio_mode,
            ContextRefSource::AtRest(bundle) => bundle.video_audio_mode,
        }
    }

    pub(in crate::app) fn video_audio_vst(self) -> Option<&'a VideoAudioVstState> {
        match self.source {
            ContextRefSource::Mounted(app) => app.video_audio_vst.as_ref(),
            ContextRefSource::AtRest(bundle) => bundle.video_audio_vst.as_ref(),
        }
    }

    pub(in crate::app) fn vst3_deferred_media_open(self) -> Option<usize> {
        match self.source {
            ContextRefSource::Mounted(app) => app.vst3_deferred_media_open,
            ContextRefSource::AtRest(bundle) => bundle.vst3_deferred_media_open,
        }
    }

    pub(in crate::app) fn fs_lanczos_cache(self) -> &'a crate::gpu_lanczos::GpuLanczosCache {
        match self.source {
            ContextRefSource::Mounted(app) => &app.fs_lanczos_cache,
            ContextRefSource::AtRest(bundle) => &bundle.fs_lanczos_cache,
        }
    }

    pub(in crate::app) fn selected(self) -> Option<usize> {
        match self.source {
            ContextRefSource::Mounted(app) => app.selected,
            ContextRefSource::AtRest(bundle) => bundle.selected,
        }
    }

    pub(in crate::app) fn bookmark_view_state(self) -> Option<&'a BookmarkViewState> {
        match self.source {
            ContextRefSource::Mounted(app) => app.bookmark_view_state.as_ref(),
            ContextRefSource::AtRest(bundle) => bundle.bookmark_view_state.as_ref(),
        }
    }

    pub(in crate::app) fn archive_source_override(self) -> Option<&'a Path> {
        match self.source {
            ContextRefSource::Mounted(app) => app.archive_source_override.as_deref(),
            ContextRefSource::AtRest(bundle) => bundle.archive_source_override.as_deref(),
        }
    }

    pub(in crate::app) fn music_bookmarks(self) -> &'a [crate::video_bookmarks::VideoBookmarkMeta] {
        match self.source {
            ContextRefSource::Mounted(app) => &app.music_bookmarks,
            ContextRefSource::AtRest(bundle) => &bundle.music_bookmarks,
        }
    }

    pub(in crate::app) fn music_bookmarks_loaded(self) -> bool {
        match self.source {
            ContextRefSource::Mounted(app) => app.music_bookmarks_loaded_for.is_some(),
            ContextRefSource::AtRest(bundle) => bundle.music_bookmarks_loaded_for.is_some(),
        }
    }

    pub(in crate::app) fn normalize_ui_state(
        self,
        idx: usize,
    ) -> Option<crate::video::normalize_types::NormalizeUiState> {
        match self.source {
            ContextRefSource::Mounted(app) => app.normalize_ui_states.get(&idx).copied(),
            ContextRefSource::AtRest(bundle) => bundle.normalize_ui_states.get(&idx).copied(),
        }
    }

    pub(in crate::app) fn final_ai_pending_job_id(self, key: &FinalAiKey) -> Option<u64> {
        match self.source {
            ContextRefSource::Mounted(app) => {
                app.final_ai_pending.get(key).map(|pending| pending.job_id)
            }
            ContextRefSource::AtRest(bundle) => bundle
                .final_ai_pending
                .get(key)
                .map(|pending| pending.job_id),
        }
    }
}

#[cfg(windows)]
pub(in crate::app) struct ContextMut<'a> {
    bundle: &'a mut ViewerContextBundle,
}

#[cfg(windows)]
impl ContextMut<'_> {
    pub(in crate::app) fn as_ref(&self) -> ContextRef<'_> {
        ContextRef::at_rest(self.bundle)
    }

    pub(in crate::app) fn clear_normalize_state(&mut self) {
        self.bundle.clear_normalize_state();
    }
}

#[cfg(windows)]
impl ViewerContextBundle {
    /// この viewer context が所有する非同期 work を、context の terminal retire と同じ
    /// ownership 境界で停止する。
    ///
    /// thumbnail pool だけは condvar 待ちで残留し得るため notify まで必要。その他は有限の
    /// one-shot worker だが、owner の消滅後に CPU / GPU / AI 処理を続ける理由がないので、
    /// 各 worker が既に監視している cancel token を立てる。
    fn cancel_all_context_work(&self) {
        self.cancel_token.store(true, Ordering::Relaxed);
        if let Some(q) = &self.reload_queue {
            q.1.notify_all();
        }
        if let Some(q) = &self.heavy_io_queue {
            q.1.notify_all();
        }
        if let Some(pending) = self.tag_prewarm_pending.as_ref() {
            pending.cancel();
        }
        if let Some(pending) = self.tag_legacy_seed_pending.as_ref() {
            pending.cancel();
        }
        if let Some(pending) = self.metadata_pending.as_ref() {
            pending.cancel();
        }
        if let Some(pending) = self.converted_archive_cache_paths_pending.as_ref() {
            pending.cancel.store(true, Ordering::Relaxed);
        }
        if let Some(pending) = self.folder_nav_pending.as_ref() {
            pending.cancel.store(true, Ordering::Relaxed);
        }
        if let Some(pending) = self.folder_pane_open_pending.as_ref() {
            pending.cancel.store(true, Ordering::Relaxed);
        }
        for pending in self.final_effect_pending.values() {
            pending.cancel.store(true, Ordering::Relaxed);
        }

        for pending in self.fs_pending.values() {
            pending.cancel();
        }
        if let Some(pending) = self.details_meta_pending.as_ref() {
            pending.cancel.store(true, Ordering::Relaxed);
        }
        for pending in self.comic_bake_pending.values() {
            pending.cancel.store(true, Ordering::Relaxed);
        }
        for pending in self.erase_inpaint_pending.values() {
            pending.cancel.store(true, Ordering::Relaxed);
        }
    }
}

#[cfg(windows)]
impl Drop for ViewerContextBundle {
    /// bundle 化したロード複合体 (review-v2.3.0 P2-8/P2-9) の後始末。detached 窓の close /
    /// モード切替の一括 clear / 新メディアによる parked 窓の強制 close では bundle ごと
    /// 破棄されるが、この context の worker pool (通常 5〜14 スレッド) は cancel が立たないと
    /// queue の condvar 待ちで永久残留する (窓の開閉のたびに 1 プールずつ蓄積するスレッド
    /// リーク、review-v2.3.0 hunt P2)。cancel を立てて両キューを notify すれば worker は
    /// 起床 → cancel 検知 → 退出する。swap は参照パターンの destructure なので Drop と
    /// 両立し、空 bundle (`empty()`) の drop は誰も掴んでいない token を立てるだけの no-op。
    fn drop(&mut self) {
        self.cancel_all_context_work();
    }
}

// Tests exercise their own cfg graph, so production must prove this teardown exists explicitly.
#[cfg(all(windows, not(test)))]
const _: () = {
    #[allow(drop_bounds)]
    fn assert_explicit_drop<T: Drop>() {}

    let _ = assert_explicit_drop::<ViewerContextBundle>;
};

#[cfg(windows)]
impl ViewerContextBundle {
    fn set_items_generation(&mut self, items_generation: u64) {
        if self.items_generation != items_generation {
            self.still_seek_thumbnail_pages.clear();
            if let Ok(mut shared) = self.still_seek_thumbnail_pages_shared.write() {
                shared.clear();
            }
        }
        self.items_generation = items_generation;
        self.fs_cache.set_items_generation(items_generation);
        self.fs_pending.set_items_generation(items_generation);
        self.fs_early_dims.set_items_generation(items_generation);
        self.fs_upload_backlog
            .set_items_generation(items_generation);
    }

    fn clear_normalize_state(&mut self) {
        self.normalize_ui_states.clear();
        self.normalize_auto_scan_suppressed.clear();
    }

    fn empty() -> Self {
        // per-context ロード複合体: 空コンテキストは「誰も繋がっていない」fresh channel と
        // token を持つ (App::new と同じ初期状態。この tx を掴む worker は存在しないので
        // rx は常に Empty を返す)。
        let (tx, rx) = mpsc::channel();
        Self {
            address: String::new(),
            current_folder: None,
            favorite_view_context: FavoriteViewContextState::default(),
            navigation_scope: ViewerNavigationScope::Main,
            archive_source_override: None,
            zip_nav: None,
            stack_mode_requested: false,
            stack_view: None,
            stack_showing_flat: false,
            stack_active_rule: None,
            stack_script_error: None,
            stack_toggle_select_path: None,
            items: Vec::new(),
            items_generation: 0,
            visible_indices: Vec::new(),
            facet_name_cache: Vec::new(),
            facet_name_cache_generation: None,
            facet_name_cache_pending: None,
            facet_name_cache_failed_generation: None,
            thumbnails: Vec::new(),
            image_metas: Vec::new(),
            video_thumb_overrides: std::collections::HashMap::new(),
            auto_aspect: crate::auto_aspect::AutoAspectState::default(),
            selected: None,
            grid_click_selection_anchor: None,
            scroll_offset_y: 0.0,
            scroll_to_selected: false,
            pending_grid_scroll: None,
            requested: std::collections::HashMap::new(),
            idle_upgrade_cache_bypass_ineligible: std::collections::HashSet::new(),
            keep_range: (0, 0),
            keep_set: std::collections::HashSet::new(),
            still_seek_thumbnail_pages: std::collections::HashSet::new(),
            still_seek_thumbnail_pages_shared: Arc::new(std::sync::RwLock::new(
                std::collections::HashSet::new(),
            )),
            thumbnail_eviction_generation: None,
            details_thumb_suppression_applied: false,
            details_hover_thumb_idx: None,
            details_hover_thumb_viewport_open: false,
            texture_backlog: Vec::new(),
            details_order: Vec::new(),
            details_order_revision: 0,
            details_cell_content_revisions: DetailsCellContentRevisions::default(),
            details_tag_prewarm_indices: Vec::new(),
            details_lazy_meta: std::collections::HashMap::new(),
            details_meta_pending: None,
            details_lazy_visible_revision: 0,
            details_image_dims_state: LazyColumnState::Disabled,
            metadata_cache: std::collections::HashMap::new(),
            exif_cache: std::collections::HashMap::new(),
            xmp_cache: std::collections::HashMap::new(),
            xmp_panorama_info: std::collections::HashMap::new(),
            metadata_pending: None,
            tags_cache: std::collections::HashMap::new(),
            tag_prewarm_pending: None,
            tag_prewarm_queued: std::collections::HashSet::new(),
            tag_legacy_seed_pending: None,
            pending_finalize: std::collections::HashSet::new(),
            tx,
            rx,
            cancel_token: Arc::new(AtomicBool::new(false)),
            reload_queue: None,
            heavy_io_queue: None,
            scroll_hint: Arc::new(AtomicUsize::new(0)),
            visible_end_shared: Arc::new(AtomicUsize::new(0)),
            keep_start_shared: Arc::new(AtomicUsize::new(0)),
            keep_end_shared: Arc::new(AtomicUsize::new(0)),
            last_vis_range: (0, 0),
            vis_settle_at: None,
            vis_first_logged: false,
            vis_all_logged: false,
            folder_nav_pending: None,
            folder_pane_open_pending: None,
            pending_folder_nav_steps: 0,
            pending_folder_nav_mode: FolderNavMode::Grid,
            search_filter: None,
            search_filter_origin_folder: None,
            checked: std::collections::HashSet::new(),
            rotation_cache: std::collections::HashMap::new(),
            page_dims_cache: crate::page_dims::PageDimsCache::default(),
            spread_display_units_cache: crate::ui_fullscreen::SpreadDisplayUnitsCache::default(),
            rating_cache: std::collections::HashMap::new(),
            rating_filter_suppressed_at: None,
            rating_session_write_seen_generation: 0,
            metadata_import_refresh_index: None,
            current_folder_rating_cache: None,
            current_folder_last_mtime: None,
            current_folder_signature: None,
            folder_pin_map: std::collections::HashMap::new(),
            converted_archive_cache_paths: std::collections::HashMap::new(),
            converted_archive_pin_root_states: std::collections::HashMap::new(),
            converted_archive_cache_paths_pending: None,
            current_color_cache_map: None,
            current_color_catalog: None,
            vst3_deferred_media_open: None,
            fullscreen_idx: None,
            fullscreen_page_slice: crate::page_split::PageSlice::Full,
            viewer_session: ViewerSession::default(),
            native_video_in_window_active: false,
            video_audio_mode: None,
            video_audio_vst: None,
            video_audio_mode_entry_target: None,
            video_audio_exit_pending: None,
            panorama_state: None,
            panorama_intent: crate::panorama::PanoramaSessionIntent::default(),
            fs_info_panel: crate::ui_helpers::FullscreenInfoPanelState::default(),
            pano_toast_shown_for_current_fs: false,
            analysis_mode: false,
            analysis_hover_color: None,
            analysis_pinned_color: None,
            analysis_grayscale: false,
            analysis_mosaic_grid: false,
            analysis_filter_mag: 0,
            analysis_guide_drag: None,
            view_trim_mode: false,
            view_trim_apply_mode: crate::view_trim::ViewTrimApplyMode::default(),
            view_trim_page_apply_root_idx: None,
            view_trim_page_spread_separate: false,
            view_trim_book_settings: crate::view_trim::ViewTrimBookSettings::default(),
            view_trim_page_overrides: std::collections::HashMap::new(),
            view_trim_dirty_page_overrides: std::collections::HashSet::new(),
            view_trim_save_pending: false,
            fs_cache: ItemsGenerationMap::new("fs_cache"),
            fs_lanczos_cache: crate::gpu_lanczos::GpuLanczosCache::default(),
            fs_margin_bbox_cache: std::collections::HashMap::new(),
            input_generation: std::collections::HashMap::new(),
            fs_pending: ItemsGenerationMap::with_discard("fs_pending", cancel_fs_pending_value),
            fullscreen_pdf_promotion: FullscreenPdfPromotionState::default(),
            fs_pdf_display_target: None,
            fs_early_dims: ItemsGenerationMap::new("fs_early_dims"),
            fs_upload_backlog: FsUploadBacklog::new("fs_upload_backlog", fs_upload_backlog_idx),
            top_level_grid_view: top_level_grid_view::TopLevelGridView::default(),
            snapshot: None,
            global_search_subfolder_restore: None,
            favsearch_subfolder_restore: None,
            items_are_global_search_view: false,
            items_are_tag_view: false,
            items_are_reading_history_view: false,
            items_are_bookmark_view: false,
            items_are_rating_view: false,
            items_are_subfolder_expansion_view: false,
            items_are_smart_folder_view: false,
            items_are_drive_list: false,
            reading_history_return_from: None,
            bookmark_view_state: None,
            bookmark_open_pending: None,
            fs_open_intent_from_grid: false,
            video_presentation_transition: PresentationTransitionOwner::default(),
            fs_zoom: 1.0,
            fs_pan: egui::Vec2::ZERO,
            fs_zoom_active: false,
            fs_zoom_aiming: false,
            fs_zoom_factor: 1.0,
            fs_zoom_pdf_rerender_idx: None,
            fs_zoom_pdf_rerender_zoom: 1.0,
            fs_pan_drag_start: None,
            fs_vertical_scroll: 0.0,
            fs_seek_drag_active: false,
            fs_seek_gesture: crate::ui_fullscreen::StillSeekGesture::Idle,
            fs_seek_overlay_visible: false,
            fs_vertical_cache_keep_set: std::collections::HashSet::new(),
            continuous_page_transitions: std::collections::HashMap::new(),
            fs_free_rotation: 0.0,
            fs_rotation_drag_start: None,
            analysis_zoom: 1.0,
            analysis_pan: egui::Vec2::ZERO,
            analysis_pan_drag_start: None,
            analysis_overlay_cache: None,
            analysis_hist_cache: None,
            analysis_sv_cache: None,
            spread_mode: crate::settings::SpreadMode::default(),
            spread_shift_anchor_idx: None,
            reading_flow: crate::settings::ReadingFlow::default(),
            reading_direction: crate::settings::ReadingDirection::default(),
            slideshow_playing: false,
            slideshow_next_at: std::time::Instant::now(),
            slideshow_anchor_idx: None,
            continuous_reading_scroll_transition: None,
            slideshow_scroll_range_cache: None,
            pdf_password_request: None,
            pdf_current_password: None,
            pdf_password_pending_save: None,
            pdf_enumerate_pending: None,
            zip_enumerate_pending: None,
            fs_nav_after_pdf_enumerate: None,
            pending_auto_fs_open: false,
            pending_return_to_parent: false,
            pdf_placeholder_count: None,
            viewer_navigation_caches: crate::ui_fullscreen::ViewerNavigationCaches::default(),
            fs_nav_locked_gen: None,
            fs_nav_dropped_block_signature: None,
            fs_nav_dropped_block_count: 0,
            fs_load_skip_signature: None,
            fs_holdover_tex: None,
            fs_boundary_hint: None,
            virtual_folder_writeback: None,
            pdf_prefetch_grace_until: None,
            thumb_pixels: std::collections::HashMap::new(),
            thumb_edit_preview_layers: std::collections::HashMap::new(),
            thumb_edit_preview_keys: std::collections::HashMap::new(),
            thumb_adjust_tex: std::collections::HashMap::new(),
            passthrough_rendition_cache: PassthroughRenditionCache::default(),
            adjustment_page_params: std::collections::HashMap::new(),
            local_adjust_page_layers: std::collections::HashMap::new(),
            local_adjust_pages: std::collections::HashSet::new(),
            local_adjust_selected_layers: std::collections::HashMap::new(),
            local_adjust_generation: std::collections::HashMap::new(),
            local_adjust_cache: std::collections::HashMap::new(),
            local_adjust_pending: std::collections::HashMap::new(),
            export_crop_page_settings: std::collections::HashMap::new(),
            export_crop_pages: std::collections::HashSet::new(),
            mask_pages: std::collections::HashSet::new(),
            comic_pages: std::collections::HashSet::new(),
            conceal_pages: std::collections::HashSet::new(),
            erase_mask_generation: std::collections::HashMap::new(),
            conceal_mask_generation: std::collections::HashMap::new(),
            edit_result_cache: std::collections::HashMap::new(),
            final_ai_cache: std::collections::HashMap::new(),
            final_ai_pending: std::collections::HashMap::new(),
            final_ai_failed: std::collections::HashSet::new(),
            final_composite_cache: FinalCompositeCache::default(),
            final_effect_pending: std::collections::HashMap::new(),
            adjustment_cache: std::collections::HashMap::new(),
            erase_result_cache: std::collections::HashMap::new(),
            erase_preview_cache: std::collections::HashMap::new(),
            erase_base_cache: std::collections::HashMap::new(),
            conceal_base_cache: std::collections::HashMap::new(),
            conceal_cache: std::collections::HashMap::new(),
            comic_cache: std::collections::HashMap::new(),
            comic_bake_pending: std::collections::HashMap::new(),
            erase_inpaint_pending: std::collections::HashMap::new(),
            ai_classify_cache: std::collections::HashMap::new(),
            normalize_ui_states: std::collections::HashMap::new(),
            normalize_auto_scan_suppressed: std::collections::HashSet::new(),
            music_bookmarks: Vec::new(),
            music_bookmarks_loaded_for: None,
            last_loop_pos: std::collections::HashMap::new(),
        }
    }
}

impl App {
    #[cfg(windows)]
    pub(in crate::app) fn pause_mounted_background_work_keep_current_frame(&mut self) {
        self.slideshow_playing = false;
        self.continuous_reading_scroll_transition = None;
        self.slideshow_scroll_range_cache = None;
        self.fs_seek_drag_active = false;
        self.fs_seek_gesture.interrupt();
        self.fs_seek_overlay_visible = false;
        self.pending_auto_fs_open = false;
        self.pending_return_to_parent = false;
        self.fs_nav_after_pdf_enumerate = None;
        self.fs_nav_locked_gen = None;
        self.fs_holdover_tex = None;
        self.fs_nav_dropped_block_signature = None;
        self.fs_nav_dropped_block_count = 0;
        self.continuous_page_transitions.clear();
        self.pdf_enumerate_pending = None;
        self.zip_enumerate_pending = None;
        if let Some(pending) = self.folder_nav_pending.take() {
            pending.cancel.store(true, Ordering::Relaxed);
        }
        if let Some(pending) = self.folder_pane_open_pending.take() {
            pending.cancel.store(true, Ordering::Relaxed);
        }
        self.pending_folder_nav_steps = 0;
        self.pending_folder_nav_mode = FolderNavMode::Grid;
        for (_, pending) in self.fs_pending.drain() {
            pending.cancel();
        }
        self.texture_backlog.clear();
        for pending in self.final_ai_pending.values() {
            pending.cancel.store(true, Ordering::Relaxed);
        }
        self.final_ai_pending.clear();
        for (_, pending) in self.local_adjust_pending.drain() {
            pending.cancel.store(true, Ordering::Relaxed);
        }
        for (_, pending) in self.comic_bake_pending.drain() {
            pending.cancel.store(true, Ordering::Relaxed);
        }
        for (_, pending) in self.erase_inpaint_pending.drain() {
            pending.cancel.store(true, Ordering::Relaxed);
        }
    }

    #[cfg(windows)]
    pub(in crate::app) fn activate_mounted_as_independent_detached(&mut self, window_id: u64) {
        self.viewer_presentation = ViewerPresentation::DetachedWindow;
        self.detached_viewer_window_id = Some(window_id);
        self.detached_viewer_independent_active = true;
        self.detached_viewer_open_next_still_detached_once = false;
        self.last_viewer_sync_stamp = None;
        self.fs_open_intent_from_grid = false;
        self.pending_auto_fs_open = false;
        self.pending_return_to_parent = false;
        self.pdf_prefetch_grace_until = None;
    }

    #[cfg(windows)]
    pub(in crate::app) fn become_mounted_independent_detached_viewer(
        &mut self,
        window_id: u64,
        idx: usize,
    ) {
        self.selected = Some(idx);
        self.fullscreen_idx = Some(idx);
        // 新しく開くページは分割方向の最初の半分から。ここで残すと前のページの
        // 「右半分を見ていた」が別のページへ引き継がれる。
        self.fullscreen_page_slice = crate::page_split::PageSlice::Full;
        self.native_video_in_window_active = false;
        self.activate_mounted_as_independent_detached(window_id);
    }

    #[cfg(windows)]
    pub(in crate::app) fn swap_viewer_context_bundle(&mut self, bundle: &mut ViewerContextBundle) {
        // path-keyed DB 更新は context-global。退避する側も復元する側も、idx-keyed cache を
        // その時点の最新世代へ揃えてから ownership を渡す。この関数はnavigationではなく
        // ownership交換のプリミティブなので、同期に伴う表示再構築でApp-globalなfacet
        // scope / suppressionを変更してはならない。
        if self.sync_current_context_rating_session_writes() {
            self.rebuild_visible_indices_preserving_facet_scope();
        }
        let favorite_view_now = std::time::Instant::now();
        self.capture_active_favorite_view_change_at(favorite_view_now);
        let favorite_view_inherited =
            crate::settings::FavoriteViewState::from_settings(&self.settings);
        self.settings.clear_favorite_view_overlay();
        #[cfg(test)]
        panic_at_viewer_context_swap_interior_for_test();
        macro_rules! swap_field {
            ($field:ident) => {
                std::mem::swap(&mut self.$field, $field);
            };
        }

        let ViewerContextBundle {
            address,
            current_folder,
            favorite_view_context,
            navigation_scope,
            archive_source_override,
            zip_nav,
            stack_mode_requested,
            stack_view,
            stack_showing_flat,
            stack_active_rule,
            stack_script_error,
            stack_toggle_select_path,
            items,
            items_generation,
            visible_indices,
            facet_name_cache,
            facet_name_cache_generation,
            facet_name_cache_pending,
            facet_name_cache_failed_generation,
            thumbnails,
            image_metas,
            video_thumb_overrides,
            auto_aspect,
            selected,
            grid_click_selection_anchor,
            scroll_offset_y,
            scroll_to_selected,
            pending_grid_scroll,
            requested,
            idle_upgrade_cache_bypass_ineligible,
            keep_range,
            keep_set,
            still_seek_thumbnail_pages,
            still_seek_thumbnail_pages_shared,
            thumbnail_eviction_generation,
            details_thumb_suppression_applied,
            details_hover_thumb_idx,
            details_hover_thumb_viewport_open,
            texture_backlog,
            details_order,
            details_order_revision,
            details_cell_content_revisions,
            details_tag_prewarm_indices,
            details_lazy_meta,
            details_meta_pending,
            details_lazy_visible_revision,
            details_image_dims_state,
            metadata_cache,
            exif_cache,
            xmp_cache,
            xmp_panorama_info,
            metadata_pending,
            tags_cache,
            tag_prewarm_pending,
            tag_prewarm_queued,
            tag_legacy_seed_pending,
            pending_finalize,
            tx,
            rx,
            cancel_token,
            reload_queue,
            heavy_io_queue,
            scroll_hint,
            visible_end_shared,
            keep_start_shared,
            keep_end_shared,
            last_vis_range,
            vis_settle_at,
            vis_first_logged,
            vis_all_logged,
            folder_nav_pending,
            folder_pane_open_pending,
            pending_folder_nav_steps,
            pending_folder_nav_mode,
            search_filter,
            search_filter_origin_folder,
            checked,
            rotation_cache,
            page_dims_cache,
            spread_display_units_cache,
            rating_cache,
            rating_filter_suppressed_at,
            rating_session_write_seen_generation,
            metadata_import_refresh_index,
            current_folder_rating_cache,
            current_folder_last_mtime,
            current_folder_signature,
            folder_pin_map,
            converted_archive_cache_paths,
            converted_archive_pin_root_states,
            converted_archive_cache_paths_pending,
            current_color_cache_map,
            current_color_catalog,
            vst3_deferred_media_open,
            fullscreen_idx,
            fullscreen_page_slice,
            viewer_session,
            native_video_in_window_active,
            video_audio_mode,
            video_audio_vst,
            video_audio_mode_entry_target,
            video_audio_exit_pending,
            panorama_state,
            panorama_intent,
            fs_info_panel,
            pano_toast_shown_for_current_fs,
            analysis_mode,
            analysis_hover_color,
            analysis_pinned_color,
            analysis_grayscale,
            analysis_mosaic_grid,
            analysis_filter_mag,
            analysis_guide_drag,
            view_trim_mode,
            view_trim_apply_mode,
            view_trim_page_apply_root_idx,
            view_trim_page_spread_separate,
            view_trim_book_settings,
            view_trim_page_overrides,
            view_trim_dirty_page_overrides,
            view_trim_save_pending,
            fs_cache,
            fs_lanczos_cache,
            fs_margin_bbox_cache,
            input_generation,
            fs_pending,
            fullscreen_pdf_promotion,
            fs_pdf_display_target,
            fs_early_dims,
            fs_upload_backlog,
            top_level_grid_view,
            snapshot,
            global_search_subfolder_restore,
            favsearch_subfolder_restore,
            items_are_global_search_view,
            items_are_tag_view,
            items_are_reading_history_view,
            items_are_bookmark_view,
            items_are_rating_view,
            items_are_subfolder_expansion_view,
            items_are_smart_folder_view,
            items_are_drive_list,
            reading_history_return_from,
            bookmark_view_state,
            bookmark_open_pending,
            fs_open_intent_from_grid,
            video_presentation_transition,
            fs_zoom,
            fs_pan,
            fs_zoom_active,
            fs_zoom_aiming,
            fs_zoom_factor,
            fs_zoom_pdf_rerender_idx,
            fs_zoom_pdf_rerender_zoom,
            fs_pan_drag_start,
            fs_vertical_scroll,
            fs_seek_drag_active,
            fs_seek_gesture,
            fs_seek_overlay_visible,
            fs_vertical_cache_keep_set,
            continuous_page_transitions,
            fs_free_rotation,
            fs_rotation_drag_start,
            analysis_zoom,
            analysis_pan,
            analysis_pan_drag_start,
            analysis_overlay_cache,
            analysis_hist_cache,
            analysis_sv_cache,
            spread_mode,
            spread_shift_anchor_idx,
            reading_flow,
            reading_direction,
            slideshow_playing,
            slideshow_next_at,
            slideshow_anchor_idx,
            continuous_reading_scroll_transition,
            slideshow_scroll_range_cache,
            pdf_password_request,
            pdf_current_password,
            pdf_password_pending_save,
            pdf_enumerate_pending,
            zip_enumerate_pending,
            fs_nav_after_pdf_enumerate,
            pending_auto_fs_open,
            pending_return_to_parent,
            pdf_placeholder_count,
            viewer_navigation_caches,
            fs_nav_locked_gen,
            fs_nav_dropped_block_signature,
            fs_nav_dropped_block_count,
            fs_load_skip_signature,
            fs_holdover_tex,
            fs_boundary_hint,
            virtual_folder_writeback,
            pdf_prefetch_grace_until,
            thumb_pixels,
            thumb_edit_preview_layers,
            thumb_edit_preview_keys,
            thumb_adjust_tex,
            passthrough_rendition_cache,
            adjustment_page_params,
            local_adjust_page_layers,
            local_adjust_pages,
            local_adjust_selected_layers,
            local_adjust_generation,
            local_adjust_cache,
            local_adjust_pending,
            export_crop_page_settings,
            export_crop_pages,
            mask_pages,
            comic_pages,
            conceal_pages,
            erase_mask_generation,
            conceal_mask_generation,
            edit_result_cache,
            final_ai_cache,
            final_ai_pending,
            final_ai_failed,
            final_composite_cache,
            final_effect_pending,
            adjustment_cache,
            erase_result_cache,
            erase_preview_cache,
            erase_base_cache,
            conceal_base_cache,
            conceal_cache,
            comic_cache,
            comic_bake_pending,
            erase_inpaint_pending,
            ai_classify_cache,
            normalize_ui_states,
            normalize_auto_scan_suppressed,
            music_bookmarks,
            music_bookmarks_loaded_for,
            last_loop_pos,
        } = bundle;

        swap_field!(address);
        swap_field!(current_folder);
        swap_field!(favorite_view_context);
        swap_field!(navigation_scope);
        swap_field!(archive_source_override);
        swap_field!(zip_nav);
        swap_field!(stack_mode_requested);
        swap_field!(stack_view);
        swap_field!(stack_showing_flat);
        swap_field!(stack_active_rule);
        swap_field!(stack_script_error);
        swap_field!(stack_toggle_select_path);
        swap_field!(items);
        swap_field!(items_generation);
        swap_field!(visible_indices);
        swap_field!(facet_name_cache);
        swap_field!(facet_name_cache_generation);
        swap_field!(facet_name_cache_pending);
        swap_field!(facet_name_cache_failed_generation);
        swap_field!(thumbnails);
        swap_field!(image_metas);
        swap_field!(video_thumb_overrides);
        swap_field!(auto_aspect);
        swap_field!(selected);
        swap_field!(grid_click_selection_anchor);
        swap_field!(scroll_offset_y);
        swap_field!(scroll_to_selected);
        swap_field!(pending_grid_scroll);
        swap_field!(requested);
        swap_field!(idle_upgrade_cache_bypass_ineligible);
        swap_field!(keep_range);
        swap_field!(keep_set);
        swap_field!(still_seek_thumbnail_pages);
        swap_field!(still_seek_thumbnail_pages_shared);
        swap_field!(thumbnail_eviction_generation);
        swap_field!(details_thumb_suppression_applied);
        swap_field!(details_hover_thumb_idx);
        swap_field!(details_hover_thumb_viewport_open);
        swap_field!(texture_backlog);
        swap_field!(details_order);
        swap_field!(details_order_revision);
        swap_field!(details_cell_content_revisions);
        swap_field!(details_tag_prewarm_indices);
        swap_field!(details_lazy_meta);
        swap_field!(details_meta_pending);
        swap_field!(details_lazy_visible_revision);
        swap_field!(details_image_dims_state);
        swap_field!(metadata_cache);
        swap_field!(exif_cache);
        swap_field!(xmp_cache);
        swap_field!(xmp_panorama_info);
        swap_field!(metadata_pending);
        swap_field!(tags_cache);
        swap_field!(tag_prewarm_pending);
        swap_field!(tag_prewarm_queued);
        swap_field!(tag_legacy_seed_pending);
        swap_field!(pending_finalize);
        // per-context ロード複合体 (review-v2.3.0 P2-8/P2-9)。channel/token/キューが
        // コンテキストと一緒に移動するので、requested / pending_finalize の bookkeeping は
        // swap 後もそのまま信用できる (末尾の clear は不要になった)。
        swap_field!(tx);
        swap_field!(rx);
        swap_field!(cancel_token);
        swap_field!(reload_queue);
        swap_field!(heavy_io_queue);
        swap_field!(scroll_hint);
        swap_field!(visible_end_shared);
        swap_field!(keep_start_shared);
        swap_field!(keep_end_shared);
        swap_field!(last_vis_range);
        swap_field!(vis_settle_at);
        swap_field!(vis_first_logged);
        swap_field!(vis_all_logged);
        swap_field!(folder_nav_pending);
        swap_field!(folder_pane_open_pending);
        swap_field!(pending_folder_nav_steps);
        swap_field!(pending_folder_nav_mode);
        swap_field!(search_filter);
        swap_field!(search_filter_origin_folder);
        swap_field!(checked);
        swap_field!(rotation_cache);
        swap_field!(page_dims_cache);
        swap_field!(spread_display_units_cache);
        swap_field!(rating_cache);
        swap_field!(rating_session_write_seen_generation);
        swap_field!(metadata_import_refresh_index);
        swap_field!(current_folder_rating_cache);
        swap_field!(current_folder_last_mtime);
        swap_field!(current_folder_signature);
        swap_field!(folder_pin_map);
        swap_field!(converted_archive_cache_paths);
        swap_field!(converted_archive_pin_root_states);
        swap_field!(converted_archive_cache_paths_pending);
        swap_field!(current_color_cache_map);
        swap_field!(current_color_catalog);
        // VST3 deferred open は fullscreen_idx / items と同じ context ownership。
        // (review-v2.3.0 追補 BA-7: vst3 deferred)
        swap_field!(vst3_deferred_media_open);
        swap_field!(fullscreen_idx);
        // 左右は元ページと同じ所有。片方だけ残すと別 viewer の半分が見える。
        swap_field!(fullscreen_page_slice);
        viewer_session.swap_with_mounted(
            &mut self.viewer_presentation,
            &mut self.last_viewer_sync_stamp,
            &mut self.detached_viewer_independent_active,
            &mut self.detached_viewer_open_next_still_detached_once,
            &mut self.detached_viewer_window_id,
        );
        swap_field!(native_video_in_window_active);
        swap_field!(video_audio_mode);
        swap_field!(video_audio_vst);
        swap_field!(video_audio_mode_entry_target);
        swap_field!(video_audio_exit_pending);
        swap_field!(panorama_state);
        swap_field!(panorama_intent);
        swap_field!(fs_info_panel);
        swap_field!(pano_toast_shown_for_current_fs);
        swap_field!(analysis_mode);
        swap_field!(analysis_hover_color);
        swap_field!(analysis_pinned_color);
        swap_field!(analysis_grayscale);
        swap_field!(analysis_mosaic_grid);
        swap_field!(analysis_filter_mag);
        swap_field!(analysis_guide_drag);
        swap_field!(view_trim_mode);
        swap_field!(view_trim_apply_mode);
        swap_field!(view_trim_page_apply_root_idx);
        swap_field!(view_trim_page_spread_separate);
        swap_field!(view_trim_book_settings);
        swap_field!(view_trim_page_overrides);
        swap_field!(view_trim_dirty_page_overrides);
        swap_field!(view_trim_save_pending);
        swap_field!(fs_cache);
        swap_field!(fs_lanczos_cache);
        swap_field!(fs_margin_bbox_cache);
        swap_field!(input_generation);
        swap_field!(fs_pending);
        swap_field!(fullscreen_pdf_promotion);
        swap_field!(fs_pdf_display_target);
        swap_field!(fs_early_dims);
        swap_field!(fs_upload_backlog);
        swap_field!(top_level_grid_view);
        swap_field!(snapshot);
        swap_field!(rating_filter_suppressed_at);
        swap_field!(global_search_subfolder_restore);
        swap_field!(favsearch_subfolder_restore);
        swap_field!(items_are_global_search_view);
        swap_field!(items_are_tag_view);
        swap_field!(items_are_reading_history_view);
        swap_field!(items_are_bookmark_view);
        swap_field!(items_are_rating_view);
        swap_field!(items_are_subfolder_expansion_view);
        swap_field!(items_are_smart_folder_view);
        swap_field!(items_are_drive_list);
        swap_field!(reading_history_return_from);
        swap_field!(bookmark_view_state);
        swap_field!(bookmark_open_pending);
        swap_field!(fs_open_intent_from_grid);
        swap_field!(video_presentation_transition);
        swap_field!(fs_zoom);
        swap_field!(fs_pan);
        swap_field!(fs_zoom_active);
        swap_field!(fs_zoom_aiming);
        swap_field!(fs_zoom_factor);
        swap_field!(fs_zoom_pdf_rerender_idx);
        swap_field!(fs_zoom_pdf_rerender_zoom);
        swap_field!(fs_pan_drag_start);
        swap_field!(fs_vertical_scroll);
        swap_field!(fs_seek_drag_active);
        swap_field!(fs_seek_gesture);
        swap_field!(fs_seek_overlay_visible);
        swap_field!(fs_vertical_cache_keep_set);
        swap_field!(continuous_page_transitions);
        swap_field!(fs_free_rotation);
        swap_field!(fs_rotation_drag_start);
        swap_field!(analysis_zoom);
        swap_field!(analysis_pan);
        swap_field!(analysis_pan_drag_start);
        swap_field!(analysis_overlay_cache);
        swap_field!(analysis_hist_cache);
        swap_field!(analysis_sv_cache);
        swap_field!(spread_mode);
        swap_field!(spread_shift_anchor_idx);
        swap_field!(reading_flow);
        swap_field!(reading_direction);
        swap_field!(slideshow_playing);
        swap_field!(slideshow_next_at);
        swap_field!(slideshow_anchor_idx);
        swap_field!(continuous_reading_scroll_transition);
        swap_field!(slideshow_scroll_range_cache);
        swap_field!(pdf_password_request);
        swap_field!(pdf_current_password);
        swap_field!(pdf_password_pending_save);
        swap_field!(pdf_enumerate_pending);
        swap_field!(zip_enumerate_pending);
        swap_field!(fs_nav_after_pdf_enumerate);
        swap_field!(pending_auto_fs_open);
        swap_field!(pending_return_to_parent);
        swap_field!(pdf_placeholder_count);
        swap_field!(viewer_navigation_caches);
        swap_field!(fs_nav_locked_gen);
        swap_field!(fs_nav_dropped_block_signature);
        swap_field!(fs_nav_dropped_block_count);
        swap_field!(fs_load_skip_signature);
        swap_field!(fs_holdover_tex);
        swap_field!(fs_boundary_hint);
        swap_field!(virtual_folder_writeback);
        swap_field!(pdf_prefetch_grace_until);
        swap_field!(thumb_pixels);
        swap_field!(thumb_edit_preview_layers);
        swap_field!(thumb_edit_preview_keys);
        swap_field!(thumb_adjust_tex);
        swap_field!(passthrough_rendition_cache);
        swap_field!(adjustment_page_params);
        swap_field!(local_adjust_page_layers);
        swap_field!(local_adjust_pages);
        swap_field!(local_adjust_selected_layers);
        swap_field!(local_adjust_generation);
        swap_field!(local_adjust_cache);
        swap_field!(local_adjust_pending);
        swap_field!(export_crop_page_settings);
        swap_field!(export_crop_pages);
        swap_field!(mask_pages);
        swap_field!(comic_pages);
        swap_field!(conceal_pages);
        swap_field!(erase_mask_generation);
        swap_field!(conceal_mask_generation);
        swap_field!(edit_result_cache);
        swap_field!(final_ai_cache);
        swap_field!(final_ai_pending);
        swap_field!(final_ai_failed);
        swap_field!(final_composite_cache);
        swap_field!(final_effect_pending);
        swap_field!(adjustment_cache);
        swap_field!(erase_result_cache);
        swap_field!(erase_preview_cache);
        swap_field!(erase_base_cache);
        swap_field!(conceal_base_cache);
        swap_field!(conceal_cache);
        swap_field!(comic_cache);
        swap_field!(comic_bake_pending);
        swap_field!(erase_inpaint_pending);
        swap_field!(ai_classify_cache);
        swap_field!(normalize_ui_states);
        swap_field!(normalize_auto_scan_suppressed);
        swap_field!(music_bookmarks);
        swap_field!(music_bookmarks_loaded_for);
        swap_field!(last_loop_pos);

        // 旧実装はここで requested / pending_finalize を無条件 clear していた (worker queue が
        // App-global で、swap 後の bookkeeping を信用できなかったため)。detached 窓が 1 枚でも
        // あると mount/unmount + parked poll の swap が毎フレーム走るので、main の bookkeeping が
        // 毎フレーム消え、Pending サムネがフレームごとに重複エンキュー/重複デコードされる
        // churn になっていた (review-v2.3.0 P2-8)。channel/token/キューを bundle 化した現在は
        // bookkeeping がコンテキストと一緒に移動するため clear 不要。
        if self.sync_current_context_rating_session_writes() {
            self.rebuild_visible_indices_preserving_facet_scope();
        }
        let favorite_path = self.effective_folder();
        self.transition_favorite_view_for_path_with_inherited_at(
            favorite_path.as_deref(),
            favorite_view_now,
            Some(favorite_view_inherited),
        );
    }

    /// legacy/unbundled viewer 用に、現在 context を main と独立 viewer に分割する。
    ///
    /// main grid が使う一覧 identity / worker 複合体は main に残し、viewer の一時状態だけを
    /// 戻り値へ移す。ParkedLive media と、通常画像を grid から always-new detached viewer
    /// として開く境界の両方がこの primitive を使う。
    ///
    /// `ViewerContextBundle` の全 field を destructure して 3 分類するため、field 追加時は
    /// コンパイルエラーになり、空 bundle + allowlist 方式の状態喪失を再発させない。
    /// (review-v2.3.0 追補4: live-park main 文脈保持)
    #[cfg(windows)]
    pub(in crate::app) fn split_current_context_preserving_main_grid(
        &mut self,
    ) -> Box<ViewerContextBundle> {
        macro_rules! duplicate_for_parked {
            ($($field:ident),+ $(,)?) => {
                $(*$field = self.$field.clone();)+
            };
        }
        macro_rules! move_to_parked {
            ($($field:ident),+ $(,)?) => {
                $(std::mem::swap(&mut self.$field, $field);)+
            };
        }
        macro_rules! keep_in_main {
            ($($field:ident),+ $(,)?) => {
                $(let _ = $field;)+
            };
        }

        let mut parked = Box::new(ViewerContextBundle::empty());
        let ViewerContextBundle {
            address,
            current_folder,
            favorite_view_context,
            navigation_scope,
            archive_source_override,
            zip_nav,
            stack_mode_requested,
            stack_view,
            stack_showing_flat,
            stack_active_rule,
            stack_script_error,
            stack_toggle_select_path,
            items,
            items_generation,
            visible_indices,
            facet_name_cache,
            facet_name_cache_generation,
            facet_name_cache_pending,
            facet_name_cache_failed_generation,
            thumbnails,
            image_metas,
            video_thumb_overrides,
            auto_aspect,
            selected,
            grid_click_selection_anchor,
            scroll_offset_y,
            scroll_to_selected,
            pending_grid_scroll,
            requested,
            metadata_import_refresh_index,
            idle_upgrade_cache_bypass_ineligible,
            keep_range,
            keep_set,
            still_seek_thumbnail_pages,
            still_seek_thumbnail_pages_shared,
            thumbnail_eviction_generation,
            details_thumb_suppression_applied,
            details_hover_thumb_idx,
            details_hover_thumb_viewport_open,
            texture_backlog,
            details_order,
            details_order_revision,
            details_cell_content_revisions,
            details_tag_prewarm_indices,
            details_lazy_meta,
            details_meta_pending,
            details_lazy_visible_revision,
            details_image_dims_state,
            metadata_cache,
            exif_cache,
            xmp_cache,
            xmp_panorama_info,
            metadata_pending,
            tags_cache,
            tag_prewarm_pending,
            tag_prewarm_queued,
            tag_legacy_seed_pending,
            pending_finalize,
            tx,
            rx,
            cancel_token,
            reload_queue,
            heavy_io_queue,
            scroll_hint,
            visible_end_shared,
            keep_start_shared,
            keep_end_shared,
            last_vis_range,
            vis_settle_at,
            vis_first_logged,
            vis_all_logged,
            folder_nav_pending,
            folder_pane_open_pending,
            pending_folder_nav_steps,
            pending_folder_nav_mode,
            search_filter,
            search_filter_origin_folder,
            checked,
            rotation_cache,
            page_dims_cache,
            spread_display_units_cache,
            rating_cache,
            rating_filter_suppressed_at,
            rating_session_write_seen_generation,
            current_folder_rating_cache,
            current_folder_last_mtime,
            current_folder_signature,
            folder_pin_map,
            converted_archive_cache_paths,
            converted_archive_pin_root_states,
            converted_archive_cache_paths_pending,
            current_color_cache_map,
            current_color_catalog,
            vst3_deferred_media_open,
            fullscreen_idx,
            fullscreen_page_slice,
            viewer_session,
            native_video_in_window_active,
            video_audio_mode,
            video_audio_vst,
            video_audio_mode_entry_target,
            video_audio_exit_pending,
            panorama_state,
            panorama_intent,
            fs_info_panel,
            pano_toast_shown_for_current_fs,
            analysis_mode,
            analysis_hover_color,
            analysis_pinned_color,
            analysis_grayscale,
            analysis_mosaic_grid,
            analysis_filter_mag,
            analysis_guide_drag,
            view_trim_mode,
            view_trim_apply_mode,
            view_trim_page_apply_root_idx,
            view_trim_page_spread_separate,
            view_trim_book_settings,
            view_trim_page_overrides,
            view_trim_dirty_page_overrides,
            view_trim_save_pending,
            fs_cache,
            fs_lanczos_cache,
            fs_margin_bbox_cache,
            input_generation,
            fs_pending,
            fullscreen_pdf_promotion,
            fs_pdf_display_target,
            fs_early_dims,
            fs_upload_backlog,
            top_level_grid_view,
            snapshot,
            global_search_subfolder_restore,
            favsearch_subfolder_restore,
            items_are_global_search_view,
            items_are_tag_view,
            items_are_reading_history_view,
            items_are_bookmark_view,
            items_are_rating_view,
            items_are_subfolder_expansion_view,
            items_are_smart_folder_view,
            items_are_drive_list,
            reading_history_return_from,
            bookmark_view_state,
            bookmark_open_pending,
            fs_open_intent_from_grid,
            video_presentation_transition,
            fs_zoom,
            fs_pan,
            fs_zoom_active,
            fs_zoom_aiming,
            fs_zoom_factor,
            fs_zoom_pdf_rerender_idx,
            fs_zoom_pdf_rerender_zoom,
            fs_pan_drag_start,
            fs_vertical_scroll,
            fs_seek_drag_active,
            fs_seek_gesture,
            fs_seek_overlay_visible,
            fs_vertical_cache_keep_set,
            continuous_page_transitions,
            fs_free_rotation,
            fs_rotation_drag_start,
            analysis_zoom,
            analysis_pan,
            analysis_pan_drag_start,
            analysis_overlay_cache,
            analysis_hist_cache,
            analysis_sv_cache,
            spread_mode,
            spread_shift_anchor_idx,
            reading_flow,
            reading_direction,
            slideshow_playing,
            slideshow_next_at,
            slideshow_anchor_idx,
            continuous_reading_scroll_transition,
            slideshow_scroll_range_cache,
            pdf_enumerate_pending,
            zip_enumerate_pending,
            fs_nav_after_pdf_enumerate,
            pdf_password_request,
            pdf_current_password,
            pdf_password_pending_save,
            pending_auto_fs_open,
            pending_return_to_parent,
            pdf_placeholder_count,
            viewer_navigation_caches,
            fs_nav_locked_gen,
            fs_nav_dropped_block_signature,
            fs_nav_dropped_block_count,
            fs_load_skip_signature,
            fs_holdover_tex,
            fs_boundary_hint,
            virtual_folder_writeback,
            pdf_prefetch_grace_until,
            thumb_pixels,
            thumb_edit_preview_layers,
            thumb_edit_preview_keys,
            thumb_adjust_tex,
            passthrough_rendition_cache,
            adjustment_page_params,
            local_adjust_page_layers,
            local_adjust_pages,
            local_adjust_selected_layers,
            local_adjust_generation,
            local_adjust_cache,
            local_adjust_pending,
            export_crop_page_settings,
            export_crop_pages,
            mask_pages,
            comic_pages,
            conceal_pages,
            erase_mask_generation,
            conceal_mask_generation,
            edit_result_cache,
            final_ai_cache,
            final_ai_pending,
            final_ai_failed,
            final_composite_cache,
            final_effect_pending,
            adjustment_cache,
            erase_result_cache,
            erase_preview_cache,
            erase_base_cache,
            conceal_base_cache,
            conceal_cache,
            comic_cache,
            comic_bake_pending,
            erase_inpaint_pending,
            ai_classify_cache,
            normalize_ui_states,
            normalize_auto_scan_suppressed,
            music_bookmarks,
            music_bookmarks_loaded_for,
            last_loop_pos,
        } = parked.as_mut();

        // EOF 連続再生 / 前後ファイル移動が参照する一覧 identity は parked にも複製する。
        // A fork copies the requirement values, never the mutable worker projection.
        // Otherwise the new overlay (or its generation invalidation) cancels main's work.
        *still_seek_thumbnail_pages_shared = Arc::new(std::sync::RwLock::new(
            self.still_seek_thumbnail_pages.clone(),
        ));

        duplicate_for_parked!(
            address,
            current_folder,
            favorite_view_context,
            archive_source_override,
            zip_nav,
            stack_mode_requested,
            stack_view,
            stack_showing_flat,
            stack_active_rule,
            stack_script_error,
            stack_toggle_select_path,
            items,
            items_generation,
            visible_indices,
            facet_name_cache,
            facet_name_cache_generation,
            facet_name_cache_failed_generation,
            thumbnails,
            image_metas,
            auto_aspect,
            selected,
            grid_click_selection_anchor,
            scroll_offset_y,
            scroll_to_selected,
            pending_grid_scroll,
            keep_range,
            keep_set,
            still_seek_thumbnail_pages,
            thumbnail_eviction_generation,
            details_order,
            details_order_revision,
            details_cell_content_revisions,
            search_filter,
            search_filter_origin_folder,
            checked,
            rotation_cache,
            page_dims_cache,
            spread_display_units_cache,
            rating_cache,
            rating_filter_suppressed_at,
            rating_session_write_seen_generation,
            current_folder_rating_cache,
            tags_cache,
            current_folder_last_mtime,
            current_folder_signature,
            top_level_grid_view,
            snapshot,
            global_search_subfolder_restore,
            favsearch_subfolder_restore,
            items_are_global_search_view,
            items_are_tag_view,
            items_are_reading_history_view,
            items_are_bookmark_view,
            items_are_rating_view,
            items_are_subfolder_expansion_view,
            items_are_smart_folder_view,
            items_are_drive_list,
            reading_history_return_from,
            bookmark_view_state,
        );

        // 再生中 player / pending と fullscreen viewer の一時 UI だけを parked 所有へ移す。
        move_to_parked!(
            vst3_deferred_media_open,
            fullscreen_idx,
            fullscreen_page_slice,
            native_video_in_window_active,
            video_audio_mode,
            video_audio_vst,
            video_audio_mode_entry_target,
            video_audio_exit_pending,
            panorama_state,
            // 意図は 360 state と同じ側へ動く。渡した viewer が 360 を続けるので、
            // その viewer がページを移ったときに復帰するのも同じ側 (backlog §1.145)。
            panorama_intent,
            fs_info_panel,
            pano_toast_shown_for_current_fs,
            analysis_mode,
            analysis_hover_color,
            analysis_pinned_color,
            analysis_grayscale,
            analysis_mosaic_grid,
            analysis_filter_mag,
            analysis_guide_drag,
            fs_cache,
            fs_lanczos_cache,
            fs_margin_bbox_cache,
            input_generation,
            fs_pending,
            fullscreen_pdf_promotion,
            fs_pdf_display_target,
            fs_early_dims,
            fs_upload_backlog,
            fs_open_intent_from_grid,
            video_presentation_transition,
            fs_zoom,
            fs_pan,
            fs_zoom_active,
            fs_zoom_aiming,
            fs_zoom_factor,
            fs_zoom_pdf_rerender_idx,
            fs_zoom_pdf_rerender_zoom,
            fs_pan_drag_start,
            fs_vertical_scroll,
            fs_seek_drag_active,
            fs_seek_gesture,
            fs_seek_overlay_visible,
            fs_vertical_cache_keep_set,
            continuous_page_transitions,
            fs_free_rotation,
            fs_rotation_drag_start,
            analysis_zoom,
            analysis_pan,
            analysis_pan_drag_start,
            analysis_overlay_cache,
            analysis_hist_cache,
            analysis_sv_cache,
            slideshow_playing,
            slideshow_next_at,
            slideshow_anchor_idx,
            continuous_reading_scroll_transition,
            slideshow_scroll_range_cache,
            viewer_navigation_caches,
            fs_nav_locked_gen,
            fs_nav_dropped_block_signature,
            fs_nav_dropped_block_count,
            fs_load_skip_signature,
            fs_holdover_tex,
            fs_boundary_hint,
            pdf_password_request,
            pdf_current_password,
            pdf_password_pending_save,
            normalize_ui_states,
            normalize_auto_scan_suppressed,
            music_bookmarks,
            music_bookmarks_loaded_for,
            last_loop_pos,
            bookmark_open_pending,
        );

        viewer_session.swap_with_mounted(
            &mut self.viewer_presentation,
            &mut self.last_viewer_sync_stamp,
            &mut self.detached_viewer_independent_active,
            &mut self.detached_viewer_open_next_still_detached_once,
            &mut self.detached_viewer_window_id,
        );

        // グリッド worker / 詳細列 / タグ prewarm / 編集・見開き・view-trim / folder-nav は
        // main が原本を保持する。parked メディア窓はこれらを駆動しないので empty のままでよい。
        keep_in_main!(
            navigation_scope,
            // 新しい parked context は複製済み items/cache の独立 owner になる。進行中の
            // receiver だけは複製できないため、元の main context に残す。
            facet_name_cache_pending,
            requested,
            metadata_import_refresh_index,
            idle_upgrade_cache_bypass_ineligible,
            details_thumb_suppression_applied,
            details_hover_thumb_idx,
            details_hover_thumb_viewport_open,
            texture_backlog,
            details_tag_prewarm_indices,
            details_lazy_meta,
            details_meta_pending,
            details_lazy_visible_revision,
            details_image_dims_state,
            video_thumb_overrides,
            metadata_cache,
            exif_cache,
            xmp_cache,
            xmp_panorama_info,
            metadata_pending,
            tag_prewarm_pending,
            tag_prewarm_queued,
            tag_legacy_seed_pending,
            pending_finalize,
            tx,
            rx,
            cancel_token,
            reload_queue,
            heavy_io_queue,
            scroll_hint,
            visible_end_shared,
            keep_start_shared,
            keep_end_shared,
            last_vis_range,
            vis_settle_at,
            vis_first_logged,
            vis_all_logged,
            folder_nav_pending,
            folder_pane_open_pending,
            pending_folder_nav_steps,
            pending_folder_nav_mode,
            folder_pin_map,
            converted_archive_cache_paths,
            converted_archive_pin_root_states,
            converted_archive_cache_paths_pending,
            current_color_cache_map,
            current_color_catalog,
            view_trim_mode,
            view_trim_apply_mode,
            view_trim_page_apply_root_idx,
            view_trim_page_spread_separate,
            view_trim_book_settings,
            view_trim_page_overrides,
            view_trim_dirty_page_overrides,
            view_trim_save_pending,
            spread_mode,
            spread_shift_anchor_idx,
            reading_flow,
            reading_direction,
            pdf_enumerate_pending,
            zip_enumerate_pending,
            fs_nav_after_pdf_enumerate,
            pending_auto_fs_open,
            pending_return_to_parent,
            pdf_placeholder_count,
            virtual_folder_writeback,
            pdf_prefetch_grace_until,
            thumb_pixels,
            thumb_edit_preview_layers,
            thumb_edit_preview_keys,
            thumb_adjust_tex,
            passthrough_rendition_cache,
            adjustment_page_params,
            local_adjust_page_layers,
            local_adjust_pages,
            local_adjust_selected_layers,
            local_adjust_generation,
            local_adjust_cache,
            local_adjust_pending,
            export_crop_page_settings,
            export_crop_pages,
            mask_pages,
            comic_pages,
            conceal_pages,
            erase_mask_generation,
            conceal_mask_generation,
            edit_result_cache,
            final_ai_cache,
            final_ai_pending,
            final_ai_failed,
            final_composite_cache,
            final_effect_pending,
            adjustment_cache,
            erase_result_cache,
            erase_preview_cache,
            erase_base_cache,
            conceal_base_cache,
            conceal_cache,
            comic_cache,
            comic_bake_pending,
            erase_inpaint_pending,
            ai_classify_cache,
        );
        parked
    }

    /// 完全な物理一覧を既に materialize 済みの main context から、auto-fullscreen の
    /// detached read model を作る。通常の grid leaf open は descriptor 経路で非同期列挙するため、
    /// この snapshot 経路は folder / ZIP / PDF の列挙完了後に限る。
    #[cfg(windows)]
    pub(in crate::app) fn split_materialized_physical_context_for_detached_scope(
        &mut self,
        physical_context: &Path,
        idx: usize,
        items_generation: u64,
    ) -> Box<ViewerContextBundle> {
        let mut detached = self.split_current_context_preserving_main_grid();

        detached.view_trim_mode = self.view_trim_mode;
        detached.view_trim_apply_mode = self.view_trim_apply_mode;
        detached.view_trim_page_apply_root_idx = self.view_trim_page_apply_root_idx;
        detached.view_trim_page_spread_separate = self.view_trim_page_spread_separate;
        detached.view_trim_book_settings = self.view_trim_book_settings.clone();
        detached.view_trim_page_overrides = self.view_trim_page_overrides.clone();
        detached.spread_mode = self.spread_mode;
        detached.spread_shift_anchor_idx = self.spread_shift_anchor_idx;
        detached.reading_flow = self.reading_flow;
        detached.reading_direction = self.reading_direction;

        detached.thumb_pixels = self.thumb_pixels.clone();
        detached.thumb_edit_preview_layers = self.thumb_edit_preview_layers.clone();
        detached.thumb_edit_preview_keys = self.thumb_edit_preview_keys.clone();
        detached.thumb_adjust_tex = self.thumb_adjust_tex.clone();
        detached.adjustment_page_params = self.adjustment_page_params.clone();
        detached.local_adjust_page_layers = self.local_adjust_page_layers.clone();
        detached.local_adjust_pages = self.local_adjust_pages.clone();
        detached.local_adjust_selected_layers = self.local_adjust_selected_layers.clone();
        detached.local_adjust_generation = self.local_adjust_generation.clone();
        detached.export_crop_page_settings = self.export_crop_page_settings.clone();
        detached.export_crop_pages = self.export_crop_pages.clone();
        detached.mask_pages = self.mask_pages.clone();
        detached.comic_pages = self.comic_pages.clone();
        detached.conceal_pages = self.conceal_pages.clone();
        detached.erase_mask_generation = self.erase_mask_generation.clone();
        detached.conceal_mask_generation = self.conceal_mask_generation.clone();
        detached.final_ai_cache = self.final_ai_cache.clone();
        detached.final_ai_failed = self.final_ai_failed.clone();
        detached.erase_base_cache = self.erase_base_cache.clone();
        detached.conceal_base_cache = self.conceal_base_cache.clone();
        detached.ai_classify_cache = self.ai_classify_cache.clone();

        detached.navigation_scope = ViewerNavigationScope::DetachedPhysical;
        detached.set_items_generation(items_generation);
        detached.address = physical_context.display().to_string();
        detached.current_folder = Some(physical_context.to_path_buf());
        detached.visible_indices = detached
            .items
            .iter()
            .enumerate()
            .filter_map(|(item_idx, item)| {
                item_belongs_to_detached_physical_scope(item, physical_context).then_some(item_idx)
            })
            .collect();
        detached.top_level_grid_view = top_level_grid_view::TopLevelGridView::default();
        // 物理フォルダ scope の detached は「その 1 フォルダの一覧」として作り直すので、
        // top-level surface と対で snapshot の退避も持ち込まない。呼び出し側は
        // `is_snapshot_active()` で snapshot 中の source を既に弾いているが、
        // **作成ポリシーとして明示**しておく (退避だけ相続して surface が Folder に
        // 戻っていると、解除の行き先が無い state になる)。
        detached.snapshot = None;
        detached.rating_filter_suppressed_at = None;
        detached.global_search_subfolder_restore = None;
        detached.favsearch_subfolder_restore = None;
        detached.details_thumb_suppression_applied = false;
        detached.details_order.clear();
        detached.viewer_navigation_caches.invalidate();
        detached.selected = Some(idx);

        detached
    }
}

#[cfg(all(test, windows))]
std::thread_local! {
    static VIEWER_CONTEXT_SWAP_INTERIOR_FAILPOINT: std::cell::Cell<bool> =
        const { std::cell::Cell::new(false) };
}

#[cfg(all(test, windows))]
fn arm_viewer_context_swap_interior_failpoint_for_test() {
    VIEWER_CONTEXT_SWAP_INTERIOR_FAILPOINT.with(|armed| armed.set(true));
}

#[cfg(all(test, windows))]
fn panic_at_viewer_context_swap_interior_for_test() {
    VIEWER_CONTEXT_SWAP_INTERIOR_FAILPOINT.with(|armed| {
        if armed.replace(false) {
            panic!("viewer-context swap interior failpoint");
        }
    });
}

#[cfg(windows)]
enum ProductionForkSpec {
    LiveMedia,
    MaterializedStill {
        physical_context: PathBuf,
        idx: usize,
        items_generation: u64,
    },
}

#[cfg(windows)]
impl App {
    fn execute_viewer_context_ops(
        &mut self,
        ops: Vec<TableOp>,
        mut fork_spec: Option<ProductionForkSpec>,
    ) {
        let mut transient = None;
        for op in ops {
            self.execute_viewer_context_op(op, &mut transient, &mut fork_spec);
        }
        assert!(transient.is_none());
    }

    fn execute_viewer_context_op(
        &mut self,
        op: TableOp,
        transient: &mut Option<Box<ViewerContextBundle>>,
        fork_spec: &mut Option<ProductionForkSpec>,
    ) {
        match op {
            TableOp::ReplaceProjectionWithFreshEmpty => {
                assert!(transient.is_none());
                let mut payload = Box::new(ViewerContextBundle::empty());
                self.swap_viewer_context_bundle(&mut payload);
                *transient = Some(payload);
            }
            TableOp::ForkProjectionIntoTransient(policy) => {
                assert!(transient.is_none());
                *transient = Some(self.fork_viewer_context_payload(policy, fork_spec));
            }
            other => self.execute_viewer_context_storage_op(other, transient),
        }
    }

    fn execute_viewer_context_storage_op(
        &mut self,
        op: TableOp,
        transient: &mut Option<Box<ViewerContextBundle>>,
    ) {
        match op {
            TableOp::DepositInto(id) => {
                self.viewer_contexts
                    .table
                    .deposit(id, transient.take().unwrap());
            }
            TableOp::WithdrawFrom(id) => {
                assert!(transient.is_none());
                *transient = Some(self.viewer_contexts.table.withdraw(id));
            }
            TableOp::RestoreProjectionAndDropDisplacedEmpty => {
                let mut payload = transient.take().unwrap();
                self.swap_viewer_context_bundle(&mut payload);
                drop(payload);
            }
            TableOp::DropTransientAsRetired(_) => drop(transient.take().unwrap()),
            _ => unreachable!(),
        }
    }

    fn fork_viewer_context_payload(
        &mut self,
        policy: ForkPolicy,
        spec: &mut Option<ProductionForkSpec>,
    ) -> Box<ViewerContextBundle> {
        match (policy, spec.take()) {
            (ForkPolicy::LiveMediaPark { .. }, Some(ProductionForkSpec::LiveMedia)) => {
                self.split_current_context_preserving_main_grid()
            }
            (_, Some(spec)) => self.fork_materialized_payload(spec),
            _ => panic!("viewer-context fork plan was missing"),
        }
    }

    fn fork_materialized_payload(&mut self, spec: ProductionForkSpec) -> Box<ViewerContextBundle> {
        if let ProductionForkSpec::MaterializedStill {
            physical_context,
            idx,
            items_generation,
        } = spec
        {
            self.split_materialized_physical_context_for_detached_scope(
                &physical_context,
                idx,
                items_generation,
            )
        } else {
            panic!("viewer-context fork plan disagreed")
        }
    }

    fn items_generation_for_viewer_context(id: ViewerContextId) -> u64 {
        DETACHED_VIEWER_CONTEXT_GENERATION_BASE
            | id.serial()
                .wrapping_mul(DETACHED_VIEWER_CONTEXT_GENERATION_STRIDE)
    }
}

#[cfg(windows)]
impl App {
    pub(in crate::app) fn viewer_context_main(&self) -> ViewerContextId {
        self.viewer_contexts.table.main()
    }

    pub(in crate::app) fn mounted_viewer_context_id(&self) -> Option<ViewerContextId> {
        self.viewer_contexts.table.mounted_id()
    }

    /// Identity of the payload currently projected onto `App`, including a context being built.
    pub(in crate::app) fn projected_viewer_context_id(&self) -> ViewerContextId {
        self.viewer_contexts.table.projected_id()
    }

    pub(in crate::app) fn viewer_context_residence(&self, id: ViewerContextId) -> ContextResidence {
        self.viewer_contexts.table.residence(id)
    }

    pub(crate) fn locate_window_context(
        &self,
        window_id: u64,
    ) -> Option<(ViewerContextId, ContextResidence)> {
        self.viewer_contexts.table.locate_window_context(window_id)
    }

    pub(in crate::app) fn viewer_context_window_binding_probe(
        &self,
        window_id: u64,
    ) -> Option<(ViewerContextId, ContextResidence)> {
        self.viewer_contexts.table.window_binding_probe(window_id)
    }

    pub(in crate::app) fn viewer_context_window(&self, id: ViewerContextId) -> Option<u64> {
        self.viewer_contexts.table.window_for_context(id)
    }

    pub(in crate::app) fn viewer_context_ids(&self) -> Vec<ViewerContextId> {
        self.viewer_contexts.table.ids()
    }

    /// Every context except the one currently projected onto `App`. See `ContextTable::other_ids`.
    pub(in crate::app) fn other_viewer_context_ids(&self) -> Vec<ViewerContextId> {
        self.viewer_contexts.table.other_ids()
    }

    pub(in crate::app) fn with_viewer_context_ref<R>(
        &self,
        id: ViewerContextId,
        f: impl FnOnce(ContextRef<'_>) -> R,
    ) -> Option<R> {
        match self.viewer_contexts.table.residence(id) {
            ContextResidence::Mounted => Some(f(ContextRef::mounted(self))),
            ContextResidence::AtRest => self
                .viewer_contexts
                .table
                .at_rest(id)
                .map(|payload| f(ContextRef::at_rest(payload))),
            _ => None,
        }
    }

    #[track_caller]
    pub(in crate::app) fn bind_window(
        &mut self,
        id: ViewerContextId,
        window_id: u64,
    ) -> Result<(), BindError> {
        let result = self.viewer_contexts.table.bind_window(id, window_id);
        let caller = std::panic::Location::caller();
        crate::logger::log(format!(
            "[active-detached-session] t_us={} kind=binding action=bind window_id={} context={id:?} result={:?} caller={}:{} session={:?} transition={}",
            crate::logger::elapsed_micros(),
            window_id,
            result.as_ref().err(),
            caller.file(),
            caller.line(),
            self.active_detached_session
                .map(|session| session.window_id),
            crate::presentation_observer::active_transition_id().unwrap_or(0),
        ));
        result
    }

    #[track_caller]
    pub(in crate::app) fn unbind_window(&mut self, window_id: u64) -> Option<ViewerContextId> {
        assert_ne!(
            self.active_detached_window_id(),
            Some(window_id),
            "cannot release window {window_id} while the active detached session still names it"
        );
        let result = self.viewer_contexts.table.unbind_window(window_id);
        let caller = std::panic::Location::caller();
        crate::logger::log(format!(
            "[active-detached-session] t_us={} kind=binding action=unbind window_id={} context={:?} caller={}:{} session={:?} last_setter={}/{} transition={}",
            crate::logger::elapsed_micros(),
            window_id,
            result,
            caller.file(),
            caller.line(),
            self.active_detached_session
                .map(|session| session.window_id),
            self.active_detached_session_probe.owner,
            self.active_detached_session_probe.reason,
            crate::presentation_observer::active_transition_id().unwrap_or(0),
        ));
        result
    }

    #[track_caller]
    pub(in crate::app) fn reserve_window_binding_for_build(&mut self, window_id: u64) {
        self.viewer_contexts
            .table
            .reserve_window_binding_for_build(window_id);
        let caller = std::panic::Location::caller();
        crate::logger::log(format!(
            "[active-detached-session] t_us={} kind=binding action=reserve window_id={} caller={}:{} session={:?} transition={}",
            crate::logger::elapsed_micros(),
            window_id,
            caller.file(),
            caller.line(),
            self.active_detached_session
                .map(|session| session.window_id),
            crate::presentation_observer::active_transition_id().unwrap_or(0),
        ));
    }

    pub(in crate::app) fn with_viewer_context<R>(
        &mut self,
        id: ViewerContextId,
        f: impl FnOnce(&mut Self) -> R,
    ) -> Result<R, MountError> {
        let Some(original) = self.viewer_contexts.table.mounted_id() else {
            return Err(MountError {
                id,
                residence: self.viewer_contexts.table.residence(id),
            });
        };
        let ops = self.viewer_contexts.table.plan_mount(id)?;
        self.execute_viewer_context_ops(ops, None);
        self.viewer_contexts.table.finish_mount();
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| f(self)));
        let ops = self
            .viewer_contexts
            .table
            .plan_mount(original)
            .expect("mounted viewer-context owner disappeared");
        self.execute_viewer_context_ops(ops, None);
        self.viewer_contexts.table.finish_mount();
        match result {
            Ok(value) => Ok(value),
            Err(payload) => std::panic::resume_unwind(payload),
        }
    }

    pub(crate) fn with_window_viewer_context<R>(
        &mut self,
        window_id: u64,
        f: impl FnOnce(&mut Self) -> R,
    ) -> Result<R, MountError> {
        let (id, _) = self
            .locate_window_context(window_id)
            .unwrap_or_else(|| panic!("window {window_id} has no viewer-context binding"));
        self.with_viewer_context(id, f)
    }

    pub(in crate::app) fn build_viewer_context(
        &mut self,
        reason: &'static str,
        f: impl FnOnce(&mut Self, ViewerContextId) -> BuildOutcome,
    ) -> Option<ViewerContextId> {
        let (reserved, ops) = self.viewer_contexts.table.plan_begin_build();
        self.execute_viewer_context_ops(ops, None);
        self.viewer_contexts.table.finish_begin_build();
        self.set_items_generation(Self::items_generation_for_viewer_context(reserved));
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| f(self, reserved)));
        match result {
            Ok(outcome) => self.finish_viewer_context_build(reserved, reason, outcome),
            Err(payload) => {
                self.abort_viewer_context_build();
                std::panic::resume_unwind(payload)
            }
        }
    }

    fn abort_viewer_context_build(&mut self) {
        let ops = self.viewer_contexts.table.plan_abort_build();
        self.execute_viewer_context_ops(ops, None);
        self.viewer_contexts.table.finish_abort_build();
        let session = self.active_detached_session;
        let binding = session.and_then(|value| {
            self.viewer_context_window_binding_probe(value.window_id)
                .map(|(id, residence)| format!("{id:?}/{residence:?}"))
        });
        crate::logger::log(format!(
            "[active-detached-session] t_us={} kind=binding action=abort_build session={:?} binding={} last_setter={}/{} transition={}",
            crate::logger::elapsed_micros(),
            session.map(|value| value.window_id),
            binding.as_deref().unwrap_or("none"),
            self.active_detached_session_probe.owner,
            self.active_detached_session_probe.reason,
            crate::presentation_observer::active_transition_id().unwrap_or(0),
        ));
    }

    fn finish_viewer_context_build(
        &mut self,
        reserved: ViewerContextId,
        reason: &'static str,
        outcome: BuildOutcome,
    ) -> Option<ViewerContextId> {
        match outcome {
            BuildOutcome::Commit => {
                let ops = self.viewer_contexts.table.plan_commit_build();
                self.execute_viewer_context_ops(ops, None);
                let finished = self.viewer_contexts.table.finish_commit_build();
                if let Some(window_id) = self.viewer_contexts.table.window_for_context(finished) {
                    crate::logger::log(format!(
                        "[active-detached-session] t_us={} kind=binding action=publish_build window_id={} context={finished:?} reason={} session={:?} transition={}",
                        crate::logger::elapsed_micros(),
                        window_id,
                        reason,
                        self.active_detached_session
                            .map(|session| session.window_id),
                        crate::presentation_observer::active_transition_id().unwrap_or(0),
                    ));
                }
                Some(finished)
            }
            BuildOutcome::Abort(abort_reason) => {
                crate::logger::log(format!(
                    "viewer_context_build_aborted id={reserved:?} reason={reason} abort={abort_reason}"
                ));
                self.abort_viewer_context_build();
                None
            }
        }
    }

    fn fork_mounted_context(
        &mut self,
        policy: ForkPolicy,
        spec: ProductionForkSpec,
    ) -> ViewerContextId {
        let (id, ops) = self.viewer_contexts.table.plan_fork(policy);
        self.execute_viewer_context_ops(ops, Some(spec));
        let finished = self.viewer_contexts.table.finish_fork();
        assert_eq!(finished, id);
        id
    }

    pub(in crate::app) fn fork_mounted_live_media_context(
        &mut self,
        window_id: u64,
    ) -> ViewerContextId {
        let source = self.projected_viewer_context_id();
        let parked = self.fork_mounted_context(
            ForkPolicy::LiveMediaPark { window_id },
            ProductionForkSpec::LiveMedia,
        );
        crate::logger::log(format!(
            "[active-detached-session] t_us={} kind=binding action=transfer_live window_id={} from={source:?} to={parked:?} session={:?} transition={}",
            crate::logger::elapsed_micros(),
            window_id,
            self.active_detached_session
                .map(|session| session.window_id),
            crate::presentation_observer::active_transition_id().unwrap_or(0),
        ));
        self.transfer_native_video_open_pending_context(source, parked);
        parked
    }

    pub(in crate::app) fn fork_materialized_still_context(
        &mut self,
        physical_context: &Path,
        idx: usize,
    ) -> ViewerContextId {
        let id = ViewerContextId(self.viewer_contexts.table.next_serial);
        let items_generation = Self::items_generation_for_viewer_context(id);
        self.fork_mounted_context(
            ForkPolicy::MaterializedStillOpen,
            ProductionForkSpec::MaterializedStill {
                physical_context: physical_context.to_path_buf(),
                idx,
                items_generation,
            },
        )
    }

    pub(in crate::app) fn retire_context<D>(
        &mut self,
        id: ViewerContextId,
        _reason: &'static str,
        digest: impl FnOnce(ContextMut<'_>) -> D,
    ) -> Result<D, RetireError> {
        let reason = _reason;
        let retiring_window = self.viewer_contexts.table.window_for_context(id);
        self.viewer_contexts.table.begin_retire(id)?;
        if retiring_window == self.active_detached_window_id() {
            self.begin_active_detached_session_close(reason);
            self.finish_active_detached_session_close(reason);
        }
        let value = {
            let payload = self.viewer_contexts.table.retiring_slot_mut(id).unwrap();
            digest(ContextMut { bundle: payload })
        };
        if let Some(window_id) = retiring_window {
            assert_eq!(self.unbind_window(window_id), Some(id));
        }
        let ops = self.viewer_contexts.table.plan_finish_retire(id);
        self.execute_viewer_context_ops(ops, None);
        self.viewer_contexts.table.finish_retire();
        Ok(value)
    }

    pub(in crate::app) fn close_and_retire_context<D>(
        &mut self,
        id: ViewerContextId,
        reason: &'static str,
        finish: impl FnOnce(&mut Self),
        digest: impl FnOnce(ContextMut<'_>) -> D,
    ) -> Result<D, RetireContextError> {
        self.with_viewer_context(id, finish)
            .map_err(RetireContextError::Mount)?;
        self.retire_context(id, reason, digest)
            .map_err(RetireContextError::Retire)
    }

    pub(in crate::app) fn stash_mounted_and_start_fresh(
        &mut self,
        _reason: &'static str,
    ) -> ViewerContextId {
        let ops = self.viewer_contexts.table.plan_promote();
        self.execute_viewer_context_ops(ops, None);
        self.viewer_contexts.table.finish_promote()
    }
}

#[cfg(all(test, windows))]
impl App {
    pub(in crate::app) fn begin_mounted_detached_session_for_test(
        &mut self,
        window_id: u64,
        source: DetachedSource,
    ) {
        if self.locate_window_context(window_id).is_none() {
            let mounted = self
                .mounted_viewer_context_id()
                .expect("test mounted detached session requires a mounted context");
            self.bind_window(mounted, window_id)
                .unwrap_or_else(|error| panic!("test mounted session binding failed: {error:?}"));
        }
        self.detached_viewer_window_id = Some(window_id);
        self.begin_active_detached_session(window_id, source);
    }

    pub(in crate::app) fn bind_mounted_context_for_test(&mut self, window_id: u64) {
        let mounted = self
            .mounted_viewer_context_id()
            .expect("test window binding requires a mounted context");
        self.bind_window(mounted, window_id)
            .unwrap_or_else(|error| panic!("test mounted context binding failed: {error:?}"));
        self.last_active_detached_window_id = Some(window_id);
    }

    pub(crate) fn build_window_context_for_test(
        &mut self,
        window_id: u64,
        configure: impl FnOnce(&mut Self),
    ) -> ViewerContextId {
        self.build_viewer_context("test_build_window_context", |app, _reserved| {
            configure(app);
            app.reserve_window_binding_for_build(window_id);
            BuildOutcome::Commit
        })
        .expect("test window viewer context build must commit")
    }
    pub(in crate::app) fn push_window_context_for_test(
        &mut self,
        ctx: &egui::Context,
        window_id: u64,
        configure: impl FnOnce(&mut Self),
    ) -> ViewerContextId {
        let texture = ctx.load_texture(
            format!("window_context_for_test_{window_id}"),
            egui::ColorImage::new([1, 1], vec![egui::Color32::BLACK]),
            egui::TextureOptions::LINEAR,
        );
        let snapshot = DetachedImageWindowSnapshot {
            id: window_id,
            texture: crate::gpu_lanczos::FullscreenPaintResource::direct(texture),
            title: format!("paused-{window_id}"),
            location_display: format!("paused-{window_id}"),
            image_dims: None,
            rotation: crate::rotation_db::Rotation::None,
            zoom_pan: None,
            free_rotation: 0.0,
            image_rect_norm: egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
            image_content_bbox: None,
            frozen_continuous_pages: Vec::new(),
            reopen_descriptor: None,
            reopen_sync_stamp: None,
            activation_ready_frame: 0,
            activation_armed: true,
            focused_last_frame: false,
            initial_placement_applied: true,
        };
        let id = self.build_window_context_for_test(window_id, configure);
        self.detached_image_windows.push(snapshot);
        id
    }

    pub(in crate::app) fn build_active_context_for_test(
        &mut self,
        window_id: Option<u64>,
        source: DetachedSource,
        configure: impl FnOnce(&mut Self),
    ) -> ViewerContextId {
        let window_id = window_id.unwrap_or_else(|| self.allocate_detached_viewer_window_id());
        let id = self
            .build_viewer_context("test_build_active_context", |app, _reserved| {
                configure(app);
                app.detached_viewer_window_id = Some(window_id);
                app.reserve_window_binding_for_build(window_id);
                BuildOutcome::Commit
            })
            .expect("test active viewer context build must commit");
        self.begin_active_detached_session(window_id, source);
        id
    }

    pub(in crate::app) fn remove_active_context_for_test(&mut self) {
        let Some(id) = self.active_viewer_context_id() else {
            return;
        };
        self.retire_context(id, "test_remove_active_context", |_| ())
            .unwrap_or_else(|error| panic!("test active context retire failed: {error:?}"));
    }

    pub(in crate::app) fn stash_mounted_as_active_for_test(
        &mut self,
        window_id: u64,
    ) -> ViewerContextId {
        let id = self.stash_mounted_and_start_fresh("test_stash_mounted_as_active");
        self.bind_window(id, window_id)
            .unwrap_or_else(|error| panic!("test stashed context binding failed: {error:?}"));
        let previous = self.active_detached_session;
        self.active_detached_session = Some(ActiveDetachedSession {
            window_id,
            source: DetachedSource::Image,
        });
        self.record_active_detached_session_write(
            "set",
            "test_stash_mounted_as_active",
            "App::stash_mounted_as_active_for_test",
            previous,
            self.active_detached_session,
            std::panic::Location::caller(),
        );
        id
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;
    use std::panic::{AssertUnwindSafe, catch_unwind};
    use std::rc::Rc;

    #[cfg(windows)]
    fn still_seek_committed_center(center: usize) -> crate::ui_fullscreen::StillSeekGesture {
        crate::ui_fullscreen::StillSeekGesture::StripCommitted {
            layout_center_pos: center,
            page_pos_at_commit: 3,
        }
    }

    #[cfg(windows)]
    #[test]
    fn viewer_context_still_seek_centers_survive_mount_and_close() {
        use crate::ui_fullscreen::StillSeekGesture;
        for b_page in [3, 7] {
            let mut app = crate::app::setup_app_for_test();
            let a = app.build_window_context_for_test(701, |app| {
                app.fullscreen_idx = Some(3);
                app.fs_seek_gesture = still_seek_committed_center(120);
            });
            let b = app.build_window_context_for_test(702, |app| {
                app.fullscreen_idx = Some(b_page);
                assert_eq!(
                    app.fs_seek_gesture,
                    StillSeekGesture::Idle,
                    "a fresh context must not inherit A's center"
                );
                app.fs_seek_gesture = StillSeekGesture::StripCommitted {
                    layout_center_pos: 440,
                    page_pos_at_commit: b_page,
                };
            });
            for _ in 0..2 {
                app.with_viewer_context(a, |app| {
                    assert_eq!(app.fs_seek_gesture, still_seek_committed_center(120));
                })
                .unwrap();
                app.with_viewer_context(b, |app| {
                    assert_eq!(
                        app.fs_seek_gesture,
                        StillSeekGesture::StripCommitted {
                            layout_center_pos: 440,
                            page_pos_at_commit: b_page
                        }
                    );
                })
                .unwrap();
            }
            app.close_and_retire_context(
                b,
                "still_seek_test_close",
                |app| {
                    app.close_fullscreen();
                },
                |_| (),
            )
            .unwrap();
            app.with_viewer_context(a, |app| {
                assert_eq!(
                    app.fs_seek_gesture,
                    still_seek_committed_center(120),
                    "closing B must not reset A"
                );
            })
            .unwrap();
        }
    }

    #[cfg(windows)]
    #[test]
    fn viewer_context_still_seek_pause_ends_only_its_active_gesture() {
        let mut app = crate::app::setup_app_for_test();
        let a = app.build_window_context_for_test(703, |app| {
            app.fs_seek_gesture = still_seek_committed_center(120);
        });
        let b = app.build_window_context_for_test(704, |app| {
            app.fs_seek_gesture = crate::ui_fullscreen::StillSeekGesture::Strip {
                origin_center_pos: 3,
                origin_pointer_x: 100.0,
                layout_center_pos: 440,
                page_pos_at_origin: 3,
            };
            app.fs_seek_drag_active = true;
            app.pause_mounted_background_work_keep_current_frame();
            assert_eq!(
                app.fs_seek_gesture,
                crate::ui_fullscreen::StillSeekGesture::Idle
            );
            assert!(!app.fs_seek_drag_active);
        });
        app.with_viewer_context(a, |app| {
            app.pause_mounted_background_work_keep_current_frame();
            assert_eq!(app.fs_seek_gesture, still_seek_committed_center(120));
        })
        .unwrap();
        app.retire_context(b, "still_seek_test_retire", |_| ())
            .unwrap();
    }

    #[cfg(windows)]
    #[test]
    fn viewer_context_still_seek_split_moves_gesture_and_isolates_request_projection() {
        let ctx = egui::Context::default();
        let mut app = crate::app::setup_app_for_test();
        app.items = (0..10)
            .map(|idx| GridItem::Image(PathBuf::from(format!("c:/seek/{idx}.png"))))
            .collect();
        app.ensure_still_seek_thumbnail_requests(&ctx, &[2, 3]);
        app.fs_seek_gesture = still_seek_committed_center(7);
        let mut fork = app.split_current_context_preserving_main_grid();
        assert_eq!(
            app.fs_seek_gesture,
            crate::ui_fullscreen::StillSeekGesture::Idle,
            "the viewer gesture must move out of the main grid"
        );
        let main_projection = app.still_seek_thumbnail_pages_shared.clone();
        app.swap_viewer_context_bundle(&mut fork);
        assert_eq!(app.fs_seek_gesture, still_seek_committed_center(7));
        assert!(!Arc::ptr_eq(
            &main_projection,
            &app.still_seek_thumbnail_pages_shared
        ));
        app.ensure_still_seek_thumbnail_requests(&ctx, &[8, 9]);
        assert_eq!(
            *main_projection.read().unwrap(),
            [2, 3].into_iter().collect()
        );
        app.bump_items_generation();
        assert!(app.still_seek_thumbnail_pages.is_empty());
        assert!(
            app.still_seek_thumbnail_pages_shared
                .read()
                .unwrap()
                .is_empty()
        );
        assert_eq!(
            *main_projection.read().unwrap(),
            [2, 3].into_iter().collect()
        );
        app.swap_viewer_context_bundle(&mut fork);
        drop(fork);
        assert_eq!(
            *main_projection.read().unwrap(),
            [2, 3].into_iter().collect()
        );
    }

    #[cfg(windows)]
    #[test]
    fn viewer_context_still_seek_fork_request_projection_is_independent() {
        let ctx = egui::Context::default();
        let mut app = crate::app::setup_app_for_test();
        app.items = (0..10)
            .map(|idx| GridItem::Image(PathBuf::from(format!("c:/seek/{idx}.png"))))
            .collect();
        app.ensure_still_seek_thumbnail_requests(&ctx, &[2, 3]);
        let mut fork = app.split_current_context_preserving_main_grid();
        let main_projection = app.still_seek_thumbnail_pages_shared.clone();
        app.swap_viewer_context_bundle(&mut fork);
        app.ensure_still_seek_thumbnail_requests(&ctx, &[8, 9]);
        assert_eq!(
            *main_projection.read().unwrap(),
            [2, 3].into_iter().collect()
        );
    }

    #[cfg(windows)]
    #[test]
    fn favorite_view_state_is_isolated_by_bundle_swap() {
        let mut app = crate::app::setup_app_for_test();
        app.settings.remember_favorite_view_state = true;
        app.settings.thumb_px = 100;
        let a = crate::settings::FavoriteEntry::new(
            "a".to_owned(),
            std::path::PathBuf::from(r"C:\favorite-a"),
        );
        let b = crate::settings::FavoriteEntry::new(
            "b".to_owned(),
            std::path::PathBuf::from(r"C:\favorite-b"),
        );
        let mut a_state = crate::settings::FavoriteViewState::from_settings(&app.settings);
        a_state.thumb_px = 160;
        let mut b_state = a_state.clone();
        b_state.thumb_px = 260;
        app.favorite_view_states.insert(a.id, a_state);
        app.favorite_view_states.insert(b.id, b_state);
        app.settings.favorites.extend([a.clone(), b.clone()]);
        app.current_folder = Some(a.path.clone());
        app.transition_favorite_view_for_path(Some(&a.path));
        app.settings.thumb_px = 170;

        let mut parked = ViewerContextBundle::empty();
        parked.current_folder = Some(b.path.clone());
        app.swap_viewer_context_bundle(&mut parked);
        assert_eq!(app.settings.thumb_px, 260);
        assert_eq!(app.favorite_view_states[&a.id].thumb_px, 170);

        app.settings.thumb_px = 270;
        app.swap_viewer_context_bundle(&mut parked);
        assert_eq!(app.settings.thumb_px, 170);
        assert_eq!(app.favorite_view_states[&b.id].thumb_px, 270);

        app.transition_favorite_view_for_path(Some(std::path::Path::new(r"C:\outside")));
        assert_eq!(
            app.settings.thumb_px, 100,
            "共通状態はどちらの窓にも上書きされない"
        );
    }

    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    enum Event {
        Unbind(ViewerContextId),
        Drop(ViewerContextId),
    }

    struct TestPayload {
        tag: u32,
        empty: bool,
        trace: Option<Rc<RefCell<Vec<Event>>>>,
        drop_id: Option<ViewerContextId>,
    }

    impl TestPayload {
        fn occupied(tag: u32) -> Self {
            Self {
                tag,
                empty: false,
                trace: None,
                drop_id: None,
            }
        }

        fn traced(tag: u32, trace: Rc<RefCell<Vec<Event>>>) -> Self {
            Self {
                tag,
                empty: false,
                trace: Some(trace),
                drop_id: None,
            }
        }

        fn fresh_empty() -> Self {
            Self {
                tag: 0,
                empty: true,
                trace: None,
                drop_id: None,
            }
        }

        fn materialize(&mut self, tag: u32) {
            self.tag = tag;
            self.empty = false;
        }

        fn forked(&self, policy: ForkPolicy) -> Self {
            let offset = match policy {
                ForkPolicy::LiveMediaPark { .. } => 1_000,
                ForkPolicy::MaterializedStillOpen => 2_000,
            };
            Self {
                tag: self.tag + offset,
                empty: false,
                trace: self.trace.clone(),
                drop_id: None,
            }
        }
    }

    impl Drop for TestPayload {
        fn drop(&mut self) {
            if let (Some(trace), Some(id)) = (&self.trace, self.drop_id) {
                trace.borrow_mut().push(Event::Drop(id));
            }
        }
    }

    fn harness(tag: u32) -> (ContextTable<TestPayload>, TestPayload) {
        (ContextTable::new(), TestPayload::occupied(tag))
    }

    fn execute_ops(
        table: &mut ContextTable<TestPayload>,
        projection: &mut TestPayload,
        ops: &[TableOp],
    ) {
        execute_ops_with_failpoint(table, projection, ops, None);
    }

    fn execute_ops_with_failpoint(
        table: &mut ContextTable<TestPayload>,
        projection: &mut TestPayload,
        ops: &[TableOp],
        fail_after: Option<usize>,
    ) {
        let mut transient = None;
        for (index, op) in ops.iter().copied().enumerate() {
            match op {
                TableOp::ReplaceProjectionWithFreshEmpty => {
                    assert!(transient.is_none());
                    transient = Some(std::mem::replace(projection, TestPayload::fresh_empty()));
                }
                TableOp::ForkProjectionIntoTransient(policy) => {
                    assert!(transient.is_none());
                    transient = Some(projection.forked(policy));
                }
                TableOp::DepositInto(id) => {
                    let payload = transient.take().expect("deposit requires a transient");
                    table.deposit(id, payload);
                }
                TableOp::WithdrawFrom(id) => {
                    assert!(transient.is_none());
                    transient = Some(table.withdraw(id));
                }
                TableOp::RestoreProjectionAndDropDisplacedEmpty => {
                    let payload = transient.take().expect("restore requires a transient");
                    let displaced = std::mem::replace(projection, payload);
                    assert!(displaced.empty);
                    drop(displaced);
                }
                TableOp::DropTransientAsRetired(id) => {
                    let mut payload = transient.take().expect("retire requires a transient");
                    payload.drop_id = Some(id);
                    let trace = payload.trace.clone();
                    table.unbind_context_core(id);
                    assert!(!table.window_of.contains_key(&id));
                    assert!(!table.context_of.values().any(|owner| *owner == id));
                    if let Some(trace) = trace {
                        trace.borrow_mut().push(Event::Unbind(id));
                    }
                    drop(payload);
                }
            }

            if fail_after == Some(index + 1) {
                panic!("operation failpoint {}", index + 1);
            }
        }
        assert!(transient.is_none());
    }

    fn begin_build(
        table: &mut ContextTable<TestPayload>,
        projection: &mut TestPayload,
    ) -> ViewerContextId {
        let (reserved, ops) = table.plan_begin_build();
        execute_ops(table, projection, &ops);
        table.finish_begin_build();
        reserved
    }

    fn fork_at_rest(
        table: &mut ContextTable<TestPayload>,
        projection: &mut TestPayload,
        policy: ForkPolicy,
    ) -> ViewerContextId {
        let (new_id, ops) = table.plan_fork(policy);
        execute_ops(table, projection, &ops);
        assert_eq!(table.finish_fork(), new_id);
        new_id
    }

    fn at_rest_payload(table: &ContextTable<TestPayload>, id: ViewerContextId) -> &TestPayload {
        match table.slots.get(&id) {
            Some(Slot::AtRest(payload)) => payload,
            Some(Slot::Retiring(_)) | None => panic!("context is not at rest"),
        }
    }

    #[test]
    #[cfg(windows)]
    fn mounted_independent_activation_sets_the_complete_session_identity_tuple() {
        let mut app = crate::app::tests::phase_c_support::setup_app();
        app.viewer_presentation = ViewerPresentation::Fullscreen;
        app.detached_viewer_independent_active = false;
        app.detached_viewer_open_next_still_detached_once = true;
        app.detached_viewer_window_id = None;
        app.last_viewer_sync_stamp = Some(ViewerSyncStamp {
            idx: 1,
            item_key: "stale".to_owned(),
            items_generation: 10,
        });

        app.activate_mounted_as_independent_detached(37);

        assert_eq!(app.viewer_presentation, ViewerPresentation::DetachedWindow);
        assert!(app.detached_viewer_independent_active);
        assert!(!app.detached_viewer_open_next_still_detached_once);
        assert_eq!(app.detached_viewer_window_id, Some(37));
        assert_eq!(app.last_viewer_sync_stamp, None);
        assert!(!app.fs_open_intent_from_grid);
        assert!(!app.pending_auto_fs_open);
        assert!(!app.pending_return_to_parent);
        assert_eq!(app.pdf_prefetch_grace_until, None);
    }

    #[test]
    #[cfg(windows)]
    fn production_i1_i1b_mount_round_trip_is_panic_safe_and_never_loses_projection() {
        let mut app = crate::app::tests::phase_c_support::setup_app();
        app.address = "main".to_string();
        let main = app.viewer_context_main();
        let built = app
            .build_viewer_context("production_i1_build", |app, reserved| {
                assert_eq!(
                    app.viewer_context_residence(reserved),
                    ContextResidence::Building
                );
                assert_eq!(app.viewer_context_ids(), vec![main, reserved]);
                app.address = "detached".to_string();
                app.reserve_window_binding_for_build(701);
                BuildOutcome::Commit
            })
            .unwrap();

        assert_eq!(app.mounted_viewer_context_id(), Some(main));
        assert_eq!(app.address, "main");
        let panic = catch_unwind(AssertUnwindSafe(|| {
            let _ = app.with_viewer_context(built, |app| {
                assert_eq!(app.address, "detached");
                panic!("production mount panic");
            });
        }));
        assert!(panic.is_err());
        assert_eq!(app.mounted_viewer_context_id(), Some(main));
        assert_eq!(
            app.viewer_context_residence(built),
            ContextResidence::AtRest
        );
        assert_eq!(app.address, "main");
    }

    #[test]
    #[cfg(windows)]
    fn production_i2_i3_window_bindings_are_a_bijection() {
        let mut app = crate::app::tests::phase_c_support::setup_app();
        let first = app
            .build_viewer_context("production_i2_first", |app, _| {
                app.reserve_window_binding_for_build(702);
                BuildOutcome::Commit
            })
            .unwrap();
        let second = app
            .build_viewer_context("production_i2_second", |_, _| BuildOutcome::Commit)
            .unwrap();

        assert_eq!(
            app.bind_window(second, 702),
            Err(BindError::WindowOwnedBy(first))
        );
        assert_eq!(
            app.bind_window(first, 703),
            Err(BindError::ContextOwnedBy(702))
        );
        assert_eq!(app.unbind_window(702), Some(first));
        assert_eq!(app.bind_window(first, 703), Ok(()));
        assert_eq!(
            app.locate_window_context(703),
            Some((first, ContextResidence::AtRest))
        );
    }

    #[test]
    #[cfg(windows)]
    fn production_i4_main_is_rejected_while_at_rest() {
        let mut app = crate::app::tests::phase_c_support::setup_app();
        let main = app.viewer_context_main();
        let other = app
            .build_viewer_context("production_i4_other", |_, _| BuildOutcome::Commit)
            .unwrap();

        app.with_viewer_context(other, |app| {
            assert_eq!(app.viewer_context_residence(main), ContextResidence::AtRest);
            assert_eq!(
                app.retire_context(main, "production_i4_reject", |_| ()),
                Err(RetireError::IsMain)
            );
        })
        .unwrap();

        assert_eq!(app.viewer_context_main(), main);
        assert_eq!(app.mounted_viewer_context_id(), Some(main));
    }

    #[test]
    #[cfg(windows)]
    fn production_i5_i6_building_and_retiring_reject_invalid_operations() {
        let mut app = crate::app::tests::phase_c_support::setup_app();
        let aborted = app.build_viewer_context("production_i5_building", |app, reserved| {
            assert_eq!(
                app.bind_window(reserved, 704),
                Err(BindError::NotBindable(ContextResidence::Building))
            );
            assert_eq!(
                app.with_viewer_context(reserved, |_| ()),
                Err(MountError {
                    id: reserved,
                    residence: ContextResidence::Building,
                })
            );
            assert_eq!(
                app.retire_context(reserved, "production_i5_retire", |_| ()),
                Err(RetireError::Building)
            );
            BuildOutcome::Abort("expected")
        });
        assert_eq!(aborted, None);

        let retiring = app
            .build_viewer_context("production_i6_retiring", |_, _| BuildOutcome::Commit)
            .unwrap();
        app.viewer_contexts.table.begin_retire(retiring).unwrap();
        assert_eq!(
            app.bind_window(retiring, 705),
            Err(BindError::NotBindable(ContextResidence::Retiring))
        );
        assert_eq!(
            app.with_viewer_context(retiring, |_| ()),
            Err(MountError {
                id: retiring,
                residence: ContextResidence::Retiring,
            })
        );
        assert_eq!(
            app.retire_context(retiring, "production_i6_repeat", |_| ()),
            Err(RetireError::NotAtRest(ContextResidence::Retiring))
        );
        let ops = app.viewer_contexts.table.plan_finish_retire(retiring);
        app.execute_viewer_context_ops(ops, None);
        app.viewer_contexts.table.finish_retire();
        assert_eq!(
            app.mounted_viewer_context_id(),
            Some(app.viewer_context_main())
        );
    }

    #[test]
    #[cfg(windows)]
    fn production_i7_transactions_end_with_a_mounted_projection() {
        let mut app = crate::app::tests::phase_c_support::setup_app();
        let built = app
            .build_viewer_context("production_i7_build", |_, _| BuildOutcome::Commit)
            .unwrap();
        assert!(app.mounted_viewer_context_id().is_some());
        app.with_viewer_context(built, |_| ()).unwrap();
        assert!(app.mounted_viewer_context_id().is_some());
        app.retire_context(built, "production_i7_retire", |_| ())
            .unwrap();
        assert!(app.mounted_viewer_context_id().is_some());
        let stashed = app.stash_mounted_and_start_fresh("production_i7_promote");
        assert_eq!(
            app.viewer_context_residence(stashed),
            ContextResidence::AtRest
        );
        assert_eq!(
            app.mounted_viewer_context_id(),
            Some(app.viewer_context_main())
        );
    }

    fn recover_build_after_interior_failpoint(app: &mut App) {
        app.viewer_contexts.table.pending = None;
        app.abort_viewer_context_build();
    }

    #[test]
    #[cfg(windows)]
    fn production_i8_abort_swap_panic_never_publishes_reserved_binding() {
        let mut app = crate::app::tests::phase_c_support::setup_app();
        let panic = catch_unwind(AssertUnwindSafe(|| {
            app.build_viewer_context("production_i8_abort", |app, _| {
                app.reserve_window_binding_for_build(706);
                arm_viewer_context_swap_interior_failpoint_for_test();
                BuildOutcome::Abort("inject_swap_panic")
            });
        }));
        assert!(panic.is_err());
        assert!(app.viewer_contexts.table.window_of.is_empty());
        assert!(app.viewer_contexts.table.context_of.is_empty());
        recover_build_after_interior_failpoint(&mut app);
    }

    #[test]
    #[cfg(windows)]
    fn production_i8_unwind_swap_panic_never_publishes_reserved_binding() {
        let mut app = crate::app::tests::phase_c_support::setup_app();
        let panic = catch_unwind(AssertUnwindSafe(|| {
            app.build_viewer_context("production_i8_unwind", |app, _| {
                app.reserve_window_binding_for_build(707);
                arm_viewer_context_swap_interior_failpoint_for_test();
                panic!("build body panic");
            });
        }));
        assert!(panic.is_err());
        assert!(app.viewer_contexts.table.window_of.is_empty());
        assert!(app.viewer_contexts.table.context_of.is_empty());
        recover_build_after_interior_failpoint(&mut app);
    }

    #[test]
    fn other_ids_excludes_the_projected_context_while_building() {
        let (mut table, mut projection) = harness(11);
        let main = table.main();
        let reserved = begin_build(&mut table, &mut projection);

        // While building, `mounted_id` is None, so a filter written against it keeps the
        // reserved id and the traversal walks into the payload it is already standing on.
        assert_eq!(table.mounted_id(), None);
        assert!(table.ids().contains(&reserved));
        assert!(!table.other_ids().contains(&reserved));
        assert_eq!(table.other_ids(), vec![main]);
    }

    #[test]
    fn build_commit_restores_previous_and_stashes_reserved() {
        let (mut table, mut projection) = harness(11);
        let previous = table.main();
        let reserved = begin_build(&mut table, &mut projection);
        projection.materialize(22);

        let ops = table.plan_commit_build();
        execute_ops(&mut table, &mut projection, &ops);
        assert_eq!(table.finish_commit_build(), reserved);

        assert_eq!(table.mounted_id(), Some(previous));
        assert_eq!(table.residence(reserved), ContextResidence::AtRest);
        assert_eq!(projection.tag, 11);
        assert_eq!(at_rest_payload(&table, reserved).tag, 22);
    }

    #[test]
    fn build_abort_restores_previous_retires_reserved_and_publishes_no_binding() {
        let (mut table, mut projection) = harness(11);
        let previous = table.main();
        let reserved = begin_build(&mut table, &mut projection);
        projection.materialize(22);
        table.reserve_window_binding_for_build(90);

        let ops = table.plan_abort_build();
        execute_ops(&mut table, &mut projection, &ops);
        table.finish_abort_build();

        assert_eq!(table.mounted_id(), Some(previous));
        assert_eq!(table.residence(reserved), ContextResidence::Retired);
        assert_eq!(projection.tag, 11);
        assert!(table.window_of.is_empty());
        assert!(table.context_of.is_empty());
        assert_eq!(table.locate_window_context(90), None);
    }

    #[test]
    fn uncommitted_allocated_id_is_retired_and_unallocated_id_is_unknown() {
        let (mut table, mut projection) = harness(1);
        let reserved = begin_build(&mut table, &mut projection);
        let never_allocated = ViewerContextId(reserved.serial() + 1);

        let ops = table.plan_abort_build();
        execute_ops(&mut table, &mut projection, &ops);
        table.finish_abort_build();

        assert_eq!(table.residence(reserved), ContextResidence::Retired);
        assert_eq!(table.residence(never_allocated), ContextResidence::Unknown);
    }

    #[test]
    fn build_binding_is_invisible_until_commit() {
        let (mut table, mut projection) = harness(1);
        let reserved = begin_build(&mut table, &mut projection);
        projection.materialize(2);

        table.reserve_window_binding_for_build(44);
        assert_eq!(table.locate_window_context(44), None);

        let ops = table.plan_commit_build();
        execute_ops(&mut table, &mut projection, &ops);
        table.finish_commit_build();

        assert_eq!(
            table.locate_window_context(44),
            Some((reserved, ContextResidence::AtRest))
        );
    }

    #[test]
    fn reserving_the_same_build_window_twice_is_idempotent() {
        let (mut table, mut projection) = harness(1);
        begin_build(&mut table, &mut projection);

        table.reserve_window_binding_for_build(7);
        table.reserve_window_binding_for_build(7);

        assert!(matches!(
            table.projection,
            Projection::Building {
                pending_bind: Some(7),
                ..
            }
        ));
    }

    #[test]
    #[should_panic(expected = "a build cannot reserve more than one window")]
    fn reserving_a_different_second_build_window_panics() {
        let (mut table, mut projection) = harness(1);
        begin_build(&mut table, &mut projection);
        table.reserve_window_binding_for_build(7);
        table.reserve_window_binding_for_build(8);
    }

    #[test]
    #[should_panic(expected = "window binding can only be reserved while building")]
    fn reserving_a_build_window_outside_building_panics() {
        let (mut table, _projection) = harness(1);
        table.reserve_window_binding_for_build(7);
    }

    #[test]
    fn mount_reentry_is_empty_and_mounting_another_context_round_trips() {
        let (mut table, mut projection) = harness(10);
        let original = table.main();

        let reentry_ops = table.plan_mount(original).unwrap();
        assert!(reentry_ops.is_empty());
        table.finish_mount();
        assert_eq!(table.mounted_id(), Some(original));
        assert_eq!(projection.tag, 10);

        let other = fork_at_rest(
            &mut table,
            &mut projection,
            ForkPolicy::MaterializedStillOpen,
        );
        let other_tag = at_rest_payload(&table, other).tag;

        let ops = table.plan_mount(other).unwrap();
        execute_ops(&mut table, &mut projection, &ops);
        table.finish_mount();
        assert_eq!(table.mounted_id(), Some(other));
        assert_eq!(projection.tag, other_tag);

        let ops = table.plan_mount(original).unwrap();
        execute_ops(&mut table, &mut projection, &ops);
        table.finish_mount();
        assert_eq!(table.mounted_id(), Some(original));
        assert_eq!(projection.tag, 10);
    }

    #[test]
    fn building_and_retiring_contexts_reject_mount_and_binding() {
        let (mut building_table, mut building_projection) = harness(1);
        let reserved = begin_build(&mut building_table, &mut building_projection);
        assert_eq!(
            building_table.plan_mount(reserved).unwrap_err(),
            MountError {
                id: reserved,
                residence: ContextResidence::Building,
            }
        );
        assert_eq!(
            building_table.begin_retire(reserved),
            Err(RetireError::Building)
        );

        let (mut retiring_table, mut retiring_projection) = harness(10);
        let retiring = fork_at_rest(
            &mut retiring_table,
            &mut retiring_projection,
            ForkPolicy::MaterializedStillOpen,
        );
        retiring_table.begin_retire(retiring).unwrap();
        assert_eq!(
            retiring_table.plan_mount(retiring).unwrap_err(),
            MountError {
                id: retiring,
                residence: ContextResidence::Retiring,
            }
        );
        assert_eq!(
            retiring_table.bind_window(retiring, 50),
            Err(BindError::NotBindable(ContextResidence::Retiring))
        );
    }

    #[test]
    fn binding_enforces_bijection_idempotence_and_bindable_residence() {
        let (mut table, mut projection) = harness(1);
        let mounted = table.main();
        let at_rest = fork_at_rest(
            &mut table,
            &mut projection,
            ForkPolicy::MaterializedStillOpen,
        );

        assert_eq!(table.bind_window(mounted, 10), Ok(()));
        assert_eq!(table.bind_window(mounted, 10), Ok(()));
        assert_eq!(
            table.bind_window(at_rest, 10),
            Err(BindError::WindowOwnedBy(mounted))
        );
        assert_eq!(
            table.bind_window(mounted, 11),
            Err(BindError::ContextOwnedBy(10))
        );

        let reserved = begin_build(&mut table, &mut projection);
        assert_eq!(
            table.bind_window(reserved, 12),
            Err(BindError::NotBindable(ContextResidence::Building))
        );
    }

    #[test]
    fn transfer_moves_a_window_between_live_contexts_and_checks_origin() {
        let (mut table, mut projection) = harness(1);
        let from = table.main();
        let to = fork_at_rest(
            &mut table,
            &mut projection,
            ForkPolicy::MaterializedStillOpen,
        );
        table.bind_window(from, 20).unwrap();

        table.transfer_core(20, from, to).unwrap();
        assert_eq!(table.residence(from), ContextResidence::Mounted);
        assert_eq!(table.residence(to), ContextResidence::AtRest);
        assert_eq!(
            table.locate_window_context(20),
            Some((to, ContextResidence::AtRest))
        );
        assert_eq!(
            table.transfer_core(20, from, from),
            Err(BindError::WrongOrigin(Some(to)))
        );
    }

    #[test]
    fn unbind_allows_the_same_context_to_bind_a_different_window() {
        let (mut table, _projection) = harness(1);
        let id = table.main();
        table.bind_window(id, 30).unwrap();

        assert_eq!(table.unbind_window(30), Some(id));
        assert!(table.window_of.is_empty());
        assert!(table.context_of.is_empty());
        assert_eq!(table.bind_window(id, 31), Ok(()));
        assert_eq!(
            table.locate_window_context(31),
            Some((id, ContextResidence::Mounted))
        );
    }

    #[test]
    fn promote_rebinds_main_stashes_the_old_main_and_rejects_non_main_projection() {
        let (mut table, mut projection) = harness(5);
        let old_main = table.main();

        let ops = table.plan_promote();
        execute_ops(&mut table, &mut projection, &ops);
        assert_eq!(table.finish_promote(), old_main);

        let fresh_main = table.main();
        assert_ne!(fresh_main, old_main);
        assert_eq!(table.mounted_id(), Some(fresh_main));
        assert_eq!(table.residence(old_main), ContextResidence::AtRest);
        assert_eq!(at_rest_payload(&table, old_main).tag, 5);
        assert_eq!(table.bind_window(old_main, 60), Ok(()));

        let ops = table.plan_mount(old_main).unwrap();
        execute_ops(&mut table, &mut projection, &ops);
        table.finish_mount();
        let result = catch_unwind(AssertUnwindSafe(|| table.plan_promote()));
        assert!(result.is_err());
    }

    #[test]
    fn retire_exposes_only_retiring_payload_then_retires_and_unbinds_it() {
        let (mut table, mut projection) = harness(1);
        let retiring = fork_at_rest(
            &mut table,
            &mut projection,
            ForkPolicy::MaterializedStillOpen,
        );
        let still_at_rest = fork_at_rest(
            &mut table,
            &mut projection,
            ForkPolicy::MaterializedStillOpen,
        );
        table.bind_window(retiring, 70).unwrap();

        assert!(table.retiring_slot_mut(retiring).is_none());
        assert!(table.retiring_slot_mut(still_at_rest).is_none());
        table.begin_retire(retiring).unwrap();
        assert_eq!(table.residence(retiring), ContextResidence::Retiring);
        table.retiring_slot_mut(retiring).unwrap().tag = 77;
        assert_eq!(table.retiring_slot_mut(retiring).unwrap().tag, 77);
        assert!(table.retiring_slot_mut(still_at_rest).is_none());

        let ops = table.plan_finish_retire(retiring);
        execute_ops(&mut table, &mut projection, &ops);
        table.finish_retire();

        assert_eq!(table.residence(retiring), ContextResidence::Retired);
        assert_eq!(table.locate_window_context(70), None);
        assert_eq!(table.residence(still_at_rest), ContextResidence::AtRest);
    }

    #[test]
    fn retire_trace_records_unbind_before_drop_and_maps_are_already_clear() {
        let trace = Rc::new(RefCell::new(Vec::new()));
        let mut table = ContextTable::new();
        let mut projection = TestPayload::traced(1, trace.clone());
        let retiring = fork_at_rest(
            &mut table,
            &mut projection,
            ForkPolicy::MaterializedStillOpen,
        );
        table.bind_window(retiring, 71).unwrap();
        table.begin_retire(retiring).unwrap();

        let ops = table.plan_finish_retire(retiring);
        execute_ops(&mut table, &mut projection, &ops);
        assert!(!table.window_of.contains_key(&retiring));
        assert!(!table.context_of.contains_key(&71));
        assert_eq!(
            trace.borrow().as_slice(),
            [Event::Unbind(retiring), Event::Drop(retiring)]
        );
        table.finish_retire();
    }

    #[test]
    fn retiring_at_rest_main_is_rejected_without_damaging_the_table() {
        let (mut table, mut projection) = harness(1);
        let main = table.main();
        let forked = fork_at_rest(
            &mut table,
            &mut projection,
            ForkPolicy::MaterializedStillOpen,
        );

        let ops = table.plan_mount(forked).unwrap();
        execute_ops(&mut table, &mut projection, &ops);
        table.finish_mount();

        assert_eq!(table.residence(main), ContextResidence::AtRest);
        assert_eq!(table.begin_retire(main), Err(RetireError::IsMain));
        assert_eq!(table.main(), main);
        assert_eq!(table.residence(main), ContextResidence::AtRest);
        assert!(table.ids().contains(&main));

        let ops = table.plan_mount(main).unwrap();
        execute_ops(&mut table, &mut projection, &ops);
        table.finish_mount();
        assert_eq!(table.mounted_id(), Some(main));
    }

    #[test]
    fn promoted_stashed_former_main_can_be_retired() {
        let (mut table, mut projection) = harness(1);
        let former_main = table.main();

        let ops = table.plan_promote();
        execute_ops(&mut table, &mut projection, &ops);
        assert_eq!(table.finish_promote(), former_main);
        assert_ne!(table.main(), former_main);

        table.begin_retire(former_main).unwrap();
        let ops = table.plan_finish_retire(former_main);
        execute_ops(&mut table, &mut projection, &ops);
        table.finish_retire();

        assert_eq!(table.residence(former_main), ContextResidence::Retired);
        assert!(!table.ids().contains(&former_main));
    }

    #[test]
    fn fork_policies_preserve_projection_and_live_park_transfers_inside_finish() {
        let (mut still_table, mut still_projection) = harness(3);
        let still_from = still_table.main();
        still_table.bind_window(still_from, 80).unwrap();
        let still_fork = fork_at_rest(
            &mut still_table,
            &mut still_projection,
            ForkPolicy::MaterializedStillOpen,
        );
        assert_eq!(still_table.mounted_id(), Some(still_from));
        assert_eq!(
            still_table.locate_window_context(80),
            Some((still_from, ContextResidence::Mounted))
        );
        assert_eq!(still_table.residence(still_fork), ContextResidence::AtRest);

        let (mut live_table, mut live_projection) = harness(4);
        let live_from = live_table.main();
        live_table.bind_window(live_from, 81).unwrap();
        let policy = ForkPolicy::LiveMediaPark { window_id: 81 };
        let (live_fork, ops) = live_table.plan_fork(policy);
        execute_ops(&mut live_table, &mut live_projection, &ops);
        assert_eq!(live_table.context_of.get(&81), Some(&live_from));
        assert_eq!(live_table.finish_fork(), live_fork);
        assert_eq!(
            live_table.locate_window_context(81),
            Some((live_fork, ContextResidence::AtRest))
        );
        assert_eq!(live_table.residence(live_from), ContextResidence::Mounted);

        let (mut invalid_table, _invalid_projection) = harness(5);
        invalid_table.bind_window(invalid_table.main(), 82).unwrap();
        let result = catch_unwind(AssertUnwindSafe(|| {
            invalid_table.plan_fork(ForkPolicy::LiveMediaPark { window_id: 83 })
        }));
        assert!(result.is_err());
    }

    #[test]
    fn operation_vectors_match_all_seven_transaction_contracts_exactly() {
        let (mut commit_table, mut commit_projection) = harness(1);
        let commit_previous = commit_table.main();
        let (commit_reserved, begin_ops) = commit_table.plan_begin_build();
        assert_eq!(
            begin_ops,
            vec![
                TableOp::ReplaceProjectionWithFreshEmpty,
                TableOp::DepositInto(commit_previous),
            ]
        );
        execute_ops(&mut commit_table, &mut commit_projection, &begin_ops);
        commit_table.finish_begin_build();
        commit_projection.materialize(2);
        let commit_ops = commit_table.plan_commit_build();
        assert_eq!(
            commit_ops,
            vec![
                TableOp::ReplaceProjectionWithFreshEmpty,
                TableOp::DepositInto(commit_reserved),
                TableOp::WithdrawFrom(commit_previous),
                TableOp::RestoreProjectionAndDropDisplacedEmpty,
            ]
        );
        execute_ops(&mut commit_table, &mut commit_projection, &commit_ops);
        commit_table.finish_commit_build();

        let (mut abort_table, mut abort_projection) = harness(3);
        let abort_previous = abort_table.main();
        let (abort_reserved, begin_ops) = abort_table.plan_begin_build();
        execute_ops(&mut abort_table, &mut abort_projection, &begin_ops);
        abort_table.finish_begin_build();
        abort_projection.materialize(4);
        let abort_ops = abort_table.plan_abort_build();
        assert_eq!(
            abort_ops,
            vec![
                TableOp::ReplaceProjectionWithFreshEmpty,
                TableOp::DepositInto(abort_reserved),
                TableOp::WithdrawFrom(abort_previous),
                TableOp::RestoreProjectionAndDropDisplacedEmpty,
                TableOp::WithdrawFrom(abort_reserved),
                TableOp::DropTransientAsRetired(abort_reserved),
            ]
        );
        execute_ops(&mut abort_table, &mut abort_projection, &abort_ops);
        abort_table.finish_abort_build();
        let (mut mount_table, mut mount_projection) = harness(5);
        let mount_from = mount_table.main();
        let fork_policy = ForkPolicy::MaterializedStillOpen;
        let (forked, fork_ops) = mount_table.plan_fork(fork_policy);
        assert_eq!(
            fork_ops,
            vec![
                TableOp::ForkProjectionIntoTransient(fork_policy),
                TableOp::DepositInto(forked),
            ]
        );
        execute_ops(&mut mount_table, &mut mount_projection, &fork_ops);
        mount_table.finish_fork();

        let (mut live_fork_table, mut live_fork_projection) = harness(7);
        let live_fork_from = live_fork_table.main();
        let live_fork_policy = ForkPolicy::LiveMediaPark { window_id: 82 };
        live_fork_table.bind_window(live_fork_from, 82).unwrap();
        let (live_forked, live_fork_ops) = live_fork_table.plan_fork(live_fork_policy);
        assert_eq!(
            live_fork_ops,
            vec![
                TableOp::ForkProjectionIntoTransient(live_fork_policy),
                TableOp::DepositInto(live_forked),
            ]
        );
        execute_ops(
            &mut live_fork_table,
            &mut live_fork_projection,
            &live_fork_ops,
        );
        assert_eq!(at_rest_payload(&live_fork_table, live_forked).tag, 1_007);
        assert_eq!(live_fork_table.finish_fork(), live_forked);

        let mount_ops = mount_table.plan_mount(forked).unwrap();
        assert_eq!(
            mount_ops,
            vec![
                TableOp::ReplaceProjectionWithFreshEmpty,
                TableOp::DepositInto(mount_from),
                TableOp::WithdrawFrom(forked),
                TableOp::RestoreProjectionAndDropDisplacedEmpty,
            ]
        );
        execute_ops(&mut mount_table, &mut mount_projection, &mount_ops);
        mount_table.finish_mount();

        let (mut promote_table, mut promote_projection) = harness(6);
        let promote_stashed = promote_table.main();
        let promote_ops = promote_table.plan_promote();
        assert_eq!(
            promote_ops,
            vec![
                TableOp::ReplaceProjectionWithFreshEmpty,
                TableOp::DepositInto(promote_stashed),
            ]
        );
        execute_ops(&mut promote_table, &mut promote_projection, &promote_ops);
        promote_table.finish_promote();

        commit_table.begin_retire(commit_reserved).unwrap();
        let retire_ops = commit_table.plan_finish_retire(commit_reserved);
        assert_eq!(
            retire_ops,
            vec![
                TableOp::WithdrawFrom(commit_reserved),
                TableOp::DropTransientAsRetired(commit_reserved),
            ]
        );
        execute_ops(&mut commit_table, &mut commit_projection, &retire_ops);
        commit_table.finish_retire();
    }

    #[test]
    fn failpoint_sweep_never_publishes_build_binding_before_finish() {
        const BEGIN_FAILPOINTS: usize = 2;
        const COMMIT_FAILPOINTS: usize = 4;
        const ABORT_FAILPOINTS: usize = 6;

        for fail_after in 1..=BEGIN_FAILPOINTS {
            let (mut table, mut projection) = harness(1);
            let (_reserved, ops) = table.plan_begin_build();
            let result = catch_unwind(AssertUnwindSafe(|| {
                execute_ops_with_failpoint(&mut table, &mut projection, &ops, Some(fail_after));
            }));
            assert!(result.is_err());
            assert!(table.window_of.is_empty());
            assert!(table.context_of.is_empty());
        }

        for fail_after in 1..=COMMIT_FAILPOINTS {
            let (mut table, mut projection) = harness(2);
            begin_build(&mut table, &mut projection);
            projection.materialize(3);
            table.reserve_window_binding_for_build(100);
            let ops = table.plan_commit_build();
            let result = catch_unwind(AssertUnwindSafe(|| {
                execute_ops_with_failpoint(&mut table, &mut projection, &ops, Some(fail_after));
            }));
            assert!(result.is_err());
            assert!(table.window_of.is_empty());
            assert!(table.context_of.is_empty());
        }

        for fail_after in 1..=ABORT_FAILPOINTS {
            let (mut table, mut projection) = harness(4);
            begin_build(&mut table, &mut projection);
            projection.materialize(5);
            table.reserve_window_binding_for_build(101);
            let ops = table.plan_abort_build();
            let result = catch_unwind(AssertUnwindSafe(|| {
                execute_ops_with_failpoint(&mut table, &mut projection, &ops, Some(fail_after));
            }));
            assert!(result.is_err());
            assert!(table.window_of.is_empty());
            assert!(table.context_of.is_empty());
        }
    }

    #[test]
    fn ids_include_the_projection_and_are_sorted_deterministically() {
        let (mut table, mut projection) = harness(1);
        let main = table.main();
        let second = fork_at_rest(
            &mut table,
            &mut projection,
            ForkPolicy::MaterializedStillOpen,
        );
        let third = fork_at_rest(
            &mut table,
            &mut projection,
            ForkPolicy::MaterializedStillOpen,
        );
        assert_eq!(table.ids(), vec![main, second, third]);

        let reserved = begin_build(&mut table, &mut projection);
        assert_eq!(table.ids(), vec![main, second, third, reserved]);
    }

    #[test]
    fn residence_distinguishes_all_six_states() {
        let (mut table, mut projection) = harness(1);
        let mounted = table.main();
        assert_eq!(table.residence(mounted), ContextResidence::Mounted);

        let at_rest = fork_at_rest(
            &mut table,
            &mut projection,
            ForkPolicy::MaterializedStillOpen,
        );
        assert_eq!(table.residence(at_rest), ContextResidence::AtRest);

        let retiring = fork_at_rest(
            &mut table,
            &mut projection,
            ForkPolicy::MaterializedStillOpen,
        );
        table.begin_retire(retiring).unwrap();
        assert_eq!(table.residence(retiring), ContextResidence::Retiring);

        let reserved = begin_build(&mut table, &mut projection);
        assert_eq!(table.residence(reserved), ContextResidence::Building);

        let ops = table.plan_abort_build();
        execute_ops(&mut table, &mut projection, &ops);
        table.finish_abort_build();
        assert_eq!(table.residence(reserved), ContextResidence::Retired);

        let unknown = ViewerContextId(table.next_serial);
        assert_eq!(table.residence(unknown), ContextResidence::Unknown);
    }
}
