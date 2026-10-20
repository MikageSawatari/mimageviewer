use super::*;
use std::collections::HashMap;

impl App {
    /// Terminal ownership boundary, shared by accepted viewer closes and context retirement.
    /// Content-only close/reopen never comes here. Workers/list adoption remain alive.
    pub(super) fn retire_viewer_reopen_requests(&mut self, id: ViewerContextId) {
        if id == self.projected_viewer_context_id() {
            self.retire_book_deferred_open();
            if let Some(state) = self.epub_convert.as_mut() {
                state.retire_fullscreen_reopen();
            }
        } else {
            #[cfg(windows)]
            if let Some(bundle) = self.viewer_contexts.table.retiring_slot_mut(id) {
                bundle.book_composition_permission.retire_deferred_open();
                if let Some(state) = bundle.epub_convert.as_mut() {
                    state.retire_fullscreen_reopen();
                }
            }
        }
        // This state is App-global, unlike the book permission in the context bundle.
        // Do not release the currently projected sibling's navigation lock/holdover.
        if self
            .archive_convert
            .as_ref()
            .and_then(|state| state.deferred_fullscreen.as_ref())
            .is_some_and(|deferred| deferred.owner_context == id)
        {
            self.archive_convert.as_mut().unwrap().deferred_fullscreen = None;
        }
    }
}

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

    /// Private reservation for the builder, published ownership for a mounted payload.
    /// The reservation stays invisible to locate_window_context until commit (I8).
    fn projected_window(&self) -> Option<u64> {
        assert!(self.pending.is_none());
        match self.projection {
            Projection::Mounted(id) => self.window_for_context(id),
            Projection::Building { pending_bind, .. } => pending_bind,
        }
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

    fn at_rest_mut(&mut self, id: ViewerContextId) -> Option<&mut P> {
        assert!(self.pending.is_none());
        match self.slots.get_mut(&id) {
            Some(Slot::AtRest(payload)) => Some(payload),
            Some(Slot::Retiring(_)) | None => None,
        }
    }
}

fn reset_audio_art_terminal_rows(
    items: &[GridItem],
    parent: Option<&Path>,
    thumbnails: &mut [ThumbnailState],
    requested: &mut ThumbnailRequests,
) -> usize {
    let mut changed = 0;
    for (idx, (item, state)) in items.iter().zip(thumbnails.iter_mut()).enumerate() {
        let GridItem::Audio(path) = item else {
            continue;
        };
        if parent.is_some_and(|scope| {
            !path
                .parent()
                .is_some_and(|actual| crate::path_key::eq_keep_drive(actual, scope))
        }) || !matches!(state, ThumbnailState::NoArt | ThumbnailState::Failed)
        {
            continue;
        }
        *state = ThumbnailState::Evicted;
        requested.remove(&idx);
        changed += 1;
    }
    changed
}

impl App {
    pub(super) fn take_parked_page_edit_commits(&mut self) -> Vec<page_edit_owner::PageEditCommit> {
        let mut commits = Vec::new();
        #[cfg(windows)]
        for slot in self.viewer_contexts.table.slots.values_mut() {
            let bundle = match slot {
                Slot::AtRest(bundle) | Slot::Retiring(bundle) => bundle,
            };
            commits
                .extend(bundle.take_page_edit_commits(page_edit_owner::PageEditBoundary::History));
        }
        commits
    }

    pub(super) fn take_parked_page_edit_exit_commits(
        &mut self,
    ) -> Vec<page_edit_owner::PageEditCommit> {
        let mut commits = Vec::new();
        #[cfg(windows)]
        for slot in self.viewer_contexts.table.slots.values_mut() {
            let bundle = match slot {
                Slot::AtRest(bundle) | Slot::Retiring(bundle) => bundle,
            };
            commits.extend(bundle.take_page_edit_commits(page_edit_owner::PageEditBoundary::Exit));
        }
        commits
    }

    pub(crate) fn has_page_edit_producers(&self) -> bool {
        if self.local_adjust_lut_pending.is_some()
            || self.local_adjust_segmentation_pending.is_some()
        {
            return true;
        }
        #[cfg(windows)]
        for slot in self.viewer_contexts.table.slots.values() {
            let bundle = match slot {
                Slot::AtRest(bundle) | Slot::Retiring(bundle) => bundle,
            };
            if bundle.local_adjust_lut_pending.is_some()
                || bundle.local_adjust_segmentation_pending.is_some()
            {
                return true;
            }
        }
        false
    }

    pub(crate) fn poll_all_page_edit_producers(&mut self, ctx: &egui::Context) {
        self.poll_local_adjust_lut_load(ctx);
        self.poll_local_adjust_segmentation(ctx);
        #[cfg(windows)]
        {
            let owners = self
                .viewer_contexts
                .table
                .slots
                .iter()
                .filter_map(|(id, slot)| {
                    let bundle = match slot {
                        Slot::AtRest(bundle) | Slot::Retiring(bundle) => bundle,
                    };
                    (bundle.local_adjust_lut_pending.is_some()
                        || bundle.local_adjust_segmentation_pending.is_some())
                    .then_some(*id)
                })
                .collect::<Vec<_>>();
            for owner in owners {
                self.with_owner_viewer_context(owner, |app| {
                    app.poll_local_adjust_lut_load(ctx);
                    app.poll_local_adjust_segmentation(ctx);
                });
            }
        }
    }

    pub(super) fn cancel_superseded_page_edit_producers(
        &mut self,
        key: &str,
        layers: &local_adjust_core::LocalAdjustmentLayers,
        accepted: bool,
    ) {
        let mut cancelled = false;
        macro_rules! cancel {
            ($state:expr) => {{
                let state = $state;
                if state
                    .local_adjust_lut_pending
                    .as_ref()
                    .is_some_and(|p| p.owner.target.page_key == key && p.document != *layers)
                {
                    state.local_adjust_lut_pending = None;
                    cancelled = true;
                }
                if state
                    .local_adjust_segmentation_pending
                    .as_ref()
                    .is_some_and(|p| p.owner.target.page_key == key && p.document != *layers)
                {
                    state.local_adjust_segmentation_pending = None;
                    cancelled = true;
                }
            }};
        }
        cancel!(&mut *self);
        #[cfg(windows)]
        if accepted {
            for slot in self.viewer_contexts.table.slots.values_mut() {
                let bundle = match slot {
                    Slot::AtRest(bundle) | Slot::Retiring(bundle) => bundle,
                };
                cancel!(bundle);
            }
        }
        if cancelled {
            crate::logger::log(format!(
                "local-adjust producer superseded by content change: key={key}"
            ));
            self.show_feedback_warning(
                "LUT／マスク生成結果を破棄しました。レイヤーが変更されています".to_owned(),
            );
        }
    }

    /// Publish accepted page parameters in place; live drags keep their own projection.
    pub(crate) fn publish_accepted_adjustment_page_edits(
        &mut self,
        documents: &HashMap<String, Option<crate::adjustment::AdjustParams>>,
    ) {
        if documents.is_empty() {
            return;
        }
        let mut color_keys = std::collections::HashSet::new();
        let mut adjustment_keys = std::collections::HashSet::new();
        macro_rules! publish {
            ($state:expr, $default:expr) => {{
                let state = $state;
                let live = state
                    .adjustment_drag_session
                    .as_ref()
                    .map(|session| (session.owner.target.page_key.clone(), session.draft.clone()));
                let mut changed = false;
                let mut changed_indices = Vec::new();
                // One item pass for the entire batch, including duplicate slots.
                for idx in 0..state.items.len() {
                    let key = if let Some(snapshot) = &state.page_edit_snapshot {
                        snapshot.order.key_at(idx).map(str::to_owned)
                    } else {
                        crate::edit_source::page_key_for_grid_item(&state.items[idx])
                    };
                    let Some(key) = key else { continue };
                    let Some(accepted) = documents.get(&key) else {
                        continue;
                    };
                    if let Some(snapshot) = state.page_edit_snapshot.as_mut()
                        && snapshot.adjustment.get(&key) != accepted.as_ref()
                    {
                        if let Some(params) = accepted {
                            snapshot.adjustment.insert(key.clone(), params.clone());
                        } else {
                            snapshot.adjustment.remove(&key);
                        }
                        changed = true;
                    }
                    let projected = live
                        .as_ref()
                        .filter(|(live_key, _)| live_key == &key)
                        .map(|(_, draft)| draft)
                        .unwrap_or(accepted);
                    if state.adjustment_page_params.get(&idx) == projected.as_ref() {
                        continue;
                    }
                    let default = ($default)(state, idx);
                    let old = state.adjustment_page_params.get(&idx).unwrap_or(&default);
                    let new = projected.as_ref().unwrap_or(&default);
                    if old != new {
                        let final_only =
                            old.ai_settings_eq(new) && old.differs_only_in_final_stage(new);
                        let ai_changed = !old.ai_settings_eq(new);
                        if !old.color_settings_eq(new) {
                            color_keys.insert(key.clone());
                        }
                        adjustment_keys.insert(key.clone());
                        changed_indices.push((idx, final_only, ai_changed));
                    }
                    if let Some(params) = projected {
                        state.adjustment_page_params.insert(idx, params.clone());
                    } else {
                        state.adjustment_page_params.remove(&idx);
                    }
                    changed = true;
                }
                // A retained drag may be absent from the current item order.
                if let Some((key, _)) = &live
                    && let Some(accepted) = documents.get(key)
                    && let Some(snapshot) = state.page_edit_snapshot.as_mut()
                    && snapshot.adjustment.get(key) != accepted.as_ref()
                {
                    if let Some(params) = accepted {
                        snapshot.adjustment.insert(key.clone(), params.clone());
                    } else {
                        snapshot.adjustment.remove(key);
                    }
                    changed = true;
                }
                let affected: std::collections::HashSet<_> =
                    changed_indices.iter().map(|(idx, _, _)| *idx).collect();
                let upstream: std::collections::HashSet<_> = changed_indices
                    .iter()
                    .filter_map(|(idx, final_only, _)| (!final_only).then_some(*idx))
                    .collect();
                if !affected.is_empty() {
                    state
                        .adjustment_cache
                        .retain(|idx, _| !affected.contains(idx));
                    state
                        .final_composite_cache
                        .retain_with_drop_reason("accepted_adjustment_page_edit", |key, _| {
                            !affected.contains(&key.edit_key.idx)
                        });
                    state.final_effect_pending.retain(|key, pending| {
                        if affected.contains(&key.edit_key.idx) {
                            pending.cancel.store(true, Ordering::Relaxed);
                            false
                        } else {
                            true
                        }
                    });
                    // Final-only controls leave final AI inputs and jobs unchanged.
                    state
                        .final_ai_cache
                        .retain(|key, _| !upstream.contains(&key.edit_key.idx));
                    state
                        .final_ai_failed
                        .retain(|key| !upstream.contains(&key.edit_key.idx));
                    state.final_ai_pending.retain(|key, pending| {
                        if upstream.contains(&key.edit_key.idx) {
                            pending.cancel.store(true, Ordering::Relaxed);
                            false
                        } else {
                            true
                        }
                    });
                    state.comic_cache.retain(|idx, _| !affected.contains(idx));
                    state.comic_bake_pending.retain(|idx, pending| {
                        if affected.contains(idx) {
                            pending.cancel.store(true, Ordering::Relaxed);
                            false
                        } else {
                            true
                        }
                    });
                }
                if changed {
                    state.page_edit_revision = state.page_edit_revision.wrapping_add(1);
                }
                changed_indices
            }};
        }
        let mounted_changes = publish!(&mut *self, |state: &App, idx| {
            state.effective_default_for_idx(idx).clone()
        });
        for (idx, _, ai_changed) in mounted_changes {
            self.invalidate_compare_prepared_for_idx(idx);
            if ai_changed {
                // Legacy AI state is App-owned. Purge only the matching mounted slot.
                self.purge_upscale_for_idx(idx);
            }
        }
        #[cfg(windows)]
        {
            let book_root = self.book_root_path();
            let settings = &self.settings;
            let favorites = &self.adjustment_favorite_params;
            for slot in self.viewer_contexts.table.slots.values_mut() {
                let bundle = match slot {
                    Slot::AtRest(bundle) | Slot::Retiring(bundle) => bundle,
                };
                let _ = publish!(bundle, |state: &ViewerContextBundle, idx| {
                    let item = &state.items[idx];
                    let location = match item {
                        GridItem::Image(path) => {
                            if path.parent().is_some_and(|parent| {
                                crate::books::is_direct_book_folder(&book_root, parent)
                            }) {
                                return App::book_page_default_adjust_params().clone();
                            }
                            path.parent()
                        }
                        GridItem::ZipImage { zip_path, .. } => Some(zip_path.as_path()),
                        GridItem::PdfPage { pdf_path, .. } => Some(pdf_path.as_path()),
                        _ => None,
                    };
                    location
                        .and_then(|path| {
                            crate::final_composite::active_favorite_default_id_for_path(
                                path,
                                &settings.favorites,
                                None,
                                |id| favorites.contains_key(&id),
                            )
                        })
                        .and_then(|id| favorites.get(&id))
                        .unwrap_or(&settings.global_preset)
                        .clone()
                });
            }
        }
        for key in adjustment_keys {
            let generation = self.adjustment_generation.entry(key).or_default();
            *generation = generation.saturating_add(1);
        }
        self.invalidate_thumb_adjust_for_page_keys(&color_keys);
    }

    /// Only cached cells are inspected, including representatives of this page.
    pub(crate) fn invalidate_thumb_adjust_for_page_key(&mut self, key: &str) {
        self.invalidate_thumb_adjust_for_page_keys(&std::collections::HashSet::from([
            key.to_owned()
        ]));
    }

    fn invalidate_thumb_adjust_for_page_keys(&mut self, keys: &std::collections::HashSet<String>) {
        if keys.is_empty() {
            return;
        }
        macro_rules! invalidate {
            ($state:expr) => {{
                let state = $state;
                state.thumb_adjust_tex.retain(|idx, _| {
                    !state
                        .thumb_edit_preview_keys
                        .get(idx)
                        .is_some_and(|preview| keys.contains(preview))
                        && !state
                            .items
                            .get(*idx)
                            .and_then(crate::edit_source::page_key_for_grid_item)
                            .is_some_and(|key| keys.contains(&key))
                });
            }};
        }
        invalidate!(&mut *self);
        #[cfg(windows)]
        for slot in self.viewer_contexts.table.slots.values_mut() {
            let bundle = match slot {
                Slot::AtRest(bundle) | Slot::Retiring(bundle) => bundle,
            };
            invalidate!(bundle);
        }
        self.pinned_adjustment_refresh_keys
            .extend(keys.iter().cloned());
    }

    /// Reopen baked representatives in every owner before consuming the shared refresh keys.
    pub(super) fn evict_pinned_adjustment_materializations(
        &mut self,
        keys: &std::collections::HashSet<String>,
    ) {
        if keys.is_empty() {
            return;
        }
        let main = self.viewer_context_main();
        let mounted_is_main = self.projected_viewer_context_id() == main;
        let mut color_filter_keys = std::collections::HashSet::new();
        macro_rules! evict {
            ($state:expr, $is_main:expr) => {{
                let state = $state;
                let matching: std::collections::HashSet<_> = state
                    .thumb_edit_preview_keys
                    .iter()
                    .filter_map(|(&idx, key)| {
                        (keys.contains(key)
                            && state
                                .items
                                .get(idx)
                                .is_some_and(|item| !item.has_page_data()))
                        .then_some(idx)
                    })
                    .collect();
                if !matching.is_empty() {
                    // The queues belong to this payload. Cancel matching queued requests in
                    // one pass per queue; other contexts and non-grid requests remain intact.
                    for queue in [&state.reload_queue, &state.heavy_io_queue]
                        .into_iter()
                        .flatten()
                    {
                        if let Ok(mut pending) = queue.0.lock() {
                            pending.retain(|request| {
                                !request
                                    .grid_index()
                                    .is_some_and(|idx| matching.contains(&idx))
                            });
                        }
                    }
                    for idx in matching {
                        if $is_main
                            && let Some(item) = state.items.get(idx)
                            && super::color_filter::color_filter_representative_supported(item)
                        {
                            color_filter_keys.insert(item.perf_key());
                        }
                        // Recorded representative keys follow the existing reload path and
                        // remain until a newly loaded representative updates them.
                        state.requested.remove(&idx);
                        state.thumb_pixels.remove(&idx);
                        state.thumb_edit_preview_layers.remove(&idx);
                        state.thumb_adjust_tex.remove(&idx);
                        state.idle_upgrade_cache_bypass_ineligible.remove(&idx);
                        if let Some(thumbnail) = state.thumbnails.get_mut(idx) {
                            *thumbnail = ThumbnailState::Evicted;
                        }
                    }
                }
            }};
        }
        evict!(&mut *self, mounted_is_main);
        #[cfg(windows)]
        for (id, slot) in self.viewer_contexts.table.slots.iter_mut() {
            let bundle = match slot {
                Slot::AtRest(bundle) | Slot::Retiring(bundle) => bundle,
            };
            evict!(bundle, *id == main);
        }
        // Color filtering is shared main-grid state, even while main is parked.
        if !color_filter_keys.is_empty() {
            for key in color_filter_keys {
                let revision = self
                    .color_filter
                    .palettes
                    .representative_revisions
                    .entry(key)
                    .or_default();
                *revision = revision.wrapping_add(1);
            }
            self.mark_color_filter_scope_dirty();
        }
    }

    pub(super) fn publish_finished_adjustment_page_edit(
        &mut self,
        owner: &PageEditOwner,
        params: &Option<crate::adjustment::AdjustParams>,
    ) {
        macro_rules! publish {
            ($state:expr) => {{
                let state = $state;
                if state.items_generation != owner.items_generation {
                    return;
                }
                let mut changed = false;
                for idx in &owner.slots {
                    if state.adjustment_page_params.get(idx) == params.as_ref() {
                        continue;
                    }
                    if let Some(params) = params {
                        state.adjustment_page_params.insert(*idx, params.clone());
                    } else {
                        state.adjustment_page_params.remove(idx);
                    }
                    state.adjustment_cache.remove(idx);
                    state.thumb_adjust_tex.remove(idx);
                    changed = true;
                }
                if changed {
                    if let Some(snapshot) = state.page_edit_snapshot.as_mut() {
                        if let Some(params) = params {
                            snapshot
                                .adjustment
                                .insert(owner.target.page_key.clone(), params.clone());
                        } else {
                            snapshot.adjustment.remove(&owner.target.page_key);
                        }
                    }
                    state.page_edit_revision = state.page_edit_revision.wrapping_add(1);
                }
            }};
        }
        if self.projected_viewer_context_id() == owner.context {
            publish!(&mut *self);
            return;
        }
        #[cfg(windows)]
        if let Some(Slot::AtRest(bundle) | Slot::Retiring(bundle)) =
            self.viewer_contexts.table.slots.get_mut(&owner.context)
        {
            publish!(bundle);
        }
    }

    pub(super) fn publish_accepted_local_page_edits(
        &mut self,
        documents: &HashMap<String, local_adjust_core::LocalAdjustmentLayers>,
    ) {
        if documents.is_empty() {
            return;
        }
        // Pending is key-owned. Idle contexts project the latest accepted
        // document until that same key has no outstanding accepted write.
        let pending_keys: std::collections::HashSet<&str> = documents
            .keys()
            .filter(|key| self.local_adjust_write_pending.contains_key(*key))
            .map(String::as_str)
            .collect();
        fn update_snapshot(
            snapshot: Option<&mut page_edit_snapshot::PageEditSnapshot>,
            key: &str,
            present: bool,
            pending: bool,
        ) -> bool {
            let Some(snapshot) = snapshot else {
                return false;
            };
            let changed = if present {
                snapshot.local_adjust.insert(key.to_owned())
            } else {
                snapshot.local_adjust.remove(key)
            };
            let volatile_changed = if pending {
                snapshot.volatile_keys.insert(key.to_owned())
            } else {
                snapshot.volatile_keys.remove(key)
            };
            changed || volatile_changed
        }
        macro_rules! publish {
            ($state:expr) => {{
                let state = $state;
                let live = state.local_adjust_shape_key_edit.is_some()
                    || state.local_adjust_shape_drag.is_some()
                    || state.local_adjust_canvas_drag.is_some()
                    || state.local_adjust_mask_brush_stroke.is_some();
                let live_key = if live {
                    match &state.local_page_edit_document {
                        LocalPageEditDocument::Editing { owner, .. } => {
                            Some(owner.target.page_key.clone())
                        }
                        _ => None,
                    }
                } else {
                    None
                };
                if let LocalPageEditDocument::Editing {
                    owner,
                    layers,
                    revision,
                    ..
                } = &mut state.local_page_edit_document
                    && live_key.as_ref() != Some(&owner.target.page_key)
                    && let Some(accepted) = documents.get(&owner.target.page_key)
                {
                    if layers != accepted {
                        *layers = accepted.clone();
                        *revision = revision.wrapping_add(1);
                    }
                }
                let mut snapshot_changed = false;
                // A captured owner, or a former volatile/present key, may be
                // absent from the current item order after streaming/reindex.
                let owner_key = match &state.local_page_edit_document {
                    LocalPageEditDocument::Selection { owner, .. }
                    | LocalPageEditDocument::Editing { owner, .. } => Some(&owner.target.page_key),
                    LocalPageEditDocument::Idle => None,
                };
                for (key, layers) in documents {
                    if live_key.as_ref() != Some(key)
                        && (owner_key == Some(key)
                            || state.page_edit_snapshot.as_ref().is_some_and(|snapshot| {
                                snapshot.volatile_keys.contains(key)
                                    || snapshot.local_adjust.contains(key)
                            }))
                    {
                        snapshot_changed |= update_snapshot(
                            state.page_edit_snapshot.as_mut(),
                            key,
                            !layers.is_empty(),
                            pending_keys.contains(key.as_str()),
                        );
                    }
                }
                let mut changed = std::collections::HashSet::new();
                for idx in 0..state.items.len() {
                    let key = if let Some(snapshot) = &state.page_edit_snapshot {
                        snapshot.order.key_at(idx).map(str::to_owned)
                    } else {
                        crate::edit_source::page_key_for_grid_item(&state.items[idx])
                    };
                    let Some(key) = key else { continue };
                    if live_key.as_ref() == Some(&key) {
                        continue;
                    }
                    let Some(layers) = documents.get(&key) else {
                        continue;
                    };
                    snapshot_changed |= update_snapshot(
                        state.page_edit_snapshot.as_mut(),
                        &key,
                        !layers.is_empty(),
                        pending_keys.contains(key.as_str()),
                    );
                    if (state
                        .local_adjust_page_layers
                        .get(&idx)
                        .is_some_and(|old| old == layers)
                        || (layers.is_empty()
                            && !state.local_adjust_page_layers.contains_key(&idx)))
                        && state.local_adjust_pages.contains(&idx) == !layers.is_empty()
                    {
                        continue;
                    }
                    if layers.is_empty() {
                        state.local_adjust_page_layers.remove(&idx);
                        state.local_adjust_pages.remove(&idx);
                    } else {
                        state.local_adjust_page_layers.insert(idx, layers.clone());
                        state.local_adjust_pages.insert(idx);
                    }
                    let generation = state.local_adjust_generation.entry(idx).or_default();
                    *generation = generation.wrapping_add(1);
                    changed.insert(idx);
                }
                if let LocalPageEditDocument::Selection {
                    owner, revision, ..
                } = &mut state.local_page_edit_document
                    && owner.slots.iter().any(|idx| changed.contains(idx))
                {
                    *revision = revision.wrapping_add(1);
                }
                state
                    .local_adjust_cache
                    .retain(|key, _| !changed.contains(&key.idx));
                if snapshot_changed || !changed.is_empty() {
                    state.page_edit_revision = state.page_edit_revision.wrapping_add(1);
                }
            }};
        }
        publish!(&mut *self);
        #[cfg(windows)]
        for slot in self.viewer_contexts.table.slots.values_mut() {
            let bundle = match slot {
                Slot::AtRest(bundle) | Slot::Retiring(bundle) => bundle,
            };
            publish!(bundle);
        }
    }

    pub(super) fn refresh_pending_page_edit_projections(&mut self) {
        let pending = self
            .local_adjust_write_pending
            .iter()
            .map(|(key, p)| (key.clone(), p.layers.clone()))
            .collect();
        self.publish_accepted_local_page_edits(&pending);
    }

    /// A global fit-mode change ends each context's selected continuous unit.
    /// Do this at the setting boundary: a parked context may not draw between
    /// two changes, so observing a later layout frame cannot detect the exit.
    pub(crate) fn invalidate_all_continuous_specified_fit_references(&mut self) {
        self.fs_continuous_fit_reference = Default::default();
        #[cfg(windows)]
        for slot in self.viewer_contexts.table.slots.values_mut() {
            let bundle = match slot {
                Slot::AtRest(bundle) | Slot::Retiring(bundle) => bundle,
            };
            bundle.fs_continuous_fit_reference = Default::default();
        }
    }

    #[cfg(all(test, windows))]
    pub(in crate::app) fn set_viewer_context_generation_for_test(
        &mut self,
        id: ViewerContextId,
        generation: u64,
    ) {
        match self.viewer_contexts.table.slots.get_mut(&id) {
            Some(Slot::AtRest(bundle)) => bundle.set_items_generation(generation),
            _ => panic!("generation fixture requires an at-rest context"),
        }
    }

    /// Explicit cache maintenance reopens only terminal audio requests. Mutate
    /// every living owner in place: no mount/swap, generation/cancel change,
    /// viewport routing, SQLite work, or loss of already displayed pixels.
    pub(crate) fn invalidate_audio_art_terminals_after_catalog_maintenance(
        &mut self,
        parent: Option<&Path>,
    ) -> usize {
        let mut changed = reset_audio_art_terminal_rows(
            &self.items,
            parent,
            &mut self.thumbnails,
            &mut self.requested,
        );
        #[cfg(windows)]
        for slot in self.viewer_contexts.table.slots.values_mut() {
            let Slot::AtRest(bundle) = slot else {
                continue;
            };
            changed += reset_audio_art_terminal_rows(
                &bundle.items,
                parent,
                &mut bundle.thumbnails,
                &mut bundle.requested,
            );
        }
        changed
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
    display_epub_source: Option<crate::pdf_loader::LeasedEpubPath>,
    fullscreen_epub_source: Option<crate::pdf_loader::LeasedEpubPath>,
    favorite_view_context: FavoriteViewContextState,
    navigation_scope: ViewerNavigationScope,
    archive_source_override: Option<PathBuf>,
    zip_nav: Option<crate::zip_tree::ZipNavState>,
    stack_mode_requested: bool,
    stack_view: Option<std::sync::Arc<crate::filename_stack::StackView>>,
    stack_showing_flat: bool,
    stack_return_state: Option<crate::filename_stack_ui::StackReturnState>,
    stack_script_pending: Option<crate::filename_stack_ui::StackPreparePending>,
    stack_request_sequence: u64,
    stack_active_rule: Option<String>,
    stack_script_error: Option<String>,
    stack_selection_target: Option<crate::filename_stack_ui::StackSelectionTarget>,
    items: Vec<GridItem>,
    page_identity_index: Arc<super::file_type_resume::PreparedPageIdentityIndex>,
    projection_recipe: super::book_materialization::ProjectionRecipe,
    current_book_bookmarks: Vec<crate::book_bookmarks::BookBookmark>,
    current_book_bookmarks_key: Option<BookBookmarkProjectionKey>,
    current_book_bookmarks_request: Option<CurrentBookBookmarksRequest>,
    book_bookmark_title_edit: Option<BookBookmarkTitleEdit>,
    bookmark_view_registered_total: usize,
    items_generation: u64,
    pdf_render_epoch: crate::pdf_loader::PdfRenderEpochOwner,
    pdf_visible_promotion: super::PdfVisiblePromotionState,
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
    requested: ThumbnailRequests,
    raw_thumb_develop: Arc<Mutex<ItemsGenerationMap<crate::thumb_loader::RawThumbPending>>>,
    idle_upgrade_cache_bypass_ineligible: std::collections::HashSet<usize>,
    keep_range: (usize, usize),
    keep_set: std::collections::HashSet<usize>,
    still_seek_thumbnail_pages: std::collections::HashSet<usize>,
    still_seek_thumbnail_pages_shared: Arc<std::sync::RwLock<std::collections::HashSet<usize>>>,
    thumbnail_eviction_generation: Option<u64>,
    details_thumb_suppression_applied: bool,
    details_hover_thumb_idx: Option<usize>,
    details_hover_thumb_viewport_open: bool,
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
    search_filter: Option<super::LocalSearchFilter>,
    search_filter_origin_folder: Option<PathBuf>,
    checked: std::collections::HashSet<usize>,
    rotation_cache: crate::rotation_cache::RotationCache,
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
    metadata_import_refresh_index: Option<MetadataImportRefreshState>,
    current_folder_rating_cache: Option<u8>,
    current_folder_last_mtime: Option<std::time::SystemTime>,
    current_folder_signature: Option<folder_scan::FolderScanSignature>,
    folder_observation: Option<FolderObservation>,
    current_folder_watch: Option<CurrentFolderWatch>,
    current_folder_watch_failed: Option<PathBuf>,
    external_rescan_pending: Option<ExternalRescanPending>,
    folder_refresh_pending: Option<book_append_refresh::FolderRefreshRequest>,
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
    /// 右ボタンの物理 press sequence。通常の AtRest 往復では同じ viewer と一緒に保持し、
    /// fork / close / index-space replacement では明示的に終端する。
    fs_secondary_press: crate::ui_fullscreen::FullscreenSecondaryPress,
    viewer_session: ViewerSession,
    native_video_in_window_active: bool,
    video_audio_mode: Option<usize>,
    video_audio_vst: Option<VideoAudioVstState>,
    video_audio_mode_runtime: Option<VideoAudioModeRuntime>,
    video_seek_strip_runtime: crate::app::native_video::VideoSeekStripRuntime,
    video_seek_strip_wave_holdover: Option<crate::app::native_video::HeldSeekStripWaveWorker>,
    video_seek_strip_next_session_id: u64,
    panorama_state: Option<crate::panorama::PanoramaState>,
    /// 現在の通常動画項目の拡大率と中心。presenter へ渡す値 snapshot の正本であり、
    /// context を mount した間だけ App field へ投影する。
    video_zoom_state: Option<crate::video::zoom_view::VideoZoomState>,
    /// Raw XButton seek holds persist with their viewer context through ordinary
    /// frame-to-frame AtRest deposits; semantic owner terminals clear them.
    native_video_mouse_seek_holds: crate::app::native_video::NativeVideoMouseSeekHolds,
    /// 360 で見ているという意図 (+ 選んだ投影方式)。フルスクリーンを閉じても残る
    /// ので、App グローバルに置くと別ウィンドウの 360 が混ざる (backlog §1.145)。
    panorama_intent: crate::panorama::PanoramaSessionIntent,
    /// 右情報パネルの表示状態 (明示 open / ロック / ホバー latch)。
    /// App-global に置くと別ウィンドウの操作で他方のパネルが開閉する (backlog §1.158)。
    fs_info_panel: crate::ui_helpers::FullscreenInfoPanelState,
    /// 情報パネルの runtime tab と類似画像サムネイル要求。worker/channel まで含めて
    /// viewer ごとに所有し、park 中の完了を別 viewer が消費しない。
    similar_panel: crate::ui_metadata_panel::SimilarPanelState,
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
    fullscreen_navigator_interaction: crate::ui_fullscreen::FullscreenNavigatorInteractionOwner,
    fullscreen_page_layout: crate::displayed_image_transform::FullscreenPageLayout,
    fs_margin_bbox_cache: std::collections::HashMap<usize, (u64, usize, Option<egui::Rect>)>,
    input_generation: std::collections::HashMap<usize, u64>,
    raw_pages: Box<RawPageStore>,
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
    global_search: crate::global_search_ui::GlobalSearchState,
    favsearch_subfolder_restore: Option<super::subfolder_expansion::SubfolderExpansionRestoreState>,
    items_are_global_search_view: bool,
    items_are_tag_view: bool,
    items_are_reading_history_view: bool,
    items_are_bookmark_view: bool,
    items_are_rating_view: bool,
    rating_view_stars: u8,
    rating_view_sort: crate::rating_view::RatingViewSort,
    rating_view_rows: Vec<crate::rating_view::RatingViewRow>,
    rating_view_rows_stars: Option<u8>,
    rating_view_pending: Option<crate::rating_view::RatingViewPending>,
    rating_view_request_sequence: u64,
    rating_view_accepted_write_generation: u64,
    rating_view_skipped: usize,
    rating_view_saved_folder: Option<PathBuf>,
    rating_view_subfolder_restore:
        Option<super::subfolder_expansion::SubfolderExpansionRestoreState>,
    rating_view_nav_stack: Vec<PathBuf>,
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
    fs_pdf_rerender_demand: PdfRerenderDemand,
    fs_continuous_fit_reference: crate::ui_fullscreen::ContinuousSpecifiedFitReference,
    fs_pan_drag_start: Option<(egui::Pos2, egui::Vec2)>,
    fs_vertical_scroll: f32,
    fs_seek_drag_active: bool,
    fs_seek_gesture: crate::ui_fullscreen::StillSeekGesture,
    fs_seek_overlay_visible: bool,
    fs_vertical_cache_keep_set: std::collections::HashSet<usize>,
    pending_fs_page_turn_ready: Option<crate::ui_fullscreen::PendingFsPageTurnReady>,
    page_turn_presentation: crate::ui_fullscreen::PageTurnPresentation,
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
    analysis_hist_cache: Option<Box<(f32, egui::Vec2, usize, [u32; 360], [u32; 256], [u32; 256])>>,
    analysis_sv_cache: Option<(f32, egui::Vec2, usize, egui::TextureHandle)>,
    spread_mode: crate::settings::SpreadMode,
    final_cover_spread_preference: crate::settings::FinalCoverSpreadPreference,
    singleton_spread_endpoint_preferences: crate::settings::SingletonSpreadEndpointPreferences,
    page_alone_preferences: crate::settings::PageAlonePreferences,
    pub(super) spread_shift: super::spread_shift_owner::ViewerSpreadShift,
    pub(super) book_composition: super::book_composition_runtime::ViewerBookComposition,
    pub(super) composition_editor: super::book_composition_editor::CompositionEditorState,
    pub(super) book_composition_permission:
        super::book_composition_permission::ViewerPresentationPermission,
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
    pdf_enumerate_pending: Option<super::PdfEnumeratePending>,
    epub_convert: Option<Box<crate::ui_dialogs::epub_convert::EpubConvertState>>,
    zip_enumerate_pending: Option<ZipEnumeratePending>,
    fs_nav_after_pdf_enumerate: Option<DeferredFsReopen>,
    pending_auto_fs_open: bool,
    pending_return_to_parent: super::FullscreenCloseContinuation,
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
    page_edit_revision: u64,
    local_page_edit_document: LocalPageEditDocument,
    local_adjust_spread_ctx: Option<LocalPageEditSpreadPivot>,
    adjustment_drag_session: Option<AdjustmentDragSession>,
    adjustment_standard_drag_session: Option<AdjustmentStandardDragSession>,
    adjustment_dragging: bool,
    thumb_adjust_drag_color_dirty: bool,
    adjustment_mode: crate::ui_helpers::MetadataPanelOpenState,
    adjust_spread_target: AdjustSpreadTarget,
    adjust_scope_selection: AdjustScopeSelection,
    adjust_scope_selection_idx: Option<usize>,
    local_adjust_mode: bool,
    local_adjust_show_mask: bool,
    local_adjust_selected_shape: Option<usize>,
    local_adjust_shape_drag: Option<LocalAdjustMaskShapeDrag>,
    local_adjust_canvas_drag: Option<LocalAdjustCanvasDrag>,
    local_adjust_mask_brush_stroke: Option<LocalAdjustMaskBrushStroke>,
    local_adjust_shape_drag_before_layers: Option<local_adjust_core::LocalAdjustmentLayers>,
    local_adjust_canvas_drag_before_layers: Option<local_adjust_core::LocalAdjustmentLayers>,
    local_adjust_mask_brush_before_layers: Option<local_adjust_core::LocalAdjustmentLayers>,
    local_adjust_shape_key_edit: Option<LocalAdjustShapeKeyEdit>,
    local_adjust_mask_lasso_points: Vec<[f32; 2]>,
    local_adjust_mask_shape_drag_start: Option<[f32; 2]>,
    local_adjust_mask_shape_drag_end: Option<[f32; 2]>,
    local_adjust_mask_edit_target: LocalAdjustMaskEditTarget,
    local_adjust_brush_deferred_render: Option<LocalAdjustBrushDeferredRender>,
    local_adjust_rgb_pick_active: Option<crate::local_adjust_effect_ui::RgbPickTarget>,
    local_adjust_repair_point_pick_active:
        Option<crate::local_adjust_effect_ui::RepairPointPickTarget>,
    local_adjust_selective_color_pick_active: bool,
    local_adjust_add_layer_dialog_open: bool,
    local_adjust_change_mask_dialog_open: bool,
    local_adjust_effect_picker_dialog_open: bool,
    local_adjust_lut_pending: Option<LocalAdjustLutLoadPending>,
    local_adjust_segmentation_pending: Option<LocalAdjustSegmentationPending>,
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
    page_edit_snapshot: Option<super::page_edit_snapshot::PageEditSnapshot>,
    page_edit_reconcile_pending: Option<super::page_edit_snapshot::ReconcileStatus>,
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
    normalize_ui_states: std::collections::HashMap<
        crate::app::normalize::NormalizeTargetKey,
        crate::video::normalize_types::NormalizeUiState,
    >,
    normalize_auto_scan_suppressed:
        std::collections::HashSet<crate::app::normalize::NormalizeTargetKey>,
    music_bookmarks: Vec<crate::video_bookmarks::VideoBookmarkMeta>,
    music_bookmarks_loaded_for: Option<PathBuf>,
    last_loop_pos: std::collections::HashMap<usize, (f64, u64)>,
}

#[cfg(windows)]
impl ViewerContextBundle {
    fn take_page_edit_commits(
        &mut self,
        boundary: page_edit_owner::PageEditBoundary,
    ) -> Vec<page_edit_owner::PageEditCommit> {
        use page_edit_owner::PageEditCommit;
        let mut commits = Vec::new();
        // Match the mounted terminal order, including the legacy legal
        // pointer/key overlap. No new same-page termination policy is imposed.
        macro_rules! local_commit {
            ($owner:expr, $before:expr, $summary:expr, $compact:expr) => {{
                let owner = $owner;
                let LocalPageEditDocument::Editing {
                    owner: document_owner,
                    layers,
                    revision,
                    ..
                } = &mut self.local_page_edit_document
                else {
                    panic!("local gesture has no owning document");
                };
                assert_eq!(document_owner.context, owner.context);
                assert_eq!(document_owner.target.page_key, owner.target.page_key);
                let mut after = layers.clone();
                if let Some(layer_idx) = $compact {
                    crate::ui_adjustment_panel::compact_local_adjust_manual_override_in(
                        &mut after, layer_idx,
                    );
                }
                let before = $before.unwrap_or_else(|| after.clone());
                *layers = after.clone();
                commits.push(PageEditCommit::Local {
                    origin: LocalAdjustWriteOrigin {
                        context: owner.context,
                        target: owner.target,
                        revision: *revision,
                    },
                    before,
                    after,
                    summary: $summary.to_owned(),
                });
            }};
        }
        if let Some(session) = self.local_adjust_shape_key_edit.take() {
            local_commit!(
                session.owner,
                Some(session.before),
                session.summary,
                None::<usize>
            );
        }
        if let Some(drag) = self.local_adjust_shape_drag.take() {
            local_commit!(
                drag.owner,
                self.local_adjust_shape_drag_before_layers.take(),
                "補正レイヤー図形マスク編集",
                Some(drag.layer_idx)
            );
        }
        if let Some(drag) = self.local_adjust_canvas_drag.take() {
            local_commit!(
                drag.owner,
                self.local_adjust_canvas_drag_before_layers.take(),
                "補正レイヤーキャンバス操作",
                None::<usize>
            );
        }
        if let Some(stroke) = self.local_adjust_mask_brush_stroke.take() {
            let before = self.local_adjust_mask_brush_before_layers.take();
            if stroke.applied {
                local_commit!(
                    stroke.owner,
                    before,
                    "補正レイヤー手描きマスク",
                    Some(stroke.layer_idx)
                );
            }
        }
        self.local_adjust_shape_drag_before_layers = None;
        self.local_adjust_canvas_drag_before_layers = None;
        self.local_adjust_mask_brush_before_layers = None;
        if boundary == page_edit_owner::PageEditBoundary::Exit {
            self.local_adjust_mask_lasso_points.clear();
            self.local_adjust_mask_shape_drag_start = None;
            self.local_adjust_mask_shape_drag_end = None;
        }
        self.local_adjust_brush_deferred_render = None;
        if let Some(session) = self.adjustment_drag_session.take() {
            commits.push(PageEditCommit::Page(session));
        }
        if let Some(session) = self.adjustment_standard_drag_session.take() {
            commits.push(PageEditCommit::Standard(session));
        }
        self.adjustment_dragging = false;
        commits
    }
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

/// Context-owned requests that must reach a terminal when a still viewer is parked. The variant
/// list is the single inventory for mounted service and park termination. Long-lived cache and
/// write workers have separate resume contracts, audited by the test below.
#[cfg(windows)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::app) enum ContextAsyncOwner {
    FolderObservation,
    BookCompositionRead,
    RetainedSpreadShift,
    PathClassification,
    HistoryTransition,
    CollectionGrid,
    CollectionNavigation,
    RatingNavigation,
    BookmarkOpen,
    FolderNavigation,
    FolderPaneScan,
    PdfEnumeration,
    ZipEnumeration,
    EpubConversion,
    PdfPassword,
    FullscreenDecode,
    RawPages,
    FinalAi,
    LocalAdjustment,
    ComicBake,
    EraseInpaint,
    SimilarPreview,
}

#[cfg(windows)]
#[derive(Clone, Copy, PartialEq, Eq)]
pub(in crate::app) enum ContextAsyncServicePhase {
    Background,
    Dialog,
}

#[cfg(windows)]
impl ContextAsyncOwner {
    pub(in crate::app) const ALL: [Self; 22] = [
        Self::FolderObservation,
        Self::BookCompositionRead,
        Self::RetainedSpreadShift,
        Self::EpubConversion,
        Self::PdfPassword,
        Self::PathClassification,
        Self::HistoryTransition,
        Self::CollectionGrid,
        Self::CollectionNavigation,
        Self::RatingNavigation,
        Self::BookmarkOpen,
        Self::FolderNavigation,
        Self::FolderPaneScan,
        Self::PdfEnumeration,
        Self::ZipEnumeration,
        Self::FullscreenDecode,
        Self::RawPages,
        Self::FinalAi,
        Self::LocalAdjustment,
        Self::ComicBake,
        Self::EraseInpaint,
        Self::SimilarPreview,
    ];

    pub(in crate::app) fn is_pending(self, context: ContextRef<'_>) -> bool {
        macro_rules! pending {
            ($owner:ident) => {
                match self {
                    Self::FolderObservation => $owner.external_rescan_pending.is_some(),
                    Self::BookCompositionRead => {
                        ($owner.book_composition.is_pending()
                            || $owner.book_composition_permission.pending())
                    }
                    Self::RetainedSpreadShift => $owner.spread_shift.retained_readoption_pending(),
                    Self::PathClassification => $owner
                        .top_level_grid_view
                        .open_path_classification()
                        .is_some(),
                    Self::HistoryTransition => $owner
                        .top_level_grid_view
                        .history_navigation_transition()
                        .is_some(),
                    Self::CollectionGrid => $owner
                        .top_level_grid_view
                        .collection_session()
                        .is_some_and(|session| {
                            matches!(
                                &session.load,
                                top_level_grid_view::CollectionGridLoadState::Snapshot { .. }
                                    | top_level_grid_view::CollectionGridLoadState::Preparing { .. }
                            )
                        }),
                    Self::CollectionNavigation => {
                        $owner.top_level_grid_view.collection_navigation_pending()
                    }
                    Self::RatingNavigation => $owner.rating_view_pending.is_some(),
                    Self::BookmarkOpen => $owner.bookmark_open_pending.is_some(),
                    Self::FolderNavigation => $owner.folder_nav_pending.is_some(),
                    Self::FolderPaneScan => $owner.folder_pane_open_pending.is_some(),
                    Self::PdfEnumeration => $owner.pdf_enumerate_pending.is_some(),
                    Self::ZipEnumeration => $owner.zip_enumerate_pending.is_some(),
                    Self::EpubConversion => $owner.epub_convert.is_some(),
                    Self::PdfPassword => $owner.pdf_password_request.is_some(),
                    Self::FullscreenDecode => $owner.fs_pending.iter().next().is_some(),
                    Self::RawPages => $owner.raw_pages.has_pending(),
                    Self::FinalAi => !$owner.final_ai_pending.is_empty(),
                    Self::LocalAdjustment => !$owner.local_adjust_pending.is_empty(),
                    Self::ComicBake => !$owner.comic_bake_pending.is_empty(),
                    Self::EraseInpaint => !$owner.erase_inpaint_pending.is_empty(),
                    Self::SimilarPreview => {
                        $owner.similar_panel.preview.has_pending_background_work()
                    }
                }
            };
        }
        match context.source {
            ContextRefSource::Mounted(owner) => pending!(owner),
            ContextRefSource::AtRest(owner) => pending!(owner),
        }
    }

    /// Composition/collection/rating/bookmark polls also admit work from their current scope.
    /// All other owners need a live request before a mounted frame calls their worker service.
    pub(in crate::app) fn needs_mounted_service(self, context: ContextRef<'_>) -> bool {
        if matches!(self, Self::FullscreenDecode | Self::RawPages) {
            // Prefetch depends on the frame's freshly computed keep range. The frame calls
            // this variant's service at that exact point instead of the early open-owner poll.
            return false;
        }
        matches!(
            self,
            Self::FolderObservation
                | Self::BookCompositionRead
                | Self::CollectionGrid
                | Self::CollectionNavigation
                | Self::RatingNavigation
        ) || self.is_pending(context)
    }

    pub(in crate::app) fn service(
        self,
        app: &mut App,
        ctx: &egui::Context,
        phase: ContextAsyncServicePhase,
    ) {
        if phase == ContextAsyncServicePhase::Dialog {
            match self {
                Self::EpubConversion if app.epub_convert.is_some() => {
                    app.show_epub_convert_dialog(ctx)
                }
                Self::PdfPassword if app.mounted_context_owns_selected_pdf_password_prompt() => {
                    app.show_pdf_password_dialog_window(ctx);
                }
                _ => {}
            }
            return;
        }
        match self {
            Self::FolderObservation => {
                app.poll_current_folder_watch(ctx);
                app.poll_external_rescan(ctx);
            }
            Self::BookCompositionRead => app.poll_book_composition_read(ctx),
            Self::RetainedSpreadShift => app.poll_retained_shift_readoption(ctx),
            Self::PathClassification => app.poll_open_path_classification(ctx),
            Self::HistoryTransition => app.poll_collection_history_transition(ctx),
            Self::CollectionGrid => app.poll_collection_grid(ctx),
            Self::CollectionNavigation => app.poll_collection_navigation(ctx),
            Self::RatingNavigation => app.poll_rating_view(),
            Self::BookmarkOpen => {
                app.poll_bookmark_media_open(ctx);
                app.poll_bookmark_book_open(ctx);
            }
            Self::FolderNavigation if app.navigation_scope.is_detached_physical() => {
                if let Some(result) = app.poll_folder_nav() {
                    app.apply_folder_nav_result(ctx, result);
                }
            }
            Self::FolderPaneScan if app.navigation_scope.is_detached_physical() => {
                if matches!(
                    app.poll_detached_physical_folder_open(ctx),
                    DetachedPhysicalFolderOpenPoll::Failed
                ) {
                    app.terminate_active_detached_open_before_viewport(
                        "detached_physical_open_failed",
                    );
                }
            }
            Self::PdfEnumeration => app.poll_pdf_enumerate(),
            Self::ZipEnumeration => app.poll_zip_enumerate(),
            Self::EpubConversion | Self::PdfPassword => {}
            Self::FullscreenDecode | Self::RawPages => {
                app.poll_prefetch(ctx, PollPrefetchOrigin::TopLevel)
            }
            Self::FinalAi => app.poll_final_ai(ctx),
            Self::LocalAdjustment => app.poll_local_adjust_render(ctx),
            Self::ComicBake => {
                app.poll_comic_bake(ctx);
            }
            Self::EraseInpaint => app.poll_erase_inpaint(ctx),
            Self::SimilarPreview => {
                let passwords = app.pdf_passwords.clone();
                if let Some(notice) = app.similar_panel.preview.poll_background(ctx, &passwords) {
                    app.show_feedback_warning(notice);
                }
            }
            Self::FolderNavigation | Self::FolderPaneScan => {}
        }
    }

    pub(in crate::app) fn terminate_on_park(self, app: &mut App) {
        match self {
            Self::FolderObservation => {}
            Self::PathClassification => {
                if let Some(candidate) = app.top_level_grid_view.take_open_path_classification() {
                    app.finish_rejected_open_path_classification(&candidate);
                }
            }
            Self::HistoryTransition => app.replace_history_navigation_transition(None),
            Self::CollectionGrid => {
                // `cancel_pending` also converts Ready/Empty/Failed/Deleted into a fresh
                // request. Parking only owns the in-flight snapshot/prepare worker; a Deleted
                // physical child must keep its tombstone when this context is resumed.
                if self.is_pending(ContextRef::mounted(app)) {
                    if let Some(session) = app.top_level_grid_view.collection_session_mut() {
                        session.cancel_pending();
                    }
                }
            }
            Self::CollectionNavigation => app.cancel_collection_navigation_intent(),
            Self::RatingNavigation => {
                if let Some(pending) = app.rating_view_pending.take() {
                    pending.cancel();
                }
            }
            Self::BookmarkOpen => {
                if let Some(request_id) = app
                    .bookmark_open_pending
                    .as_ref()
                    .map(|pending| pending.request_id())
                {
                    app.cancel_bookmark_open_request(request_id, "context_parked");
                }
            }
            Self::FolderNavigation => {
                if let Some(pending) = app.folder_nav_pending.take() {
                    pending.cancel.store(true, Ordering::Relaxed);
                }
                app.clear_pending_folder_nav_steps();
                app.release_fs_nav_lock();
            }
            Self::FolderPaneScan => {
                if let Some(mut pending) = app.folder_pane_open_pending.take() {
                    let restore = pending.epub_restore.take();
                    pending.cancel_with_diagnostic("context_parked");
                    app.finish_pane_open_restore(restore, PaneOpenRestoreExit::Abandoned);
                }
            }
            Self::PdfEnumeration => {
                if let Some(mut pending) = app.pdf_enumerate_pending.take() {
                    pending.2.cancel.store(true, Ordering::Relaxed);
                    if let Some(restore) = pending.4.take() {
                        app.restore_epub_open(restore);
                    }
                }
            }
            Self::ZipEnumeration => {
                if let Some(pending) = app.zip_enumerate_pending.take() {
                    pending.cancel.store(true, Ordering::Relaxed);
                }
            }
            Self::EpubConversion => {
                app.finish_epub_convert(crate::ui_dialogs::epub_convert::EpubConvertExit::Parked);
            }
            Self::PdfPassword => {
                let _ = app.finish_pdf_password_request_in_mounted_context(PdfPasswordExit::Parked);
            }
            Self::RawPages => app.raw_pages.park(),
            Self::FullscreenDecode => {
                for (_, pending) in app.fs_pending.drain() {
                    pending.cancel();
                }
            }
            Self::FinalAi => {
                for pending in app.final_ai_pending.values() {
                    pending.cancel.store(true, Ordering::Relaxed);
                }
                app.final_ai_pending.clear();
            }
            Self::LocalAdjustment => {
                for (_, pending) in app.local_adjust_pending.drain() {
                    pending.cancel.store(true, Ordering::Relaxed);
                }
            }
            Self::ComicBake => {
                for (_, pending) in app.comic_bake_pending.drain() {
                    pending.cancel.store(true, Ordering::Relaxed);
                }
            }
            Self::EraseInpaint => {
                for (_, pending) in app.erase_inpaint_pending.drain() {
                    pending.cancel.store(true, Ordering::Relaxed);
                }
            }
            Self::SimilarPreview => app.similar_panel.preview.terminate_on_park(),
            Self::BookCompositionRead => {
                app.retire_book_presentation_permission();
                if matches!(
                    app.book_composition,
                    super::book_composition_runtime::ViewerBookComposition::Preparing { .. }
                        | super::book_composition_runtime::ViewerBookComposition::Projecting { .. }
                ) {
                    app.book_composition = Default::default();
                }
            }
            Self::RetainedSpreadShift => app.spread_shift.cancel_retained_readoption(),
        }
    }
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

    pub(in crate::app) fn presentation(self) -> ViewerPresentation {
        match self.source {
            ContextRefSource::Mounted(app) => app.viewer_presentation,
            ContextRefSource::AtRest(bundle) => bundle.viewer_session.presentation,
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

    /// Normal cache ownership is inspected without mounting or mutating a parked context.
    pub(in crate::app) fn texture_caches(self) -> super::vram_accounting::ViewerTextureCaches<'a> {
        match self.source {
            ContextRefSource::Mounted(app) => app.viewer_texture_caches(),
            ContextRefSource::AtRest(bundle) => super::vram_accounting::ViewerTextureCaches {
                fs_lanczos_cache: &bundle.fs_lanczos_cache,
                similar_panel: &bundle.similar_panel,
                fs_cache: &bundle.fs_cache,
                final_composite_cache: &bundle.final_composite_cache,
                adjustment_cache: &bundle.adjustment_cache,
                erase_result_cache: &bundle.erase_result_cache,
                erase_preview_cache: &bundle.erase_preview_cache,
                analysis_overlay_texture: bundle
                    .analysis_overlay_cache
                    .as_ref()
                    .map(|entry| &entry.0),
                analysis_sv_texture: bundle.analysis_sv_cache.as_ref().map(|entry| &entry.3),
                snapshot: bundle.snapshot.as_ref(),
                composition_editor: &bundle.composition_editor,
                local_adjust_cache: &bundle.local_adjust_cache,
                conceal_cache: &bundle.conceal_cache,
                comic_cache: &bundle.comic_cache,
                edit_result_cache: &bundle.edit_result_cache,
                continuous_page_transitions: &bundle.continuous_page_transitions,
                passthrough_rendition_cache: &bundle.passthrough_rendition_cache,
                fs_holdover_tex: &bundle.fs_holdover_tex,
                thumbnails: &bundle.thumbnails,
                thumb_adjust_tex: &bundle.thumb_adjust_tex,
            },
        }
    }

    pub(in crate::app) fn fs_cache(self) -> &'a ItemsGenerationMap<FsCacheEntry> {
        match self.source {
            ContextRefSource::Mounted(app) => &app.fs_cache,
            ContextRefSource::AtRest(bundle) => &bundle.fs_cache,
        }
    }

    pub(in crate::app) fn raw_pages(self) -> &'a RawPageStore {
        match self.source {
            ContextRefSource::Mounted(app) => &app.raw_pages,
            ContextRefSource::AtRest(bundle) => &bundle.raw_pages,
        }
    }

    pub(in crate::app) fn viewer_session_last_sync_stamp(self) -> Option<&'a ViewerSyncStamp> {
        match self.source {
            ContextRefSource::Mounted(app) => app.last_viewer_sync_stamp.as_ref(),
            ContextRefSource::AtRest(bundle) => bundle.viewer_session.last_sync_stamp.as_ref(),
        }
    }

    pub(in crate::app) fn pdf_password_request(self) -> Option<&'a PdfPasswordRequest> {
        match self.source {
            ContextRefSource::Mounted(app) => app.pdf_password_request.as_ref(),
            ContextRefSource::AtRest(bundle) => bundle.pdf_password_request.as_ref(),
        }
    }

    pub(in crate::app) fn epub_convert(
        self,
    ) -> Option<&'a crate::ui_dialogs::epub_convert::EpubConvertState> {
        match self.source {
            ContextRefSource::Mounted(app) => app.epub_convert.as_deref(),
            ContextRefSource::AtRest(bundle) => bundle.epub_convert.as_deref(),
        }
    }

    pub(in crate::app) fn pending_context_async_owners(self) -> Vec<ContextAsyncOwner> {
        ContextAsyncOwner::ALL
            .into_iter()
            .filter(|owner| owner.is_pending(self))
            .collect()
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

    pub(in crate::app) fn book_bookmarks_request(self) -> Option<&'a CurrentBookBookmarksRequest> {
        match self.source {
            ContextRefSource::Mounted(app) => app.current_book_bookmarks_request.as_ref(),
            ContextRefSource::AtRest(bundle) => bundle.current_book_bookmarks_request.as_ref(),
        }
    }

    pub(in crate::app) fn book_bookmarks_key(self) -> Option<&'a BookBookmarkProjectionKey> {
        match self.source {
            ContextRefSource::Mounted(app) => app.current_book_bookmarks_key.as_ref(),
            ContextRefSource::AtRest(bundle) => bundle.current_book_bookmarks_key.as_ref(),
        }
    }

    pub(in crate::app) fn book_bookmarks(self) -> &'a [crate::book_bookmarks::BookBookmark] {
        match self.source {
            ContextRefSource::Mounted(app) => &app.current_book_bookmarks,
            ContextRefSource::AtRest(bundle) => &bundle.current_book_bookmarks,
        }
    }

    pub(in crate::app) fn book_bookmark_title_editing(self) -> bool {
        match self.source {
            ContextRefSource::Mounted(app) => app.book_bookmark_title_edit.is_some(),
            ContextRefSource::AtRest(bundle) => bundle.book_bookmark_title_edit.is_some(),
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

    pub(in crate::app) fn pending_fs_page_turn_ready(
        self,
    ) -> Option<&'a crate::ui_fullscreen::PendingFsPageTurnReady> {
        match self.source {
            ContextRefSource::Mounted(app) => app.pending_fs_page_turn_ready.as_ref(),
            ContextRefSource::AtRest(bundle) => bundle.pending_fs_page_turn_ready.as_ref(),
        }
    }

    pub(in crate::app) fn fs_lanczos_cache(self) -> &'a crate::gpu_lanczos::GpuLanczosCache {
        match self.source {
            ContextRefSource::Mounted(app) => &app.fs_lanczos_cache,
            ContextRefSource::AtRest(bundle) => &bundle.fs_lanczos_cache,
        }
    }

    pub(in crate::app) fn similar_panel(self) -> &'a crate::ui_metadata_panel::SimilarPanelState {
        match self.source {
            ContextRefSource::Mounted(app) => &app.similar_panel,
            ContextRefSource::AtRest(bundle) => &bundle.similar_panel,
        }
    }

    #[cfg(feature = "test-script")]
    pub(in crate::app) fn video_seek_strip_test_script_snapshot(
        self,
    ) -> crate::test_script::TestScriptSeekStripSnapshot {
        match self.source {
            ContextRefSource::Mounted(app) => app.video_seek_strip_runtime.test_script_snapshot(),
            ContextRefSource::AtRest(bundle) => {
                bundle.video_seek_strip_runtime.test_script_snapshot()
            }
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
            ContextRefSource::Mounted(app) => {
                let player = match app.fs_cache.get(&idx)? {
                    crate::app::FsCacheEntry::Video { player, .. } => player,
                    _ => return None,
                };
                let stream = player.applied_audio_stream_index()?;
                let key = crate::app::normalize::NormalizeTargetKey::new(
                    idx,
                    player.path().clone(),
                    stream,
                );
                app.normalize_ui_states.get(&key).copied()
            }
            ContextRefSource::AtRest(bundle) => {
                let player = match bundle.fs_cache.get(&idx)? {
                    crate::app::FsCacheEntry::Video { player, .. } => player,
                    _ => return None,
                };
                let stream = player.applied_audio_stream_index()?;
                let key = crate::app::normalize::NormalizeTargetKey::new(
                    idx,
                    player.path().clone(),
                    stream,
                );
                bundle.normalize_ui_states.get(&key).copied()
            }
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
    #[cfg(test)]
    fn send_thumbnail_for_test(&mut self, msg: ThumbMsg) -> Result<(), mpsc::SendError<ThumbMsg>> {
        let msg = self.requested.correlate_fixture(msg);
        self.tx.send(msg)
    }

    /// この viewer context が所有する非同期 work を、context の terminal retire と同じ
    /// ownership 境界で停止する。
    ///
    /// thumbnail pool だけは condvar 待ちで残留し得るため notify まで必要。その他は有限の
    /// one-shot worker だが、owner の消滅後に CPU / GPU / AI 処理を続ける理由がないので、
    /// 各 worker が既に監視している cancel token を立てる。
    fn cancel_all_context_work(&mut self) {
        if let Some(pending) = self.external_rescan_pending.take() {
            pending.cancel_and_retire();
        }
        self.current_folder_watch = None;
        self.folder_refresh_pending = None;
        self.pdf_render_epoch.retire();
        self.pdf_visible_promotion = super::PdfVisiblePromotionState::Idle;
        self.current_book_bookmarks_request = None;
        crate::thumb_loader::cancel_raw_thumb_tickets(&self.raw_thumb_develop);
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
        if let Some(pending) = self.metadata_pending.as_ref() {
            pending.cancel();
        }
        if let Some(pending) = self.converted_archive_cache_paths_pending.as_ref() {
            pending.cancel.store(true, Ordering::Relaxed);
        }
        if let Some(pending) = self.folder_nav_pending.as_ref() {
            pending.cancel.store(true, Ordering::Relaxed);
        }
        if let Some(pending) = self.folder_pane_open_pending.as_mut() {
            pending.cancel_with_diagnostic("context_retired");
        }
        if let Some(holdover) = self.fs_holdover_tex.as_mut() {
            holdover.finish_navigation_diagnostic("context_retired");
        }
        for pending in self.final_effect_pending.values() {
            pending.cancel.store(true, Ordering::Relaxed);
        }

        self.raw_pages.cancel_development_all();
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
            self.requested.clear();
            crate::thumb_loader::cancel_raw_thumb_tickets(&self.raw_thumb_develop);
            self.raw_thumb_develop
                .lock()
                .unwrap()
                .set_items_generation(items_generation);
            self.still_seek_thumbnail_pages.clear();
            if let Ok(mut shared) = self.still_seek_thumbnail_pages_shared.write() {
                shared.clear();
            }
            // This bundle can be rebuilt for another physical source without first becoming the
            // mounted App. Its last painted layout belongs to the old items identity.
            self.fullscreen_page_layout.clear();
        }
        self.items_generation = items_generation;
        self.fs_cache.set_items_generation(items_generation);
        self.fs_pending.set_items_generation(items_generation);
        self.raw_pages.set_items_generation(items_generation);
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
            display_epub_source: None,
            fullscreen_epub_source: None,
            favorite_view_context: FavoriteViewContextState::default(),
            navigation_scope: ViewerNavigationScope::Main,
            archive_source_override: None,
            zip_nav: None,
            stack_mode_requested: false,
            stack_view: None,
            stack_showing_flat: false,
            stack_return_state: None,
            stack_script_pending: None,
            stack_request_sequence: 0,
            stack_active_rule: None,
            stack_script_error: None,
            stack_selection_target: None,
            items: Vec::new(),
            page_identity_index: Default::default(),
            projection_recipe: Default::default(),
            current_book_bookmarks: Vec::new(),
            current_book_bookmarks_key: None,
            current_book_bookmarks_request: None,
            book_bookmark_title_edit: None,
            bookmark_view_registered_total: 0,
            items_generation: 0,
            pdf_render_epoch: crate::pdf_loader::PdfRenderEpochOwner::fresh(),
            pdf_visible_promotion: super::PdfVisiblePromotionState::default(),
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
            requested: ThumbnailRequests::default(),
            raw_thumb_develop: Arc::new(Mutex::new(ItemsGenerationMap::new("raw_thumb_develop"))),
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
            rotation_cache: crate::rotation_cache::RotationCache::default(),
            page_dims_cache: crate::page_dims::PageDimsCache::default(),
            spread_display_units_cache: crate::ui_fullscreen::SpreadDisplayUnitsCache::default(),
            rating_cache: std::collections::HashMap::new(),
            rating_filter_suppressed_at: None,
            rating_session_write_seen_generation: 0,
            metadata_import_refresh_index: None,
            current_folder_rating_cache: None,
            current_folder_last_mtime: None,
            current_folder_signature: None,
            folder_observation: None,
            current_folder_watch: None,
            current_folder_watch_failed: None,
            external_rescan_pending: None,
            folder_refresh_pending: None,
            folder_pin_map: std::collections::HashMap::new(),
            converted_archive_cache_paths: std::collections::HashMap::new(),
            converted_archive_pin_root_states: std::collections::HashMap::new(),
            converted_archive_cache_paths_pending: None,
            current_color_cache_map: None,
            current_color_catalog: None,
            vst3_deferred_media_open: None,
            fullscreen_idx: None,
            fullscreen_page_slice: crate::page_split::PageSlice::Full,
            fs_secondary_press: crate::ui_fullscreen::FullscreenSecondaryPress::Idle,
            viewer_session: ViewerSession::default(),
            native_video_in_window_active: false,
            video_audio_mode: None,
            video_audio_vst: None,
            video_audio_mode_runtime: None,
            video_seek_strip_runtime: crate::app::native_video::VideoSeekStripRuntime::Closed,
            video_seek_strip_wave_holdover: None,
            video_seek_strip_next_session_id: 1,
            panorama_state: None,
            video_zoom_state: None,
            native_video_mouse_seek_holds:
                crate::app::native_video::NativeVideoMouseSeekHolds::default(),
            panorama_intent: crate::panorama::PanoramaSessionIntent::default(),
            fs_info_panel: crate::ui_helpers::FullscreenInfoPanelState::default(),
            similar_panel: crate::ui_metadata_panel::SimilarPanelState::default(),
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
            fullscreen_navigator_interaction:
                crate::ui_fullscreen::FullscreenNavigatorInteractionOwner::default(),
            fullscreen_page_layout: crate::displayed_image_transform::FullscreenPageLayout::default(
            ),
            fs_margin_bbox_cache: std::collections::HashMap::new(),
            input_generation: std::collections::HashMap::new(),
            raw_pages: Box::new(RawPageStore::new()),
            fs_pending: ItemsGenerationMap::with_discard("fs_pending", cancel_fs_pending_value),
            fullscreen_pdf_promotion: FullscreenPdfPromotionState::default(),
            fs_pdf_display_target: None,
            fs_early_dims: ItemsGenerationMap::new("fs_early_dims"),
            fs_upload_backlog: FsUploadBacklog::new("fs_upload_backlog", fs_upload_backlog_idx),
            top_level_grid_view: top_level_grid_view::TopLevelGridView::default(),
            snapshot: None,
            global_search_subfolder_restore: None,
            global_search: crate::global_search_ui::GlobalSearchState::default(),
            favsearch_subfolder_restore: None,
            items_are_global_search_view: false,
            items_are_tag_view: false,
            items_are_reading_history_view: false,
            items_are_bookmark_view: false,
            items_are_rating_view: false,
            rating_view_stars: 0,
            rating_view_sort: crate::rating_view::RatingViewSort::default(),
            rating_view_rows: Vec::new(),
            rating_view_rows_stars: None,
            rating_view_pending: None,
            rating_view_request_sequence: 0,
            rating_view_accepted_write_generation: 0,
            rating_view_skipped: 0,
            rating_view_saved_folder: None,
            rating_view_subfolder_restore: None,
            rating_view_nav_stack: Vec::new(),
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
            fs_pdf_rerender_demand: PdfRerenderDemand::default(),
            fs_continuous_fit_reference:
                crate::ui_fullscreen::ContinuousSpecifiedFitReference::Legacy,
            fs_pan_drag_start: None,
            fs_vertical_scroll: 0.0,
            fs_seek_drag_active: false,
            fs_seek_gesture: crate::ui_fullscreen::StillSeekGesture::Idle,
            fs_seek_overlay_visible: false,
            fs_vertical_cache_keep_set: std::collections::HashSet::new(),
            pending_fs_page_turn_ready: None,
            page_turn_presentation: Default::default(),
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
            final_cover_spread_preference: crate::settings::FinalCoverSpreadPreference::default(),
            singleton_spread_endpoint_preferences:
                crate::settings::SingletonSpreadEndpointPreferences::default(),
            page_alone_preferences: crate::settings::PageAlonePreferences::default(),
            spread_shift: Default::default(),
            book_composition: Default::default(),
            composition_editor: Default::default(),
            book_composition_permission: Default::default(),
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
            epub_convert: None,
            zip_enumerate_pending: None,
            fs_nav_after_pdf_enumerate: None,
            pending_auto_fs_open: false,
            pending_return_to_parent: Default::default(),
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
            page_edit_revision: 0,
            local_page_edit_document: LocalPageEditDocument::default(),
            local_adjust_spread_ctx: None,
            adjustment_drag_session: None,
            adjustment_standard_drag_session: None,
            adjustment_dragging: false,
            thumb_adjust_drag_color_dirty: false,
            adjustment_mode: crate::ui_helpers::MetadataPanelOpenState::Closed,
            adjust_spread_target: AdjustSpreadTarget::Left,
            adjust_scope_selection: AdjustScopeSelection::Standard,
            adjust_scope_selection_idx: None,
            local_adjust_mode: false,
            local_adjust_show_mask: true,
            local_adjust_selected_shape: None,
            local_adjust_shape_drag: None,
            local_adjust_canvas_drag: None,
            local_adjust_mask_brush_stroke: None,
            local_adjust_shape_drag_before_layers: None,
            local_adjust_canvas_drag_before_layers: None,
            local_adjust_mask_brush_before_layers: None,
            local_adjust_shape_key_edit: None,
            local_adjust_mask_lasso_points: Vec::new(),
            local_adjust_mask_shape_drag_start: None,
            local_adjust_mask_shape_drag_end: None,
            local_adjust_mask_edit_target: LocalAdjustMaskEditTarget::None,
            local_adjust_brush_deferred_render: None,
            local_adjust_rgb_pick_active: None,
            local_adjust_repair_point_pick_active: None,
            local_adjust_selective_color_pick_active: false,
            local_adjust_add_layer_dialog_open: false,
            local_adjust_change_mask_dialog_open: false,
            local_adjust_effect_picker_dialog_open: false,
            local_adjust_lut_pending: None,
            local_adjust_segmentation_pending: None,
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
            page_edit_snapshot: None,
            page_edit_reconcile_pending: None,
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
        if let Some(holdover) = self.fs_holdover_tex.as_mut() {
            holdover.finish_navigation_diagnostic("context_parked");
        }
        for owner in ContextAsyncOwner::ALL {
            owner.terminate_on_park(self);
        }
        self.slideshow_playing = false;
        self.continuous_reading_scroll_transition = None;
        self.slideshow_scroll_range_cache = None;
        self.fs_seek_drag_active = false;
        self.fs_seek_gesture.interrupt();
        self.fs_seek_overlay_visible = false;
        self.pending_auto_fs_open = false;
        self.pending_return_to_parent.cancel_live_request();
        self.fs_nav_after_pdf_enumerate = None;
        self.fs_nav_locked_gen = None;
        self.fs_holdover_tex = None;
        self.fs_nav_dropped_block_signature = None;
        self.fs_nav_dropped_block_count = 0;
        self.continuous_page_transitions.clear();
        self.pending_folder_nav_steps = 0;
        self.pending_folder_nav_mode = FolderNavMode::Grid;
        crate::thumb_loader::cancel_raw_thumb_tickets(&self.raw_thumb_develop);
        self.requested.discard_images();
        debug_assert!(ContextAsyncOwner::ALL.into_iter().all(|owner| {
            owner == ContextAsyncOwner::FolderObservation
                || !owner.is_pending(ContextRef::mounted(self))
        }));
    }

    #[cfg(windows)]
    pub(in crate::app) fn activate_mounted_as_independent_detached(&mut self, window_id: u64) {
        if self.projected_viewer_context_id() != self.viewer_context_main() {
            // A linked viewer still projects the main list. Only a context that
            // already owns its independent list loses passive observation here.
            self.retire_passive_folder_observation();
        }
        self.viewer_presentation = ViewerPresentation::DetachedWindow;
        self.ensure_mounted_detached_session_binding(window_id);
        self.detached_viewer_independent_active = true;
        self.detached_viewer_open_next_still_detached_once = false;
        self.last_viewer_sync_stamp = None;
        self.fs_open_intent_from_grid = false;
        self.pending_auto_fs_open = false;
        self.pending_return_to_parent.cancel_live_request();
        self.pdf_prefetch_grace_until = None;
    }

    #[cfg(windows)]
    pub(in crate::app) fn become_mounted_independent_detached_viewer(
        &mut self,
        window_id: u64,
        idx: usize,
    ) {
        if self.viewer_mutation_modal_active() {
            return;
        }
        match self.try_own_fullscreen_epub_at(idx) {
            super::OpenAdmission::Accepted => {}
            super::OpenAdmission::NotApplicable => return,
            super::OpenAdmission::Refused(reason) => {
                self.show_open_admission_refusal(reason);
                return;
            }
        }
        self.move_grid_cursor(Some(idx), crate::app::GridCursorIntent::Automatic);
        self.fullscreen_idx = Some(idx);
        // 新しく開くページは分割方向の最初の半分から。ここで残すと前のページの
        // 「右半分を見ていた」が別のページへ引き継がれる。
        self.fullscreen_page_slice = crate::page_split::PageSlice::Full;
        self.native_video_in_window_active = false;
        self.activate_mounted_as_independent_detached(window_id);
    }

    #[cfg(windows)]
    pub(in crate::app) fn swap_viewer_context_bundle(&mut self, bundle: &mut ViewerContextBundle) {
        // The table may have a pending transition throughout this payload exchange.
        // Publish ratings only at the stable plan-before / finish-after boundaries below.
        // The existing favorite overlay transition is settings projection only; it must
        // finish before the incoming owner publishes its membership or navigates.
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
            display_epub_source,
            fullscreen_epub_source,
            favorite_view_context,
            navigation_scope,
            archive_source_override,
            zip_nav,
            stack_mode_requested,
            stack_view,
            stack_showing_flat,
            stack_return_state,
            stack_script_pending,
            stack_request_sequence,
            stack_active_rule,
            stack_script_error,
            stack_selection_target,
            items,
            page_identity_index,
            projection_recipe,
            current_book_bookmarks,
            current_book_bookmarks_key,
            current_book_bookmarks_request,
            book_bookmark_title_edit,
            bookmark_view_registered_total,
            items_generation,
            pdf_render_epoch,
            pdf_visible_promotion,
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
            raw_thumb_develop,
            idle_upgrade_cache_bypass_ineligible,
            keep_range,
            keep_set,
            still_seek_thumbnail_pages,
            still_seek_thumbnail_pages_shared,
            thumbnail_eviction_generation,
            details_thumb_suppression_applied,
            details_hover_thumb_idx,
            details_hover_thumb_viewport_open,
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
            folder_observation,
            current_folder_watch,
            current_folder_watch_failed,
            external_rescan_pending,
            folder_refresh_pending,
            folder_pin_map,
            converted_archive_cache_paths,
            converted_archive_pin_root_states,
            converted_archive_cache_paths_pending,
            current_color_cache_map,
            current_color_catalog,
            vst3_deferred_media_open,
            fullscreen_idx,
            fullscreen_page_slice,
            fs_secondary_press,
            viewer_session,
            native_video_in_window_active,
            video_audio_mode,
            video_audio_vst,
            video_audio_mode_runtime,
            video_seek_strip_runtime,
            video_seek_strip_wave_holdover,
            video_seek_strip_next_session_id,
            panorama_state,
            video_zoom_state,
            native_video_mouse_seek_holds,
            panorama_intent,
            fs_info_panel,
            similar_panel,
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
            fullscreen_navigator_interaction,
            fullscreen_page_layout,
            fs_margin_bbox_cache,
            input_generation,
            raw_pages,
            fs_pending,
            fullscreen_pdf_promotion,
            fs_pdf_display_target,
            fs_early_dims,
            fs_upload_backlog,
            top_level_grid_view,
            snapshot,
            global_search_subfolder_restore,
            global_search,
            favsearch_subfolder_restore,
            items_are_global_search_view,
            items_are_tag_view,
            items_are_reading_history_view,
            items_are_bookmark_view,
            items_are_rating_view,
            rating_view_stars,
            rating_view_sort,
            rating_view_rows,
            rating_view_rows_stars,
            rating_view_pending,
            rating_view_request_sequence,
            rating_view_accepted_write_generation,
            rating_view_skipped,
            rating_view_saved_folder,
            rating_view_subfolder_restore,
            rating_view_nav_stack,
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
            fs_pdf_rerender_demand,
            fs_continuous_fit_reference,
            fs_pan_drag_start,
            fs_vertical_scroll,
            fs_seek_drag_active,
            fs_seek_gesture,
            fs_seek_overlay_visible,
            fs_vertical_cache_keep_set,
            pending_fs_page_turn_ready,
            page_turn_presentation,
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
            final_cover_spread_preference,
            singleton_spread_endpoint_preferences,
            page_alone_preferences,
            spread_shift,
            book_composition,
            composition_editor,
            book_composition_permission,
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
            epub_convert,
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
            page_edit_revision,
            local_page_edit_document,
            local_adjust_spread_ctx,
            adjustment_drag_session,
            adjustment_standard_drag_session,
            adjustment_dragging,
            thumb_adjust_drag_color_dirty,
            adjustment_mode,
            adjust_spread_target,
            adjust_scope_selection,
            adjust_scope_selection_idx,
            local_adjust_mode,
            local_adjust_show_mask,
            local_adjust_selected_shape,
            local_adjust_shape_drag,
            local_adjust_canvas_drag,
            local_adjust_mask_brush_stroke,
            local_adjust_shape_drag_before_layers,
            local_adjust_canvas_drag_before_layers,
            local_adjust_mask_brush_before_layers,
            local_adjust_shape_key_edit,
            local_adjust_mask_lasso_points,
            local_adjust_mask_shape_drag_start,
            local_adjust_mask_shape_drag_end,
            local_adjust_mask_edit_target,
            local_adjust_brush_deferred_render,
            local_adjust_rgb_pick_active,
            local_adjust_repair_point_pick_active,
            local_adjust_selective_color_pick_active,
            local_adjust_add_layer_dialog_open,
            local_adjust_change_mask_dialog_open,
            local_adjust_effect_picker_dialog_open,
            local_adjust_lut_pending,
            local_adjust_segmentation_pending,
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
            page_edit_snapshot,
            page_edit_reconcile_pending,
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
        swap_field!(display_epub_source);
        swap_field!(fullscreen_epub_source);
        swap_field!(favorite_view_context);
        swap_field!(navigation_scope);
        swap_field!(archive_source_override);
        swap_field!(zip_nav);
        swap_field!(stack_mode_requested);
        swap_field!(stack_view);
        swap_field!(stack_showing_flat);
        swap_field!(stack_return_state);
        swap_field!(stack_script_pending);
        swap_field!(stack_request_sequence);
        swap_field!(stack_active_rule);
        swap_field!(stack_script_error);
        swap_field!(stack_selection_target);
        swap_field!(items);
        swap_field!(page_identity_index);
        swap_field!(projection_recipe);
        swap_field!(current_book_bookmarks);
        swap_field!(current_book_bookmarks_key);
        swap_field!(current_book_bookmarks_request);
        swap_field!(book_bookmark_title_edit);
        swap_field!(bookmark_view_registered_total);
        swap_field!(items_generation);
        swap_field!(pdf_render_epoch);
        swap_field!(pdf_visible_promotion);
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
        swap_field!(raw_thumb_develop);
        swap_field!(idle_upgrade_cache_bypass_ineligible);
        swap_field!(keep_range);
        swap_field!(keep_set);
        swap_field!(still_seek_thumbnail_pages);
        swap_field!(still_seek_thumbnail_pages_shared);
        swap_field!(thumbnail_eviction_generation);
        swap_field!(details_thumb_suppression_applied);
        swap_field!(details_hover_thumb_idx);
        swap_field!(details_hover_thumb_viewport_open);
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
        // per-context ロード複合体 (review-v2.3.0 P2-8/P2-9)。channel/token/キューが
        // コンテキストと一緒に移動するので、thumbnail request owner の bookkeeping は
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
        swap_field!(folder_observation);
        swap_field!(current_folder_watch);
        swap_field!(current_folder_watch_failed);
        swap_field!(external_rescan_pending);
        swap_field!(folder_refresh_pending);
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
        swap_field!(fs_secondary_press);
        viewer_session.swap_with_mounted(
            &mut self.viewer_presentation,
            &mut self.last_viewer_sync_stamp,
            &mut self.detached_viewer_independent_active,
            &mut self.detached_viewer_open_next_still_detached_once,
        );
        swap_field!(native_video_in_window_active);
        swap_field!(video_audio_mode);
        swap_field!(video_audio_vst);
        swap_field!(video_audio_mode_runtime);
        swap_field!(video_seek_strip_runtime);
        swap_field!(video_seek_strip_wave_holdover);
        swap_field!(video_seek_strip_next_session_id);
        swap_field!(panorama_state);
        swap_field!(video_zoom_state);
        swap_field!(native_video_mouse_seek_holds);
        swap_field!(panorama_intent);
        swap_field!(fs_info_panel);
        swap_field!(similar_panel);
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
        swap_field!(fullscreen_navigator_interaction);
        swap_field!(fullscreen_page_layout);
        swap_field!(fs_margin_bbox_cache);
        swap_field!(input_generation);
        swap_field!(fs_pending);
        swap_field!(raw_pages);
        swap_field!(fullscreen_pdf_promotion);
        swap_field!(fs_pdf_display_target);
        swap_field!(fs_early_dims);
        swap_field!(fs_upload_backlog);
        swap_field!(top_level_grid_view);
        swap_field!(snapshot);
        swap_field!(rating_filter_suppressed_at);
        swap_field!(global_search_subfolder_restore);
        swap_field!(global_search);
        swap_field!(favsearch_subfolder_restore);
        swap_field!(items_are_global_search_view);
        swap_field!(items_are_tag_view);
        swap_field!(items_are_reading_history_view);
        swap_field!(items_are_bookmark_view);
        swap_field!(items_are_rating_view);
        swap_field!(rating_view_stars);
        swap_field!(rating_view_sort);
        swap_field!(rating_view_rows);
        swap_field!(rating_view_rows_stars);
        swap_field!(rating_view_pending);
        swap_field!(rating_view_request_sequence);
        swap_field!(rating_view_accepted_write_generation);
        swap_field!(rating_view_skipped);
        swap_field!(rating_view_saved_folder);
        swap_field!(rating_view_subfolder_restore);
        swap_field!(rating_view_nav_stack);
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
        swap_field!(fs_pdf_rerender_demand);
        swap_field!(fs_continuous_fit_reference);
        swap_field!(fs_pan_drag_start);
        swap_field!(fs_vertical_scroll);
        swap_field!(fs_seek_drag_active);
        swap_field!(fs_seek_gesture);
        swap_field!(fs_seek_overlay_visible);
        swap_field!(fs_vertical_cache_keep_set);
        swap_field!(pending_fs_page_turn_ready);
        swap_field!(page_turn_presentation);
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
        swap_field!(final_cover_spread_preference);
        swap_field!(singleton_spread_endpoint_preferences);
        swap_field!(page_alone_preferences);
        swap_field!(spread_shift);
        swap_field!(book_composition);
        swap_field!(composition_editor);
        swap_field!(book_composition_permission);
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
        swap_field!(epub_convert);
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
        swap_field!(page_edit_revision);
        swap_field!(local_page_edit_document);
        swap_field!(local_adjust_spread_ctx);
        swap_field!(adjustment_drag_session);
        swap_field!(adjustment_standard_drag_session);
        swap_field!(adjustment_dragging);
        swap_field!(thumb_adjust_drag_color_dirty);
        swap_field!(adjustment_mode);
        swap_field!(adjust_spread_target);
        swap_field!(adjust_scope_selection);
        swap_field!(adjust_scope_selection_idx);
        swap_field!(local_adjust_mode);
        swap_field!(local_adjust_show_mask);
        swap_field!(local_adjust_selected_shape);
        swap_field!(local_adjust_shape_drag);
        swap_field!(local_adjust_canvas_drag);
        swap_field!(local_adjust_mask_brush_stroke);
        swap_field!(local_adjust_shape_drag_before_layers);
        swap_field!(local_adjust_canvas_drag_before_layers);
        swap_field!(local_adjust_mask_brush_before_layers);
        swap_field!(local_adjust_shape_key_edit);
        swap_field!(local_adjust_mask_lasso_points);
        swap_field!(local_adjust_mask_shape_drag_start);
        swap_field!(local_adjust_mask_shape_drag_end);
        swap_field!(local_adjust_mask_edit_target);
        swap_field!(local_adjust_brush_deferred_render);
        swap_field!(local_adjust_rgb_pick_active);
        swap_field!(local_adjust_repair_point_pick_active);
        swap_field!(local_adjust_selective_color_pick_active);
        swap_field!(local_adjust_add_layer_dialog_open);
        swap_field!(local_adjust_change_mask_dialog_open);
        swap_field!(local_adjust_effect_picker_dialog_open);
        swap_field!(local_adjust_lut_pending);
        swap_field!(local_adjust_segmentation_pending);
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
        swap_field!(page_edit_snapshot);
        swap_field!(page_edit_reconcile_pending);
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

        // 旧実装はここで thumbnail request owner を無条件 clear していた (worker queue が
        // App-global で、swap 後の bookkeeping を信用できなかったため)。detached 窓が 1 枚でも
        // あると mount/unmount + parked poll の swap が毎フレーム走るので、main の bookkeeping が
        // 毎フレーム消え、Pending サムネがフレームごとに重複エンキュー/重複デコードされる
        // churn になっていた (review-v2.3.0 P2-8)。channel/token/キューを bundle 化した現在は
        // bookkeeping がコンテキストと一緒に移動するため clear 不要。
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
        // This boundary executes only after the table admitted the successful fork.
        // The transient reader and its return lifetime move as one owner; the main
        // projection receives the unchanged, already prepared physical basis.
        let direct_owner = self.direct_media_reader_active().then(|| {
            (
                self.snapshot.take().expect("direct media owner exists"),
                std::mem::take(&mut self.top_level_grid_view),
            )
        });
        crate::thumb_loader::cancel_raw_thumb_tickets(&self.raw_thumb_develop);
        // Forking changes the exact viewer owner even when the new payload retains the same
        // fullscreen index. Never carry a physical right-button sequence across that boundary.
        self.fs_secondary_press.cancel();
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
            display_epub_source,
            fullscreen_epub_source,
            favorite_view_context,
            navigation_scope,
            archive_source_override,
            zip_nav,
            stack_mode_requested,
            stack_view,
            stack_showing_flat,
            stack_return_state,
            stack_script_pending,
            stack_request_sequence,
            stack_active_rule,
            stack_script_error,
            stack_selection_target,
            items,
            page_identity_index,
            projection_recipe,
            current_book_bookmarks,
            current_book_bookmarks_key,
            current_book_bookmarks_request,
            book_bookmark_title_edit,
            bookmark_view_registered_total,
            items_generation,
            pdf_render_epoch,
            pdf_visible_promotion,
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
            raw_thumb_develop,
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
            folder_observation,
            current_folder_watch,
            current_folder_watch_failed,
            external_rescan_pending,
            folder_refresh_pending,
            folder_pin_map,
            converted_archive_cache_paths,
            converted_archive_pin_root_states,
            converted_archive_cache_paths_pending,
            current_color_cache_map,
            current_color_catalog,
            vst3_deferred_media_open,
            fullscreen_idx,
            fullscreen_page_slice,
            fs_secondary_press,
            viewer_session,
            native_video_in_window_active,
            video_audio_mode,
            video_audio_vst,
            video_audio_mode_runtime,
            video_seek_strip_runtime,
            video_seek_strip_wave_holdover,
            video_seek_strip_next_session_id,
            panorama_state,
            video_zoom_state,
            native_video_mouse_seek_holds,
            panorama_intent,
            fs_info_panel,
            similar_panel,
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
            fullscreen_navigator_interaction,
            fullscreen_page_layout,
            fs_margin_bbox_cache,
            input_generation,
            raw_pages,
            fs_pending,
            fullscreen_pdf_promotion,
            fs_pdf_display_target,
            fs_early_dims,
            fs_upload_backlog,
            top_level_grid_view,
            snapshot,
            global_search_subfolder_restore,
            global_search,
            favsearch_subfolder_restore,
            items_are_global_search_view,
            items_are_tag_view,
            items_are_reading_history_view,
            items_are_bookmark_view,
            items_are_rating_view,
            rating_view_stars,
            rating_view_sort,
            rating_view_rows,
            rating_view_rows_stars,
            rating_view_pending,
            rating_view_request_sequence,
            rating_view_accepted_write_generation,
            rating_view_skipped,
            rating_view_saved_folder,
            rating_view_subfolder_restore,
            rating_view_nav_stack,
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
            fs_pdf_rerender_demand,
            fs_continuous_fit_reference,
            fs_pan_drag_start,
            fs_vertical_scroll,
            fs_seek_drag_active,
            fs_seek_gesture,
            fs_seek_overlay_visible,
            fs_vertical_cache_keep_set,
            pending_fs_page_turn_ready,
            page_turn_presentation,
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
            final_cover_spread_preference,
            singleton_spread_endpoint_preferences,
            page_alone_preferences,
            spread_shift,
            book_composition,
            composition_editor,
            book_composition_permission,
            reading_flow,
            reading_direction,
            slideshow_playing,
            slideshow_next_at,
            slideshow_anchor_idx,
            continuous_reading_scroll_transition,
            slideshow_scroll_range_cache,
            pdf_enumerate_pending,
            epub_convert,
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
            page_edit_snapshot,
            page_edit_reconcile_pending,
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
            ..
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
            display_epub_source,
            fullscreen_epub_source,
            favorite_view_context,
            archive_source_override,
            zip_nav,
            stack_mode_requested,
            stack_view,
            stack_showing_flat,
            stack_request_sequence,
            stack_active_rule,
            stack_script_error,
            stack_selection_target,
            items,
            page_identity_index,
            projection_recipe,
            bookmark_view_registered_total,
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
            folder_observation,
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
            current_book_bookmarks,
            current_book_bookmarks_key,
            current_book_bookmarks_request,
            book_bookmark_title_edit,
            stack_return_state,
            vst3_deferred_media_open,
            fullscreen_idx,
            fullscreen_page_slice,
            fs_secondary_press,
            native_video_in_window_active,
            video_audio_mode,
            video_audio_vst,
            video_audio_mode_runtime,
            video_seek_strip_runtime,
            video_seek_strip_wave_holdover,
            video_seek_strip_next_session_id,
            panorama_state,
            video_zoom_state,
            native_video_mouse_seek_holds,
            // 意図は 360 state と同じ側へ動く。渡した viewer が 360 を続けるので、
            // その viewer がページを移ったときに復帰するのも同じ側 (backlog §1.145)。
            panorama_intent,
            fs_info_panel,
            similar_panel,
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
            fullscreen_navigator_interaction,
            fullscreen_page_layout,
            fs_margin_bbox_cache,
            input_generation,
            raw_pages,
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
            fs_pdf_rerender_demand,
            fs_continuous_fit_reference,
            fs_pan_drag_start,
            fs_vertical_scroll,
            fs_seek_drag_active,
            fs_seek_gesture,
            fs_seek_overlay_visible,
            fs_vertical_cache_keep_set,
            pending_fs_page_turn_ready,
            page_turn_presentation,
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

        // ParkedLive/ownership transfer is a semantic terminal for a physical hold.
        // Ordinary registry deposit does not pass this split and preserves the state.
        let _ = native_video_mouse_seek_holds.clear();

        viewer_session.swap_with_mounted(
            &mut self.viewer_presentation,
            &mut self.last_viewer_sync_stamp,
            &mut self.detached_viewer_independent_active,
            &mut self.detached_viewer_open_next_still_detached_once,
        );

        // グリッド worker / 詳細列 / タグ prewarm / 編集・見開き・view-trim / folder-nav は
        // main が原本を保持する。parked メディア窓はこれらを駆動しないので empty のままでよい。
        keep_in_main!(
            current_folder_watch,
            current_folder_watch_failed,
            external_rescan_pending,
            folder_refresh_pending,
            navigation_scope,
            global_search,
            rating_view_stars,
            rating_view_sort,
            rating_view_rows,
            rating_view_rows_stars,
            rating_view_pending,
            rating_view_request_sequence,
            rating_view_accepted_write_generation,
            rating_view_skipped,
            rating_view_saved_folder,
            rating_view_subfolder_restore,
            rating_view_nav_stack,
            // 新しい parked context は複製済み items/cache の独立 owner になる。進行中の
            // receiver だけは複製できないため、元の main context に残す。
            stack_script_pending,
            facet_name_cache_pending,
            pdf_render_epoch,
            pdf_visible_promotion,
            requested,
            raw_thumb_develop,
            metadata_import_refresh_index,
            idle_upgrade_cache_bypass_ineligible,
            details_thumb_suppression_applied,
            details_hover_thumb_idx,
            details_hover_thumb_viewport_open,
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
            final_cover_spread_preference,
            singleton_spread_endpoint_preferences,
            page_alone_preferences,
            spread_shift,
            book_composition,
            // A fork keeps the exact editor request with its original context. Its new
            // sibling starts Closed; the mount-independent operation owner never changes.
            composition_editor,
            book_composition_permission,
            reading_flow,
            reading_direction,
            pdf_enumerate_pending,
            epub_convert,
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
            page_edit_snapshot,
            page_edit_reconcile_pending,
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
        if let Some((mut snapshot, surface)) = direct_owner {
            let basis_generation = match &snapshot.source_label {
                crate::snapshot::SnapshotSourceLabel::DirectMedia {
                    basis_generation, ..
                } => *basis_generation,
                _ => unreachable!("direct media owner has direct media source"),
            };
            let record_matches_basis = self.folder_observation.as_ref().is_some_and(|record| {
                record.generation == basis_generation
                    && record.source == snapshot.origin
                    && self.current_folder.as_ref() == Some(&record.source)
            });
            self.items = std::mem::take(&mut snapshot.saved_items);
            self.thumbnails = std::mem::take(&mut snapshot.saved_thumbnails);
            self.image_metas = std::mem::take(&mut snapshot.saved_image_metas);
            self.visible_indices = std::mem::take(&mut snapshot.saved_visible_indices);
            self.page_identity_index = snapshot.saved_page_identity_index.clone();
            self.projection_recipe = snapshot.saved_projection_recipe.clone();
            self.scroll_offset_y = snapshot.saved_scroll_offset_y;
            self.zip_nav = snapshot.saved_zip_nav.take();
            self.top_level_grid_view = top_level_grid_view::TopLevelGridView::default();
            // Appended target removal leaves every basis index stable. Retain its
            // metadata/edit caches and invalidate only generation-bound main work.
            self.bump_items_generation();
            if record_matches_basis {
                self.folder_observation.as_mut().unwrap().generation = self.items_generation;
            }
            self.bump_render_epoch_only();
            self.move_grid_cursor(
                snapshot.saved_selected,
                crate::app::GridCursorIntent::Automatic,
            );
            self.viewer_navigation_caches.invalidate();
            // The generation setter retires requests and owned CPU images together.
            let basis_len = self.items.len();
            self.checked.retain(|idx| *idx < basis_len);
            self.rating_cache.retain(|idx, _| *idx < basis_len);
            self.rotation_cache.remove(&basis_len);
            self.details_order.retain(|idx| *idx < basis_len);
            parked.navigation_scope = ViewerNavigationScope::DetachedPhysical;
            parked.snapshot = Some(snapshot);
            parked.top_level_grid_view = surface;
        }
        if let Some(record) = parked.folder_observation.as_mut() {
            record.generation = parked.items_generation;
        }
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
        detached.final_cover_spread_preference = self.final_cover_spread_preference;
        detached.singleton_spread_endpoint_preferences = self.singleton_spread_endpoint_preferences;
        detached.page_alone_preferences = self.page_alone_preferences;
        detached.spread_shift = self.spread_shift.fork_accepted();
        detached.book_composition = self.book_composition.clone();
        detached.composition_editor = Default::default();
        detached.book_composition_permission = self.book_composition_permission.clone();
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
        // This fork navigates to a physical context. Do not clone the potentially large
        // virtual-list keyed owner on the UI thread; its current index maps are copied above.
        detached.page_edit_snapshot = None;
        detached.page_edit_reconcile_pending = None;
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
        detached.display_epub_source = self.display_epub_source.clone();
        detached.fullscreen_epub_source = self.fullscreen_epub_source.clone();
        detached.current_folder = Some(physical_context.to_path_buf());
        if let Some(record) = detached.folder_observation.as_mut() {
            record.source = physical_context.to_path_buf();
            record.generation = items_generation;
            record.projection_key =
                folder_projection_key(&self.settings, detached.stack_mode_requested);
        }
        if !parked_direct_media_owner(&detached) {
            detached.visible_indices = detached
                .items
                .iter()
                .enumerate()
                .filter_map(|(item_idx, item)| {
                    item_belongs_to_detached_physical_scope(item, physical_context)
                        .then_some(item_idx)
                })
                .collect();
        }
        if !parked_direct_media_owner(&detached) {
            detached.top_level_grid_view = top_level_grid_view::TopLevelGridView::default();
            detached.snapshot = None;
        }
        detached.rating_filter_suppressed_at = None;
        detached.global_search_subfolder_restore = None;
        detached.favsearch_subfolder_restore = None;
        detached.details_thumb_suppression_applied = false;
        detached.details_order.clear();
        detached.viewer_navigation_caches.invalidate();
        super::apply_grid_cursor_move(
            &mut detached.selected,
            &mut detached.grid_click_selection_anchor,
            detached.items_generation,
            Some(idx),
            super::GridCursorMove::Move,
        );

        detached
    }
}

#[cfg(all(test, windows))]
std::thread_local! {
    static VIEWER_CONTEXT_SWAP_INTERIOR_FAILPOINT: std::cell::Cell<bool> =
        const { std::cell::Cell::new(false) };
    static VIEWER_CONTEXT_RATING_BOUNDARY_TRACE: std::cell::RefCell<Option<Vec<RatingBoundaryEvent>>> =
        const { std::cell::RefCell::new(None) };
}

#[cfg(all(test, windows))]
#[derive(Debug, PartialEq, Eq)]
struct RatingBoundaryEvent {
    boundary: &'static str,
    owner: ViewerContextId,
    seen_before: u64,
    seen_after: u64,
    write_generation: u64,
    effective_filter: [bool; 6],
    grid_cols: usize,
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
    /// Publish only when the table, projected payload and effective favorite settings
    /// describe the same mounted owner. Never call this from an exchange primitive or
    /// while building an uncommitted payload.
    fn publish_mounted_context_rating_writes_at_boundary(&mut self, _boundary: &'static str) {
        self.publish_mounted_context_rating_writes_at_boundary_with_intent(
            _boundary,
            crate::rating_membership::RatingAdoptionIntent::Reconcile,
        );
    }

    fn publish_mounted_context_rating_writes_at_boundary_with_intent(
        &mut self,
        _boundary: &'static str,
        intent: crate::rating_membership::RatingAdoptionIntent,
    ) {
        let _owner = self
            .mounted_viewer_context_id()
            .expect("rating publication boundary requires a mounted viewer owner");
        #[cfg(test)]
        let seen_before = self.rating_session_write_seen_generation;
        self.publish_current_context_rating_writes_with_intent(intent);
        #[cfg(test)]
        VIEWER_CONTEXT_RATING_BOUNDARY_TRACE.with(|trace| {
            if let Some(trace) = trace.borrow_mut().as_mut() {
                trace.push(RatingBoundaryEvent {
                    boundary: _boundary,
                    owner: _owner,
                    seen_before,
                    seen_after: self.rating_session_write_seen_generation,
                    write_generation: self.rating_session_write_generation,
                    effective_filter: self.effective_rating_filter(),
                    grid_cols: self.settings.grid_cols,
                });
            }
        });
    }

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

    pub(crate) fn detached_viewer_window_id(&self) -> Option<u64> {
        self.viewer_contexts.table.projected_window()
    }

    #[cfg(test)]
    pub(in crate::app) fn set_detached_window_binding_for_test(&mut self, window_id: Option<u64>) {
        if self.viewer_context_residence(self.projected_viewer_context_id())
            == ContextResidence::Building
        {
            self.reserve_window_binding_for_build(window_id.expect("build fixture window"));
        } else if let Some(window_id) = window_id {
            self.ensure_mounted_detached_session_binding(window_id);
        } else if let Some(window_id) = self.detached_viewer_window_id() {
            self.unbind_window(window_id);
        }
    }

    pub(in crate::app) fn viewer_context_residence(&self, id: ViewerContextId) -> ContextResidence {
        self.viewer_contexts.table.residence(id)
    }

    pub(in crate::app) fn enqueue_at_rest_book_append_observation(
        &mut self,
        context: ViewerContextId,
        folder: &Path,
        cause: book_append_refresh::FolderRefreshCause,
    ) {
        if let Some(bundle) = self.viewer_contexts.table.at_rest_mut(context)
            && bundle.current_folder.as_deref() == Some(folder)
        {
            let cause = bundle
                .folder_refresh_pending
                .take()
                .filter(|request| request.folder == folder)
                .map_or(cause.clone(), |request| {
                    request.cause.merge_for_refresh(cause)
                });
            bundle.folder_refresh_pending = Some(book_append_refresh::FolderRefreshRequest {
                folder: folder.to_path_buf(),
                cause,
            });
        }
    }

    pub(in crate::app) fn cancel_at_rest_book_append_observations(&mut self, folder: &Path) {
        for slot in self.viewer_contexts.table.slots.values_mut() {
            let Slot::AtRest(bundle) = slot else {
                continue;
            };
            if bundle
                .external_rescan_pending
                .as_ref()
                .is_some_and(|pending| {
                    pending.folder == folder && pending.cause.is_append_delivery()
                })
            {
                let pending = bundle.external_rescan_pending.take().unwrap();
                pending.cancel_and_retire();
                let cause = bundle
                    .folder_refresh_pending
                    .take()
                    .filter(|request| request.folder == folder)
                    .map_or(
                        book_append_refresh::FolderRefreshCause::BookAppendExternal,
                        |request| {
                            request.cause.merge_for_refresh(
                                book_append_refresh::FolderRefreshCause::BookAppendExternal,
                            )
                        },
                    );
                bundle.folder_refresh_pending = Some(book_append_refresh::FolderRefreshRequest {
                    folder: folder.to_path_buf(),
                    cause,
                });
            }
        }
    }

    pub(in crate::app) fn book_composition_contexts_to_poll(
        &self,
        changed: &[String],
    ) -> Vec<ViewerContextId> {
        self.viewer_contexts
            .table
            .slots
            .iter()
            .filter_map(|(id, slot)| {
                let Slot::AtRest(bundle) = slot else {
                    return None;
                };
                let needs_read = matches!(
                    bundle.book_composition,
                    super::book_composition_runtime::ViewerBookComposition::Preparing { .. }
                        | super::book_composition_runtime::ViewerBookComposition::Projecting { .. }
                ) || bundle.book_composition_permission.pending()
                    || bundle
                        .book_composition
                        .scope()
                        .is_some_and(|scope| changed.contains(&scope.storage_key()));
                needs_read.then_some(*id)
            })
            .collect()
    }

    pub(in crate::app) fn deliver_spread_shift_reply_at_rest(
        &mut self,
        reply: &crate::spread_shift::ShiftWriteReply,
    ) -> Option<bool> {
        self.viewer_contexts
            .table
            .at_rest_mut(reply.operation.context_id)
            .and_then(|bundle| {
                bundle
                    .spread_shift
                    .accept_write_reply(reply, self.settings.remember_spread_shift_per_book)
                    .or_else(|| {
                        bundle
                            .snapshot
                            .as_mut()?
                            .saved_viewing_state
                            .shift
                            .accept_write_reply(reply, self.settings.remember_spread_shift_per_book)
                    })
            })
    }

    pub(in crate::app) fn make_at_rest_spread_shifts_session_only(&mut self) {
        for slot in self.viewer_contexts.table.slots.values_mut() {
            let bundle = match slot {
                Slot::AtRest(bundle) | Slot::Retiring(bundle) => bundle,
            };
            bundle.spread_shift.session_only();
            if let Some(snapshot) = bundle.snapshot.as_mut() {
                snapshot.saved_viewing_state.shift.session_only();
            }
        }
    }

    /// Page mappings update retained pairing identities. A book-root rename follows the
    /// released reload behavior and leaves every accepted context payload at its old source.
    pub(in crate::app) fn migrate_at_rest_spread_shifts(
        &mut self,
        mappings: &[crate::books::BookPathMapping],
        root_move: Option<(&std::path::Path, &std::path::Path)>,
    ) {
        if root_move.is_some() {
            return;
        }
        for slot in self.viewer_contexts.table.slots.values_mut() {
            let bundle = match slot {
                Slot::AtRest(bundle) | Slot::Retiring(bundle) => bundle,
            };
            if super::spread_shift_live_mappings::migrate(&mut bundle.spread_shift, mappings) {
                bundle.fullscreen_navigator_interaction.invalidate();
                bundle.similar_panel.preview.invalidate();
            }
            if let Some(snapshot) = bundle.snapshot.as_mut() {
                super::spread_shift_live_mappings::migrate(
                    &mut snapshot.saved_viewing_state.shift,
                    mappings,
                );
            }
        }
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

    /// A deletion mutates the shared archive ZIP store. Apply the same source-owner transition
    /// to parked/retiring payloads without mounting them or resetting their viewer state.
    pub(in crate::app) fn invalidate_converted_archive_sources_in_parked_contexts(&mut self) {
        for slot in self.viewer_contexts.table.slots.values_mut() {
            let bundle = match slot {
                Slot::AtRest(bundle) | Slot::Retiring(bundle) => bundle,
            };
            Self::invalidate_converted_archive_source_owner(
                &mut bundle.converted_archive_cache_paths,
                &mut bundle.converted_archive_cache_paths_pending,
                &mut bundle.top_level_grid_view,
            );
        }
    }

    pub(crate) fn invalidate_removed_epub_generations(
        &mut self,
        removed: &[crate::epub_cache::GenerationRow],
    ) {
        if removed.is_empty() {
            return;
        }
        let keys = removed
            .iter()
            .map(|row| crate::epub_cache::src_key(&row.src_path))
            .collect::<std::collections::HashSet<_>>();
        let refers_to_removed = |item: &GridItem| {
            let source = match item {
                GridItem::PdfFile(path) | GridItem::PdfPage { pdf_path: path, .. } => path,
                _ => return false,
            };
            keys.contains(&crate::epub_cache::src_key(source))
        };
        if self.items.iter().any(&refers_to_removed) {
            self.bump_items_generation();
        }
        for slot in self.viewer_contexts.table.slots.values_mut() {
            let bundle = match slot {
                Slot::AtRest(bundle) | Slot::Retiring(bundle) => bundle,
            };
            if bundle.items.iter().any(&refers_to_removed) {
                bundle.set_items_generation(bundle.items_generation.wrapping_add(1));
            }
        }
        let retained_prefixes = keys
            .iter()
            .map(|key| format!("{key}::page_"))
            .collect::<Vec<_>>();
        let retained_belongs_to_removed = |item_key: &str| {
            retained_prefixes
                .iter()
                .any(|prefix| item_key.starts_with(prefix))
        };
        self.retained_final_ai_cache
            .retain(|key, _| !retained_belongs_to_removed(&key.item_key));
        self.retained_pdf_page_cache
            .retain(|key, _| !retained_belongs_to_removed(&key.item_key));
        self.retained_final_ai_epoch = self.retained_final_ai_epoch.wrapping_add(1);
    }

    /// Modal admission can be checked while a detached context is being built. Unlike `ids`,
    /// reading the projected payload and parked slots does not require a settled table operation.
    pub(in crate::app) fn document_modal_owners_in_any_context(&self) -> (bool, bool) {
        let mut epub = self.epub_convert.is_some();
        let mut password = self.pdf_password_request.is_some();
        for slot in self.viewer_contexts.table.slots.values() {
            let bundle = match slot {
                Slot::AtRest(bundle) | Slot::Retiring(bundle) => bundle,
            };
            epub |= bundle.epub_convert.is_some();
            password |= bundle.pdf_password_request.is_some();
        }
        (epub, password)
    }

    /// Every context except the one currently projected onto `App`. See `ContextTable::other_ids`.
    pub(in crate::app) fn other_viewer_context_ids(&self) -> Vec<ViewerContextId> {
        self.viewer_contexts.table.other_ids()
    }

    /// A global final-cover setting is shared by every viewer context, while the geometry and
    /// interaction owners it changes are context-local. Reset parked owners in place without
    /// mounting them or cancelling unrelated workers; their navigation demand is rebound by the
    /// normal renderer after that context is mounted again.
    pub(crate) fn invalidate_final_cover_spread_display_in_parked_contexts(&mut self) {
        for id in self.viewer_contexts.table.other_ids() {
            let Some(bundle) = self.viewer_contexts.table.at_rest_mut(id) else {
                continue;
            };
            if bundle.final_cover_spread_preference
                != crate::settings::FinalCoverSpreadPreference::FollowGlobal
            {
                continue;
            }
            bundle.similar_panel.preview.invalidate();
            bundle
                .fs_lanczos_cache
                .retain_similar_preview_resource(None);
            bundle.fullscreen_navigator_interaction.invalidate();
            if matches!(
                bundle.fs_holdover_tex,
                Some(crate::app::FsHoldover::FinalEffectSourceReload(_))
            ) {
                bundle.fs_holdover_tex = None;
            }
            bundle.fullscreen_page_layout.clear();
            bundle.fs_vertical_scroll = 0.0;
            bundle.fs_pan = egui::Vec2::ZERO;
            bundle.fs_free_rotation = 0.0;
            bundle.fs_vertical_cache_keep_set.clear();
            bundle.continuous_page_transitions.clear();
            bundle.slideshow_scroll_range_cache = None;
        }
    }

    pub(crate) fn invalidate_singleton_spread_placement_in_parked_contexts(
        &mut self,
        first_changed: bool,
        last_changed: bool,
    ) {
        for id in self.viewer_contexts.table.other_ids() {
            let Some(bundle) = self.viewer_contexts.table.at_rest_mut(id) else {
                continue;
            };
            let preferences = bundle.singleton_spread_endpoint_preferences;
            if !(first_changed
                && preferences.first
                    == crate::settings::SingletonSpreadPlacementPreference::FollowGlobal
                || last_changed
                    && preferences.last
                        == crate::settings::SingletonSpreadPlacementPreference::FollowGlobal)
            {
                continue;
            }
            bundle.similar_panel.preview.invalidate();
            bundle
                .fs_lanczos_cache
                .retain_similar_preview_resource(None);
            bundle.fullscreen_navigator_interaction.invalidate();
            if matches!(
                bundle.fs_holdover_tex,
                Some(crate::app::FsHoldover::FinalEffectSourceReload(_))
            ) {
                bundle.fs_holdover_tex = None;
            }
            bundle.fullscreen_page_layout.clear();
            bundle.fs_vertical_scroll = 0.0;
            bundle.fs_pan = egui::Vec2::ZERO;
            bundle.fs_free_rotation = 0.0;
            bundle.fs_vertical_cache_keep_set.clear();
            bundle.continuous_page_transitions.clear();
            bundle.slideshow_scroll_range_cache = None;
        }
    }

    pub(crate) fn invalidate_page_alone_in_parked_contexts(
        &mut self,
        after_cover_changed: bool,
        last_changed: bool,
    ) {
        for id in self.viewer_contexts.table.other_ids() {
            let Some(bundle) = self.viewer_contexts.table.at_rest_mut(id) else {
                continue;
            };
            if !bundle.spread_mode.has_cover() {
                continue;
            }
            let preferences = bundle.page_alone_preferences;
            if !(after_cover_changed
                && preferences.after_cover == crate::settings::PageAlonePreference::FollowGlobal
                || last_changed
                    && preferences.last == crate::settings::PageAlonePreference::FollowGlobal)
            {
                continue;
            }
            bundle.similar_panel.preview.invalidate();
            bundle
                .fs_lanczos_cache
                .retain_similar_preview_resource(None);
            bundle.fullscreen_navigator_interaction.invalidate();
            if matches!(
                bundle.fs_holdover_tex,
                Some(crate::app::FsHoldover::FinalEffectSourceReload(_))
            ) {
                bundle.fs_holdover_tex = None;
            }
            bundle.fullscreen_page_layout.clear();
            bundle.fs_vertical_scroll = 0.0;
            bundle.fs_pan = egui::Vec2::ZERO;
            bundle.fs_free_rotation = 0.0;
            bundle.fs_vertical_cache_keep_set.clear();
            bundle.continuous_page_transitions.clear();
            bundle.slideshow_scroll_range_cache = None;
        }
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

    /// Access only the editor state belonging to the still-current operation.
    pub(super) fn with_composition_editor_owner_state<R>(
        &mut self,
        owner: &super::book_composition_editor::CompositionEditorOwner,
        f: impl FnOnce(&mut super::book_composition_editor::CompositionEditorState) -> R,
    ) -> Option<R> {
        if self.composition_editor_operations.owner.as_ref() != Some(owner) {
            return None;
        }
        match self.viewer_contexts.table.residence(owner.context_id) {
            ContextResidence::Mounted => Some(f(&mut self.composition_editor)),
            ContextResidence::AtRest => self
                .viewer_contexts
                .table
                .at_rest_mut(owner.context_id)
                .map(|bundle| f(&mut bundle.composition_editor)),
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
        if self.viewer_contexts.table.mounted_id().is_none() {
            return Err(MountError {
                id,
                residence: self.viewer_contexts.table.residence(id),
            });
        }
        self.publish_mounted_context_rating_writes_at_boundary("mount_outgoing");
        let result = self.with_viewer_context_state(id, |app| {
            app.publish_mounted_context_rating_writes_at_boundary("mount_incoming");
            if let Some(ctx) = crate::page_edit_write_epoch::PAGE_EDIT_WRITES.repaint_context() {
                app.poll_page_edit_reconciliation(&ctx);
            }
            app.consume_metadata_import_display_reconciliation();
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| f(app)));
            app.publish_mounted_context_rating_writes_at_boundary("return_outgoing");
            result
        })?;
        self.publish_mounted_context_rating_writes_at_boundary("return_incoming");
        if let Some(ctx) = crate::page_edit_write_epoch::PAGE_EDIT_WRITES.repaint_context() {
            self.poll_page_edit_reconciliation(&ctx);
        }
        self.consume_metadata_import_display_reconciliation();
        match result {
            Ok(value) => Ok(value),
            Err(payload) => std::panic::resume_unwind(payload),
        }
    }

    /// Exchange the existing payloads without adopting any display publication.
    /// Service callbacks must themselves perform only owner-scoped state work.
    pub(in crate::app) fn with_viewer_context_state<R>(
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

    pub(crate) fn with_window_viewer_context_state<R>(
        &mut self,
        window_id: u64,
        f: impl FnOnce(&mut Self) -> R,
    ) -> Result<R, MountError> {
        let (id, _) = self
            .locate_window_context(window_id)
            .unwrap_or_else(|| panic!("window {window_id} has no viewer-context binding"));
        self.with_viewer_context_state(id, f)
    }

    pub(in crate::app) fn build_viewer_context(
        &mut self,
        reason: &'static str,
        f: impl FnOnce(&mut Self, ViewerContextId) -> BuildOutcome,
    ) -> Option<ViewerContextId> {
        let return_origin = self.close_return_origin();
        self.publish_mounted_context_rating_writes_at_boundary("build_outgoing");
        let (reserved, ops) = self.viewer_contexts.table.plan_begin_build();
        self.execute_viewer_context_ops(ops, None);
        self.viewer_contexts.table.finish_begin_build();
        self.set_items_generation(Self::items_generation_for_viewer_context(reserved));
        self.install_close_return_origin(return_origin);
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
        let abandoned = self.projected_viewer_context_id();
        let ops = self.viewer_contexts.table.plan_abort_build();
        self.execute_viewer_context_ops(ops, None);
        self.viewer_contexts.table.finish_abort_build();
        self.publish_mounted_context_rating_writes_at_boundary("abort_incoming");
        self.normalize_lookup_pending.remove(&abandoned);
        if self
            .normalize_state
            .as_ref()
            .is_some_and(|state| state.owner_context_id == abandoned)
        {
            self.normalize_state.take().unwrap().cancel();
        }
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
                self.publish_mounted_context_rating_writes_at_boundary("commit_incoming");
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
        mut spec: ProductionForkSpec,
    ) -> ViewerContextId {
        // The caller's target was chosen before rating publication. Publication can
        // change items/order and the reader anchor; keep the explicitly chosen page.
        let target_key = match &spec {
            ProductionForkSpec::MaterializedStill { idx, .. } => Some(
                self.page_path_key(*idx)
                    .expect("materialized still fork requires a stable page key"),
            ),
            ProductionForkSpec::LiveMedia => None,
        };
        self.publish_mounted_context_rating_writes_at_boundary_with_intent(
            "fork_outgoing",
            crate::rating_membership::RatingAdoptionIntent::Manual,
        );
        if let ProductionForkSpec::MaterializedStill { idx, .. } = &mut spec {
            let key = target_key.as_ref().unwrap();
            *idx = (0..self.items.len())
                .find(|candidate| self.page_path_key(*candidate).as_ref() == Some(key))
                .expect("materialized fork target must remain installed during publication");
        }
        let return_origin = self.close_return_origin();
        let source = self.projected_viewer_context_id();
        let scan_candidate = self.normalize_state.as_ref().and_then(|state| {
            (state.owner_context_id == source).then(|| (state.fs_idx, state.file_path.clone()))
        });
        let music_candidate = self
            .music_analysis_source
            .as_ref()
            .filter(|source_key| source_key.owner_context_id == source)
            .cloned();
        let (id, ops) = self.viewer_contexts.table.plan_fork(policy);
        self.execute_viewer_context_ops(ops, Some(spec));
        let finished = self.viewer_contexts.table.finish_fork();
        assert_eq!(finished, id);
        self.with_viewer_context_state(id, |app| app.install_close_return_origin(return_origin))
            .expect("new fork retains its return owner");
        self.publish_mounted_context_rating_writes_at_boundary("fork_incoming");
        // Music analysis lives on App, outside the viewer bundle. Transfer its exact
        // source and worker inbox only when the same player and applied track moved.
        if let Some(old_source) = music_candidate {
            let mut moved_source = old_source.clone();
            moved_source.owner_context_id = id;
            let player_moved = self
                .with_viewer_context(id, |owner| {
                    owner.music_analysis_source_for_player(moved_source.fs_idx)
                        == Some(moved_source.clone())
                })
                .expect("freshly forked viewer context must mount for music transfer");
            if player_moved && self.music_analysis_source.as_ref() == Some(&old_source) {
                self.music_analysis_source = Some(moved_source.clone());
                if let Some(pending) = self.music_analysis_pending.as_mut()
                    && pending.source == old_source
                {
                    pending.source = moved_source;
                }
            }
        }
        // A fork may move a live player. The scan and unresolved lookup then
        // follow that exact player into the new viewer, regardless of fork policy.
        let norm_enabled = self.settings.audio_normalize_enabled;
        if norm_enabled || scan_candidate.is_some() {
            let scan_moved = self
                .with_viewer_context(id, |owner| {
                    let scan_moved = scan_candidate.as_ref().is_some_and(|(idx, path)| {
                        matches!(owner.fs_cache.get(idx),
                            Some(crate::app::FsCacheEntry::Video { player, .. })
                                if player.path() == path.as_path())
                    });
                    if norm_enabled {
                        let pending: Vec<(usize, usize)> = owner
                            .fs_cache
                            .iter()
                            .filter_map(|(idx, entry)| {
                                let crate::app::FsCacheEntry::Video { player, .. } = entry else {
                                    return None;
                                };
                                let streams =
                                    player.rearm_normalize_lookups_after_context_transfer();
                                Some(streams.into_iter().map(|stream| (*idx, stream)))
                            })
                            .flatten()
                            .collect();
                        for (idx, stream) in pending {
                            owner.start_normalize_lookup_for_stream(idx, stream);
                        }
                    }
                    scan_moved
                })
                .expect("freshly forked viewer context must mount for Norm transfer");
            if scan_moved {
                self.normalize_state.as_mut().unwrap().owner_context_id = id;
            }
        }
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
        let presenter_owner_transferred = self
            .with_viewer_context(parked, |app| {
                app.video_audio_mode_runtime
                    .as_mut()
                    .is_none_or(|runtime| runtime.transfer_context_owner(source, parked))
            })
            .expect("freshly forked live-media context must be mountable");
        if !presenter_owner_transferred {
            crate::logger::log(format!(
                "[video-audio] live-media context transfer cancelled stale presenter owner from={source:?} to={parked:?}"
            ));
        }
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
        self.close_composition_editor_for_context(id);
        self.cancel_image_folder_zip_for_context(id);
        self.viewer_contexts.table.begin_retire(id)?;
        self.retire_viewer_reopen_requests(id);
        if let Some(window_id) = retiring_window {
            self.detached_window_manager
                .transfer_viewport_retirement(window_id);
        }
        let commits = self
            .viewer_contexts
            .table
            .retiring_slot_mut(id)
            .unwrap()
            .take_page_edit_commits(page_edit_owner::PageEditBoundary::Exit);
        self.deliver_page_edit_commits(commits);
        self.cancel_rating_membership_restore_for_context(id);
        if retiring_window == self.active_detached_window_id() {
            self.begin_active_detached_session_close(reason);
            self.finish_active_detached_session_close(reason);
        }
        let value = {
            let payload = self.viewer_contexts.table.retiring_slot_mut(id).unwrap();
            payload.pending_fs_page_turn_ready = None;
            payload.page_turn_presentation.deactivate("retired");
            digest(ContextMut { bundle: payload })
        };
        if let Some(window_id) = retiring_window {
            assert_eq!(self.unbind_window(window_id), Some(id));
        }
        let ops = self.viewer_contexts.table.plan_finish_retire(id);
        self.execute_viewer_context_ops(ops, None);
        self.viewer_contexts.table.finish_retire();
        self.normalize_lookup_pending.remove(&id);
        if self
            .music_analysis_source
            .as_ref()
            .is_some_and(|source| source.owner_context_id == id)
        {
            self.clear_music_view_state();
        }
        if self
            .normalize_state
            .as_ref()
            .is_some_and(|state| state.owner_context_id == id)
        {
            self.normalize_state.take().unwrap().cancel();
        }
        Ok(value)
    }

    pub(in crate::app) fn close_and_retire_context<D>(
        &mut self,
        id: ViewerContextId,
        reason: &'static str,
        finish: impl FnOnce(&mut Self),
        digest: impl FnOnce(ContextMut<'_>) -> D,
    ) -> Result<D, RetireContextError> {
        self.with_viewer_context_state(id, finish)
            .map_err(RetireContextError::Mount)?;
        self.retire_context(id, reason, digest)
            .map_err(RetireContextError::Retire)
    }

    pub(in crate::app) fn stash_mounted_and_start_fresh(
        &mut self,
        _reason: &'static str,
    ) -> ViewerContextId {
        self.publish_mounted_context_rating_writes_at_boundary("promote_outgoing");
        // The outgoing bundle becomes an independent snapshot reader. Retire
        // its watch before the table deposits it, retaining active operation
        // delivery in that same bundle rather than handing it to the new main.
        self.retire_passive_folder_observation();
        let ops = self.viewer_contexts.table.plan_promote();
        self.execute_viewer_context_ops(ops, None);
        let stashed = self.viewer_contexts.table.finish_promote();
        self.publish_mounted_context_rating_writes_at_boundary("promote_incoming");
        stashed
    }
}

#[cfg(not(windows))]
impl App {
    pub(super) fn with_composition_editor_owner_state<R>(
        &mut self,
        owner: &super::book_composition_editor::CompositionEditorOwner,
        f: impl FnOnce(&mut super::book_composition_editor::CompositionEditorState) -> R,
    ) -> Option<R> {
        (self.composition_editor_operations.owner.as_ref() == Some(owner)
            && owner.context_id == ViewerContextId::single_context())
        .then(|| f(&mut self.composition_editor))
    }

    pub(in crate::app) fn with_viewer_context_state<R>(
        &mut self,
        id: ViewerContextId,
        f: impl FnOnce(&mut Self) -> R,
    ) -> Result<R, MountError> {
        self.with_viewer_context(id, f)
    }

    pub(in crate::app) fn viewer_context_main(&self) -> ViewerContextId {
        ViewerContextId::single_context()
    }

    pub(in crate::app) fn viewer_context_ids(&self) -> Vec<ViewerContextId> {
        vec![ViewerContextId::single_context()]
    }

    pub(in crate::app) fn viewer_context_residence(&self, id: ViewerContextId) -> ContextResidence {
        if id == ViewerContextId::single_context() {
            ContextResidence::Mounted
        } else {
            ContextResidence::Unknown
        }
    }

    /// Non-Windows builds have one viewer payload and therefore one projected identity.
    pub(in crate::app) fn projected_viewer_context_id(&self) -> ViewerContextId {
        ViewerContextId::single_context()
    }

    pub(in crate::app) fn with_viewer_context<R>(
        &mut self,
        id: ViewerContextId,
        f: impl FnOnce(&mut Self) -> R,
    ) -> Result<R, MountError> {
        if id != ViewerContextId::single_context() {
            return Err(MountError {
                id,
                residence: ContextResidence::Unknown,
            });
        }
        Ok(f(self))
    }
}

#[cfg(all(test, windows))]
impl App {
    pub(super) fn start_rating_boundary_trace_for_test() {
        VIEWER_CONTEXT_RATING_BOUNDARY_TRACE.with(|trace| *trace.borrow_mut() = Some(Vec::new()));
    }

    pub(super) fn take_rating_boundary_trace_for_test() -> Vec<(&'static str, ViewerContextId)> {
        VIEWER_CONTEXT_RATING_BOUNDARY_TRACE.with(|trace| {
            trace
                .borrow_mut()
                .as_mut()
                .map(std::mem::take)
                .unwrap_or_default()
                .into_iter()
                .map(|event| (event.boundary, event.owner))
                .collect()
        })
    }

    pub(super) fn stop_rating_boundary_trace_for_test() {
        VIEWER_CONTEXT_RATING_BOUNDARY_TRACE.with(|trace| *trace.borrow_mut() = None);
    }

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

    #[cfg(test)]
    pub(crate) fn with_window_context_for_test<R>(
        &mut self,
        id: ViewerContextId,
        f: impl FnOnce(&mut Self) -> R,
    ) -> R {
        self.with_viewer_context(id, f)
            .expect("test window viewer context must mount")
    }
    pub(crate) fn push_window_context_for_test(
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
            image_underlay: super::DetachedImageWindowUnderlay::Solid(egui::Color32::BLACK),
            frozen_continuous_pages: Vec::new(),
            reopen_descriptor: None,
            reopen_sync_stamp: None,
            reopen_collection_root: None,
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
            content_phase: DetachedSessionContentPhase::Ready,
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
    fn seed_c1_subscription_scan(
        app: &mut App,
        cause: book_append_refresh::FolderRefreshCause,
    ) -> Arc<AtomicBool> {
        let folder = crate::data_dir::get().join("c1-reader");
        app.current_folder = Some(folder.clone());
        let watch = CurrentFolderWatch::new(folder.clone(), 0);
        let epoch = watch.epoch;
        app.current_folder_watch = Some(watch);
        let (_tx, rx) = mpsc::channel();
        let cancel = Arc::new(AtomicBool::new(false));
        app.external_rescan_pending = Some(ExternalRescanPending {
            proof: Some(FolderRescanProof {
                baseline: None,
                projection_key: 0,
                subscription: Some((epoch, 0)),
            }),
            cause,
            owner_context_id: app.projected_viewer_context_id(),
            folder,
            mtime: std::time::UNIX_EPOCH,
            items_generation: app.items_generation,
            scan_options: app.current_external_scan_options(),
            cancel: Arc::clone(&cancel),
            rx,
            restart_requested: true,
        });
        cancel
    }

    #[cfg(windows)]
    #[test]
    fn section1375_c1_linked_activation_keeps_main_observation_and_operation() {
        let mut app = crate::app::setup_app_for_test();
        let cancel =
            seed_c1_subscription_scan(&mut app, book_append_refresh::FolderRefreshCause::Operation);
        let epoch = app.current_folder_watch.as_ref().unwrap().epoch;
        let main = app.viewer_context_main();
        app.activate_mounted_as_independent_detached(9137);
        assert_eq!(app.projected_viewer_context_id(), main);
        assert_eq!(app.current_folder_watch.as_ref().unwrap().epoch, epoch);
        assert!(app.external_rescan_pending.is_some());
        assert!(!cancel.load(Ordering::Relaxed));
    }

    #[cfg(windows)]
    #[test]
    fn section1375_c1_promote_retires_only_passive_observation_in_outgoing_owner() {
        for cause in [
            book_append_refresh::FolderRefreshCause::External,
            book_append_refresh::FolderRefreshCause::Operation,
            book_append_refresh::FolderRefreshCause::BookAppendExternal,
        ] {
            let passive = cause.is_passive_observation();
            let append = cause.is_append_delivery();
            let mut app = crate::app::setup_app_for_test();
            let cancel = seed_c1_subscription_scan(&mut app, cause.clone());
            let folder = app.current_folder.clone().unwrap();
            app.folder_refresh_pending = Some(book_append_refresh::FolderRefreshRequest {
                folder: folder.clone(),
                cause,
            });
            let old_main = app.viewer_context_main();
            assert_eq!(
                app.stash_mounted_and_start_fresh("c1_promote_observation"),
                old_main
            );
            let new_main = app.viewer_context_main();
            assert_ne!(old_main, new_main);
            assert!(cancel.load(Ordering::Relaxed));
            assert!(app.folder_refresh_pending.is_none());
            let new_folder = app.tmp.path().join("new-main");
            app.current_folder_watch = Some(CurrentFolderWatch::new(new_folder, 17));
            let new_epoch = app.current_folder_watch.as_ref().unwrap().epoch;
            for _ in 0..2 {
                app.with_viewer_context(old_main, |owner| {
                    assert!(owner.current_folder_watch.is_none());
                    assert!(owner.external_rescan_pending.is_none());
                    if passive {
                        assert!(owner.folder_refresh_pending.is_none());
                    } else {
                        let request = owner.folder_refresh_pending.as_ref().unwrap();
                        assert_eq!(request.folder, folder);
                        assert!(!request.cause.is_passive_observation());
                        assert_eq!(request.cause.is_append_delivery(), append);
                    }
                })
                .unwrap();
                assert_eq!(app.viewer_context_main(), new_main);
                assert_eq!(app.current_folder_watch.as_ref().unwrap().epoch, new_epoch);
            }
        }
    }

    #[cfg(windows)]
    #[test]
    fn section1375_c1_live_fork_keeps_main_watch_and_operation_without_reader_watch() {
        let mut app = crate::app::setup_app_for_test();
        let cancel =
            seed_c1_subscription_scan(&mut app, book_append_refresh::FolderRefreshCause::Operation);
        let main = app.viewer_context_main();
        let epoch = app.current_folder_watch.as_ref().unwrap().epoch;
        app.bind_window(main, 9138).unwrap();
        let reader = app.fork_mounted_live_media_context(9138);
        assert_ne!(reader, main);
        assert_eq!(app.current_folder_watch.as_ref().unwrap().epoch, epoch);
        assert!(app.external_rescan_pending.is_some());
        assert!(!cancel.load(Ordering::Relaxed));
        app.with_viewer_context(reader, |owner| {
            assert!(owner.current_folder_watch.is_none());
            assert!(owner.external_rescan_pending.is_none());
            assert!(owner.folder_refresh_pending.is_none());
        })
        .unwrap();
        assert_eq!(app.current_folder_watch.as_ref().unwrap().epoch, epoch);
        assert!(!cancel.load(Ordering::Relaxed));
    }

    #[cfg(windows)]
    #[test]
    fn section1375_c1_at_rest_append_cancel_preserves_general_operation() {
        let mut app = crate::app::setup_app_for_test();
        let reader = app.build_window_context_for_test(9139, |_| {});
        let (folder, cancel) = app
            .with_viewer_context(reader, |owner| {
                let cancel = seed_c1_subscription_scan(
                    owner,
                    book_append_refresh::FolderRefreshCause::Operation,
                );
                (owner.current_folder.clone().unwrap(), cancel)
            })
            .unwrap();
        app.cancel_at_rest_book_append_observations(&folder);
        assert!(!cancel.load(Ordering::Relaxed));
        app.with_viewer_context(reader, |owner| {
            assert!(matches!(
                owner.external_rescan_pending.as_ref().unwrap().cause,
                book_append_refresh::FolderRefreshCause::Operation
            ));
            owner.folder_refresh_pending = Some(book_append_refresh::FolderRefreshRequest {
                folder: folder.clone(),
                cause: book_append_refresh::FolderRefreshCause::Operation,
            });
        })
        .unwrap();
        app.enqueue_at_rest_book_append_observation(
            reader,
            &folder,
            book_append_refresh::FolderRefreshCause::BookAppendExternal,
        );
        app.with_viewer_context(reader, |owner| {
            assert!(matches!(
                owner.folder_refresh_pending.as_ref().unwrap().cause,
                book_append_refresh::FolderRefreshCause::BookAppendExternal
            ));
        })
        .unwrap();
    }

    #[cfg(windows)]
    fn pdf_promotion_for_test(
        ticket: &crate::pdf_loader::PdfRenderEpochTicket,
        retry_needed: bool,
    ) -> super::super::PdfVisiblePromotionState {
        super::super::PdfVisiblePromotionState::Tracking {
            ticket: ticket.clone(),
            keys: std::collections::HashSet::from(["same.pdf:p:0@256".to_owned()]),
            retry_needed,
        }
    }

    #[cfg(windows)]
    fn assert_pdf_promotion_for_test(
        state: &super::super::PdfVisiblePromotionState,
        expected: &crate::pdf_loader::PdfRenderEpochTicket,
        expected_retry: bool,
    ) {
        let super::super::PdfVisiblePromotionState::Tracking {
            ticket,
            keys,
            retry_needed,
        } = state
        else {
            panic!("promotion tracking must travel with its viewer");
        };
        assert!(ticket.same_request(expected));
        assert_eq!(
            keys,
            &std::collections::HashSet::from(["same.pdf:p:0@256".to_owned()])
        );
        assert_eq!(*retry_needed, expected_retry);
    }

    #[cfg(windows)]
    #[test]
    fn pdf_epoch_registry_empty_drop_and_swap_keep_only_their_own_writer() {
        let mut app = crate::app::setup_app_for_test();
        let main = app.pdf_render_epoch.snapshot();
        app.pdf_visible_promotion = pdf_promotion_for_test(&main, false);
        let empty = ViewerContextBundle::empty();
        let empty_ticket = empty.pdf_render_epoch.snapshot();
        assert!(!empty_ticket.same_request(&main));
        drop(empty);
        assert!(empty_ticket.is_stale());
        assert!(!main.is_stale());

        let mut sibling = ViewerContextBundle::empty();
        let other = sibling.pdf_render_epoch.snapshot();
        sibling.pdf_visible_promotion = pdf_promotion_for_test(&other, true);
        app.swap_viewer_context_bundle(&mut sibling);
        assert!(app.pdf_render_epoch.snapshot().same_request(&other));
        assert_pdf_promotion_for_test(&app.pdf_visible_promotion, &other, true);
        assert_pdf_promotion_for_test(&sibling.pdf_visible_promotion, &main, false);
        assert!(!main.is_stale());
        assert!(!other.is_stale());
        app.swap_viewer_context_bundle(&mut sibling);
        assert!(app.pdf_render_epoch.snapshot().same_request(&main));
        assert_pdf_promotion_for_test(&app.pdf_visible_promotion, &main, false);
        sibling.cancel_all_context_work();
        sibling.cancel_all_context_work();
        assert!(matches!(
            sibling.pdf_visible_promotion,
            super::super::PdfVisiblePromotionState::Idle
        ));
        drop(sibling);
        assert!(other.is_stale());
        assert!(!main.is_stale());
        drop(app);
        assert!(main.is_stale());
    }

    #[cfg(windows)]
    #[test]
    fn pdf_epoch_registry_build_commit_abort_promote_and_retire_are_context_local() {
        let mut app = crate::app::tests::phase_c_support::setup_app();
        let main = app.pdf_render_epoch.snapshot();
        app.pdf_visible_promotion = pdf_promotion_for_test(&main, false);
        let mut committed_ticket = None;
        let committed = app
            .build_viewer_context("pdf_epoch_commit", |owner, _| {
                let ticket = owner.pdf_render_epoch.snapshot();
                assert!(!ticket.same_request(&main));
                assert!(matches!(
                    owner.pdf_visible_promotion,
                    super::super::PdfVisiblePromotionState::Idle
                ));
                owner.pdf_visible_promotion = pdf_promotion_for_test(&ticket, true);
                committed_ticket = Some(ticket);
                BuildOutcome::Commit
            })
            .unwrap();
        let committed_ticket = committed_ticket.unwrap();
        assert!(app.pdf_render_epoch.snapshot().same_request(&main));
        assert_pdf_promotion_for_test(&app.pdf_visible_promotion, &main, false);
        app.with_viewer_context(committed, |owner| {
            assert!(
                owner
                    .pdf_render_epoch
                    .snapshot()
                    .same_request(&committed_ticket)
            );
            assert_pdf_promotion_for_test(&owner.pdf_visible_promotion, &committed_ticket, true);
        })
        .unwrap();

        let mut aborted_ticket = None;
        assert_eq!(
            app.build_viewer_context("pdf_epoch_abort", |owner, _| {
                let ticket = owner.pdf_render_epoch.snapshot();
                owner.pdf_visible_promotion = pdf_promotion_for_test(&ticket, true);
                aborted_ticket = Some(ticket);
                BuildOutcome::Abort("test")
            }),
            None
        );
        assert!(aborted_ticket.unwrap().is_stale());
        assert!(!main.is_stale());
        assert!(!committed_ticket.is_stale());
        assert_pdf_promotion_for_test(&app.pdf_visible_promotion, &main, false);

        let old_main = app.stash_mounted_and_start_fresh("pdf_epoch_promote");
        let fresh = app.pdf_render_epoch.snapshot();
        assert!(!fresh.same_request(&main));
        assert!(matches!(
            app.pdf_visible_promotion,
            super::super::PdfVisiblePromotionState::Idle
        ));
        app.with_viewer_context(old_main, |owner| {
            assert!(owner.pdf_render_epoch.snapshot().same_request(&main));
            assert_pdf_promotion_for_test(&owner.pdf_visible_promotion, &main, false);
        })
        .unwrap();
        app.retire_context(committed, "pdf_epoch_retire", |_| ())
            .unwrap();
        assert!(committed_ticket.is_stale());
        assert!(!main.is_stale());
        assert!(!fresh.is_stale());
        app.retire_context(old_main, "pdf_epoch_old_main_retire", |_| ())
            .unwrap();
        assert!(main.is_stale());
        assert!(!fresh.is_stale());
    }

    #[cfg(windows)]
    #[test]
    fn pdf_epoch_registry_ordinary_park_resume_preserves_ticket_and_same_key_tracking() {
        let mut app = crate::app::tests::phase_c_support::setup_app();
        let mut ticket = None;
        let parked = app
            .build_viewer_context("pdf_epoch_park", |owner, _| {
                let current = owner.pdf_render_epoch.snapshot();
                owner.pdf_visible_promotion = pdf_promotion_for_test(&current, true);
                ticket = Some(current);
                BuildOutcome::Commit
            })
            .unwrap();
        let ticket = ticket.unwrap();
        let main = app.pdf_render_epoch.snapshot();
        app.with_viewer_context(parked, |owner| {
            owner.pause_mounted_background_work_keep_current_frame();
            assert!(owner.pdf_render_epoch.snapshot().same_request(&ticket));
            assert_pdf_promotion_for_test(&owner.pdf_visible_promotion, &ticket, true);
        })
        .unwrap();
        assert!(!ticket.is_stale());
        app.bump_render_epoch_only();
        assert!(main.is_stale());
        assert!(!ticket.is_stale());
        let new_main = app.pdf_render_epoch.snapshot();
        app.with_viewer_context(parked, |owner| {
            assert!(owner.pdf_render_epoch.snapshot().same_request(&ticket));
            assert_pdf_promotion_for_test(&owner.pdf_visible_promotion, &ticket, true);
            owner.bump_render_epoch_only();
            assert!(ticket.is_stale());
            assert!(!new_main.is_stale());
            assert!(matches!(
                owner.pdf_visible_promotion,
                super::super::PdfVisiblePromotionState::Idle
            ));
        })
        .unwrap();
        assert!(app.pdf_render_epoch.snapshot().same_request(&new_main));
    }

    #[cfg(windows)]
    #[test]
    fn pdf_epoch_registry_both_fork_policies_keep_source_writer_and_fork_fresh_idle() {
        for live in [false, true] {
            let mut app = crate::app::tests::phase_c_support::setup_app();
            let root = app.tmp.path().join("pdf-epoch-fork");
            app.current_folder = Some(root.clone());
            app.items = vec![GridItem::PdfPage {
                pdf_path: root.join("same.pdf"),
                page_num: 0,
                content_type: None,
            }];
            app.thumbnails = vec![ThumbnailState::Pending];
            app.image_metas = vec![None];
            app.visible_indices = vec![0];
            app.fullscreen_idx = Some(0);
            let main = app.pdf_render_epoch.snapshot();
            app.pdf_visible_promotion = pdf_promotion_for_test(&main, true);
            let generation = app.items_generation;
            let request_id = app
                .requested
                .begin(
                    0,
                    generation,
                    super::super::ThumbnailRequestKind::Normal,
                    None,
                )
                .unwrap();
            let queued = LoadRequest {
                idx: 0,
                pdf_render_validity: crate::pdf_loader::PdfRenderValidity::Viewer(main.clone()),
                ..Default::default()
            };
            let queue = Arc::new((Mutex::new(vec![queued]), Condvar::new()));
            app.reload_queue = Some(queue.clone());
            let cancel = app.cancel_token.clone();
            let fork = if live {
                let main = app.viewer_context_main();
                app.bind_window(main, 8131).unwrap();
                app.fork_mounted_live_media_context(8131)
            } else {
                app.fork_materialized_still_context(&root, 0)
            };
            assert!(app.pdf_render_epoch.snapshot().same_request(&main));
            assert_pdf_promotion_for_test(&app.pdf_visible_promotion, &main, true);
            assert_eq!(app.requested.id(0), Some(request_id));
            assert!(Arc::ptr_eq(app.reload_queue.as_ref().unwrap(), &queue));
            assert!(Arc::ptr_eq(&app.cancel_token, &cancel));
            assert!(!cancel.load(Ordering::Relaxed));
            assert_eq!(queue.0.lock().unwrap().len(), 1);
            let fork_ticket = app
                .with_viewer_context(fork, |owner| {
                    let ticket = owner.pdf_render_epoch.snapshot();
                    assert!(!ticket.same_request(&main));
                    assert!(matches!(
                        owner.pdf_visible_promotion,
                        super::super::PdfVisiblePromotionState::Idle
                    ));
                    assert!(owner.requested.is_empty());
                    assert!(owner.reload_queue.is_none());
                    assert!(owner.heavy_io_queue.is_none());
                    ticket
                })
                .unwrap();
            assert!(!main.is_stale());
            app.retire_context(fork, "pdf_epoch_fork_retire", |_| ())
                .unwrap();
            assert!(fork_ticket.is_stale());
            assert!(!main.is_stale());
        }
    }

    #[cfg(windows)]
    #[test]
    fn pdf_epoch_registry_direct_reader_basis_restore_advances_only_main() {
        for materialized in [false, true] {
            let mut app = crate::app::setup_app_for_test();
            let root = app.tmp.path().join("pdf-epoch-direct");
            let basis = GridItem::PdfPage {
                pdf_path: root.join("same.pdf"),
                page_num: 0,
                content_type: None,
            };
            let direct = GridItem::Image(root.join("direct.png"));
            app.current_folder = Some(root.clone());
            app.items = vec![basis.clone(), direct.clone()];
            app.thumbnails = vec![ThumbnailState::Pending; 2];
            app.image_metas = vec![None; 2];
            app.visible_indices = vec![0, 1];
            app.fullscreen_idx = Some(1);
            app.snapshot = Some(crate::snapshot::SnapshotState {
                page_edit_source: crate::snapshot::SnapshotPageEditSource::PhysicalPrefix,
                items: Vec::new(),
                membership: Default::default(),
                origin: root.clone(),
                filter_at_capture: crate::snapshot::FilterState::from_rating([true; 6]),
                source_label: crate::snapshot::SnapshotSourceLabel::DirectMedia {
                    target: root.join("direct.png"),
                    policy: app.settings.file_type_visibility.clone(),
                    reader_order: vec![0, 1],
                    details_sort: None,
                    basis_generation: app.items_generation,
                },
                generation_id: app.items_generation,
                saved_items: vec![basis.clone()],
                saved_projection_recipe: app.projection_recipe.clone(),
                saved_viewing_state: app.capture_snapshot_viewing_state(),
                saved_page_identity_index: app.page_identity_index.clone(),
                saved_thumbnails: vec![ThumbnailState::Pending],
                saved_image_metas: vec![None],
                saved_visible_indices: vec![0],
                saved_scroll_offset_y: 0.0,
                saved_selected: Some(0),
                saved_zip_nav: None,
                list_view_items: vec![basis.clone(), direct],
                list_view_projection_recipe: app.projection_recipe.clone(),
                list_view_thumbnails: vec![ThumbnailState::Pending; 2],
                list_view_image_metas: vec![None; 2],
                list_view_page_identity_index: app.page_identity_index.clone(),
                pre_snapshot_search_origin: None,
            });
            let main = app.pdf_render_epoch.snapshot();
            app.pdf_visible_promotion = pdf_promotion_for_test(&main, true);
            let sibling = ViewerContextBundle::empty();
            let sibling_ticket = sibling.pdf_render_epoch.snapshot();
            let fork = if materialized {
                app.split_materialized_physical_context_for_detached_scope(&root, 1, 991)
            } else {
                app.split_current_context_preserving_main_grid()
            };
            assert_eq!(app.items, vec![basis]);
            assert!(!app.direct_media_reader_active());
            assert!(
                main.is_stale(),
                "restoring the physical idx basis invalidates old main work"
            );
            assert!(!app.pdf_render_epoch.snapshot().is_stale());
            assert!(matches!(
                app.pdf_visible_promotion,
                super::super::PdfVisiblePromotionState::Idle
            ));
            assert!(!sibling_ticket.is_stale());
            let fork_ticket = fork.pdf_render_epoch.snapshot();
            assert!(!fork_ticket.is_stale());
            assert!(!fork_ticket.same_request(&app.pdf_render_epoch.snapshot()));
            assert!(matches!(
                fork.pdf_visible_promotion,
                super::super::PdfVisiblePromotionState::Idle
            ));
            app.bump_render_epoch_only();
            assert!(!fork_ticket.is_stale());
            assert!(!sibling_ticket.is_stale());
        }
    }

    #[cfg(windows)]
    struct RatingBoundaryTrace;

    #[cfg(windows)]
    impl RatingBoundaryTrace {
        fn start() -> Self {
            VIEWER_CONTEXT_RATING_BOUNDARY_TRACE.with(|trace| {
                assert!(trace.borrow().is_none());
                *trace.borrow_mut() = Some(Vec::new());
            });
            Self
        }

        fn take(&self) -> Vec<RatingBoundaryEvent> {
            VIEWER_CONTEXT_RATING_BOUNDARY_TRACE
                .with(|trace| std::mem::take(trace.borrow_mut().as_mut().unwrap()))
        }
    }

    #[cfg(windows)]
    impl Drop for RatingBoundaryTrace {
        fn drop(&mut self) {
            VIEWER_CONTEXT_RATING_BOUNDARY_TRACE.with(|trace| *trace.borrow_mut() = None);
        }
    }

    #[cfg(windows)]
    fn seed_rating_boundary_page(app: &mut App, path: &Path, stars: u8) -> String {
        app.items = vec![GridItem::Image(path.to_path_buf())];
        app.thumbnails = vec![ThumbnailState::Pending];
        app.image_metas = vec![None];
        app.visible_indices = vec![0];
        app.selected = Some(0);
        app.rating_cache.insert(0, stars);
        let key = app.rating_path_key(0).unwrap();
        app.rating_db.as_ref().unwrap().set(&key, stars).unwrap();
        key
    }

    #[cfg(windows)]
    #[test]
    fn section1373_append_terminal_preserves_later_add_and_clear_in_mounted_and_parked_contexts() {
        for edited in [true, false] {
            let mut app = crate::app::tests::phase_c_support::setup_app();
            let folder = app.tmp.path().join("append-terminal");
            let destination = folder.join("0001_copy.jpg");
            std::fs::create_dir_all(&folder).unwrap();
            std::fs::write(&destination, b"append output already visible").unwrap();
            let source = app.tmp.path().join("source.jpg");
            let key = crate::adjustment_db::normalize_path(&destination);
            let source_key = crate::adjustment_db::normalize_path(&source);
            let stars = if edited { 5 } else { 0 };
            let tags = if edited {
                vec!["#later".to_owned()]
            } else {
                Vec::new()
            };
            let seed = |owner: &mut App| {
                owner.items = vec![
                    GridItem::Image(destination.clone()),
                    GridItem::Image(source.clone()),
                ];
                owner.rating_cache.insert(0, stars);
                owner.rating_cache.insert(1, 3);
                owner.tags_cache.insert(key.clone(), tags.clone());
                owner
                    .tags_cache
                    .insert(source_key.clone(), vec!["#source".into()]);
                owner
                    .adjustment_page_params
                    .insert(1, crate::adjustment::AdjustParams::default());
                owner.local_adjust_selected_layers.insert(1, 2);
                owner.local_adjust_page_layers.insert(1, Vec::new().into());
                for pages in [
                    &mut owner.local_adjust_pages,
                    &mut owner.export_crop_pages,
                    &mut owner.mask_pages,
                    &mut owner.conceal_pages,
                    &mut owner.comic_pages,
                ] {
                    pages.insert(1);
                    if edited {
                        pages.insert(0);
                    } else {
                        pages.remove(&0);
                    }
                }
                if edited {
                    owner
                        .adjustment_page_params
                        .insert(0, crate::adjustment::AdjustParams::default());
                } else {
                    owner.adjustment_page_params.remove(&0);
                }
                owner.erase_mask = Some(vec![edited, !edited]);
            };
            seed(&mut app);
            let parked = app
                .build_viewer_context("append_later_edit", |owner, _| {
                    seed(owner);
                    BuildOutcome::Commit
                })
                .unwrap();
            seed(&mut app);
            // Represent a later accepted edit/clear after the output became visible,
            // while the append result is still waiting for its UI poll.
            let owner: &mut App = &mut app;
            for keys in [
                &mut owner.adjusted_page_keys,
                &mut owner.local_adjust_page_keys,
                &mut owner.export_crop_page_keys,
                &mut owner.mask_page_keys,
                &mut owner.conceal_page_keys,
                &mut owner.comic_page_keys,
            ] {
                keys.insert(source_key.clone());
                if edited {
                    keys.insert(key.clone());
                } else {
                    keys.remove(&key);
                }
            }
            app.record_rating_session_write(key.clone(), stars, true);
            let generation = app.rating_session_write_generation;
            let revision = app.details_cell_content_revisions.tags;
            let projected = app.projected_viewer_context_id();
            let summary = crate::books::BookAppendSummary {
                book_name: "book".into(),
                folder,
                added: 1,
                first_path: Some(destination),
                edit_copies: Vec::new(),
                semantic_copies: Vec::new(),
                erase_fallback_pages: 0,
                skipped_hidden: 0,
                undo_files: None,
                metadata_copy: crate::book_append_undo::BookAppendMetadataCopySummary {
                    adjusted: vec![key.clone()],
                    local_adjusted: vec![key.clone()],
                    cropped: vec![key.clone()],
                    erased: vec![key.clone()],
                    concealed: vec![key.clone()],
                    annotated: vec![key.clone()],
                    ratings: vec![key.clone()],
                    tags: vec![key.clone()],
                    ..Default::default()
                },
            };
            app.finish_book_append_metadata(&summary);
            assert_eq!(app.projected_viewer_context_id(), projected);
            assert_eq!(app.rating_session_write_generation, generation);
            assert_eq!(app.details_cell_content_revisions.tags, revision);
            assert_eq!(app.rating_cache.get(&0), Some(&stars));
            assert_eq!(app.rating_cache.get(&1), Some(&3));
            assert_eq!(app.tags_cache.get(&key), Some(&tags));
            assert_eq!(
                app.tags_cache.get(&source_key),
                Some(&vec!["#source".into()])
            );
            assert_eq!(
                app.erase_mask.as_deref(),
                Some([edited, !edited].as_slice())
            );
            assert_eq!(app.adjustment_page_params.contains_key(&0), edited);
            for keys in [
                &app.adjusted_page_keys,
                &app.local_adjust_page_keys,
                &app.export_crop_page_keys,
                &app.mask_page_keys,
                &app.conceal_page_keys,
                &app.comic_page_keys,
            ] {
                assert_eq!(keys.contains(&key), edited);
                assert!(keys.contains(&source_key));
            }
            let bundle = app.viewer_contexts.table.at_rest_mut(parked).unwrap();
            assert_eq!(bundle.rating_cache.get(&0), Some(&stars));
            assert_eq!(bundle.rating_cache.get(&1), Some(&3));
            assert_eq!(bundle.tags_cache.get(&key), Some(&tags));
            assert_eq!(
                bundle.tags_cache.get(&source_key),
                Some(&vec!["#source".into()])
            );
            assert_eq!(bundle.adjustment_page_params.contains_key(&0), edited);
            for pages in [
                &bundle.local_adjust_pages,
                &bundle.export_crop_pages,
                &bundle.mask_pages,
                &bundle.conceal_pages,
                &bundle.comic_pages,
            ] {
                assert_eq!(pages.contains(&0), edited);
                assert!(pages.contains(&1));
            }
            assert!(bundle.adjustment_page_params.contains_key(&1));
            assert_eq!(bundle.local_adjust_selected_layers.get(&1), Some(&2));
            assert!(bundle.local_adjust_page_layers.contains_key(&1));
        }
    }

    #[cfg(windows)]
    #[test]
    fn rating_boundary_swap_never_publishes_during_table_transition() {
        let mut app = crate::app::tests::phase_c_support::setup_app();
        let path = app.tmp.path().join("swap-rating.jpg");
        let key = seed_rating_boundary_page(&mut app, &path, 3);
        let other = app
            .build_viewer_context("rating_swap_other", |owner, _| {
                seed_rating_boundary_page(owner, &path, 3);
                BuildOutcome::Commit
            })
            .unwrap();
        app.write_user_rating_shared_deferred(&key, 4, None)
            .unwrap();
        let trace = RatingBoundaryTrace::start();
        let publications = app.rating_publication_count;
        let ops = app.viewer_contexts.table.plan_mount(other).unwrap();
        app.execute_viewer_context_ops(ops, None);
        // This accessor is deliberately unavailable while pending: the primitive must
        // leave rating caches and publication receipts with their original payloads.
        assert_eq!(app.rating_publication_count, publications);
        assert_eq!(app.rating_session_write_seen_generation, 0);
        assert_eq!(app.rating_cache.get(&0), Some(&3));
        assert!(trace.take().is_empty());
        app.viewer_contexts.table.finish_mount();
        app.publish_mounted_context_rating_writes_at_boundary("test_finished_mount");
        assert_eq!(app.projected_viewer_context_id(), other);
        assert_eq!(app.rating_publication_count, publications + 1);
        assert_eq!(app.rating_cache.get(&0), Some(&4));
        assert_eq!(trace.take()[0].owner, other);
    }

    #[cfg(windows)]
    #[test]
    fn rating_boundary_mount_and_return_publish_unobserved_writes_with_effective_settings() {
        let mut app = crate::app::tests::phase_c_support::setup_app();
        app.settings.remember_favorite_view_state = true;
        let path = app.tmp.path().join("b").join("mount-rating.jpg");
        let key = seed_rating_boundary_page(&mut app, &path, 3);
        let main = app.viewer_context_main();
        let a = crate::settings::FavoriteEntry::new("main".into(), app.tmp.path().join("a"));
        let b = crate::settings::FavoriteEntry::new("other".into(), app.tmp.path().join("b"));
        let a_filter = [false, false, false, true, false, false];
        let b_filter = [true; 6];
        app.settings.rating_filter = a_filter;
        let mut a_state = crate::settings::FavoriteViewState::from_settings(&app.settings);
        a_state.grid_cols = 4;
        let mut b_state = a_state.clone();
        b_state.grid_cols = 7;
        app.favorite_view_states.insert(a.id, a_state);
        app.favorite_view_states.insert(b.id, b_state);
        app.settings.favorites.extend([a.clone(), b.clone()]);
        app.current_folder = Some(a.path.clone());
        app.transition_favorite_view_for_path(Some(&a.path));
        app.fullscreen_idx = Some(0);
        app.viewer_presentation = ViewerPresentation::Fullscreen;
        let other = app
            .build_viewer_context("rating_mount_other", |owner, _| {
                seed_rating_boundary_page(owner, &path, 3);
                owner.current_folder = Some(b.path.clone());
                owner.transition_favorite_view_for_path(Some(&b.path));
                owner.rating_filter_suppressed_at = Some((b.path.clone(), a_filter));
                // F12 physical readers keep their own membership despite App filters.
                owner.navigation_scope = ViewerNavigationScope::DetachedPhysical;
                owner.fullscreen_idx = Some(0);
                BuildOutcome::Commit
            })
            .unwrap();
        let trace = RatingBoundaryTrace::start();
        let write_generation = app.rating_session_write_generation;
        app.write_user_rating_shared_deferred(&key, 4, None)
            .unwrap();
        app.with_viewer_context(other, |owner| {
            assert_eq!(owner.projected_viewer_context_id(), other);
            assert_eq!(owner.effective_rating_filter(), b_filter);
            assert_eq!(owner.rating_cache.get(&0), Some(&4));
            assert_eq!(owner.visible_indices, vec![0]);
            assert_eq!(owner.fullscreen_idx, Some(0));
            let parked_main = owner.viewer_contexts.table.at_rest(main).unwrap();
            assert!(parked_main.visible_indices.is_empty());
            assert_eq!(parked_main.fullscreen_idx, None);
            assert_eq!(parked_main.rating_cache.get(&0), Some(&4));
            owner
                .write_user_rating_shared_deferred(&key, 3, None)
                .unwrap();
        })
        .unwrap();
        assert_eq!(app.projected_viewer_context_id(), main);
        assert_eq!(app.effective_rating_filter(), a_filter);
        assert_eq!(app.rating_cache.get(&0), Some(&3));
        assert_eq!(app.visible_indices, vec![0]);
        assert_eq!(app.fullscreen_idx, None);
        assert_eq!(app.rating_session_write_generation, write_generation + 2);
        let events = trace.take();
        assert_eq!(
            events
                .iter()
                .map(|event| (event.boundary, event.owner, event.effective_filter))
                .collect::<Vec<_>>(),
            vec![
                ("mount_outgoing", main, a_filter),
                ("mount_incoming", other, b_filter),
                ("return_outgoing", other, b_filter),
                ("return_incoming", main, a_filter),
            ]
        );
        assert!(
            events
                .iter()
                .all(|event| event.seen_after == event.write_generation)
        );
        assert_eq!(
            events
                .iter()
                .map(|event| event.grid_cols)
                .collect::<Vec<_>>(),
            [4, 7, 7, 4]
        );
        let publications = app.rating_publication_count;
        app.with_viewer_context(other, |_| ()).unwrap();
        assert_eq!(app.rating_publication_count, publications);
        assert_eq!(app.rating_session_write_generation, write_generation + 2);
    }

    #[cfg(windows)]
    #[test]
    fn rating_boundary_build_commit_abort_and_promote_publish_only_finished_owner() {
        let mut app = crate::app::tests::phase_c_support::setup_app();
        let path = app.tmp.path().join("build-rating.jpg");
        let key = seed_rating_boundary_page(&mut app, &path, 3);
        let main = app.viewer_context_main();
        app.write_user_rating_shared_deferred(&key, 4, None)
            .unwrap();
        let trace = RatingBoundaryTrace::start();
        let built = app
            .build_viewer_context("rating_boundary_commit", |owner, _| {
                assert_eq!(owner.rating_publication_count, 1);
                seed_rating_boundary_page(owner, &path, 4);
                owner
                    .write_user_rating_shared_deferred(&key, 5, None)
                    .unwrap();
                assert_eq!(owner.rating_publication_count, 1);
                BuildOutcome::Commit
            })
            .unwrap();
        assert_eq!(app.rating_cache.get(&0), Some(&5));
        let events = trace.take();
        assert_eq!(
            events
                .iter()
                .map(|event| (event.boundary, event.owner))
                .collect::<Vec<_>>(),
            [("build_outgoing", main), ("commit_incoming", main)]
        );
        // The new payload did not consume the generation before its first mount.
        assert_eq!(
            app.viewer_contexts
                .table
                .at_rest(built)
                .unwrap()
                .rating_session_write_seen_generation,
            0
        );
        app.with_viewer_context(built, |owner| {
            assert_eq!(owner.rating_cache.get(&0), Some(&5));
        })
        .unwrap();
        trace.take();
        assert_eq!(
            app.build_viewer_context("rating_boundary_abort", |owner, _| {
                owner
                    .write_user_rating_shared_deferred(&key, 2, None)
                    .unwrap();
                BuildOutcome::Abort("test")
            }),
            None
        );
        assert_eq!(app.rating_cache.get(&0), Some(&2));
        let events = trace.take();
        assert_eq!(
            events
                .iter()
                .map(|event| (event.boundary, event.owner))
                .collect::<Vec<_>>(),
            [("build_outgoing", main), ("abort_incoming", main)]
        );
        assert_eq!(
            app.stash_mounted_and_start_fresh("rating_boundary_promote"),
            main
        );
        let fresh = app.viewer_context_main();
        let events = trace.take();
        assert_eq!(
            events
                .iter()
                .map(|event| (event.boundary, event.owner))
                .collect::<Vec<_>>(),
            [("promote_outgoing", main), ("promote_incoming", fresh)]
        );
        assert!(
            events
                .iter()
                .all(|event| event.seen_after == event.write_generation)
        );
        app.retire_context(built, "rating_boundary_retire", |_| ())
            .unwrap();
        assert!(
            trace.take().is_empty(),
            "retirement must not reopen a dead reader"
        );
    }

    #[cfg(windows)]
    #[test]
    fn rating_boundary_materialized_fork_keeps_explicit_page_when_source_cursor_moves() {
        let mut app = crate::app::tests::phase_c_support::setup_app();
        let root = app.tmp.path().join("fork-pages");
        let first = root.join("first.jpg");
        let second = root.join("second.jpg");
        let key = seed_rating_boundary_page(&mut app, &first, 3);
        app.items.push(GridItem::Image(second));
        app.thumbnails.push(ThumbnailState::Pending);
        app.image_metas.push(None);
        app.visible_indices.push(1);
        app.rating_cache.insert(1, 3);
        let other_key = app.rating_path_key(1).unwrap();
        app.rating_db.as_ref().unwrap().set(&other_key, 3).unwrap();
        app.settings.rating_filter = [false, false, false, true, false, false];
        app.current_folder = Some(root.clone());
        app.write_user_rating_shared_deferred(&key, 4, None)
            .unwrap();
        let main = app.viewer_context_main();
        let trace = RatingBoundaryTrace::start();
        let forked = app.fork_materialized_still_context(&root, 0);
        assert_eq!(app.projected_viewer_context_id(), main);
        assert_eq!(app.selected, Some(1));
        assert_eq!(app.visible_indices, vec![1]);
        let fork = app.viewer_contexts.table.at_rest(forked).unwrap();
        assert_eq!(fork.selected, Some(0));
        assert_eq!(fork.items[0], GridItem::Image(first));
        assert_eq!(fork.visible_indices, vec![0, 1]);
        let events = trace.take();
        assert_eq!(events[0].boundary, "fork_outgoing");
        assert_eq!(events[1].boundary, "fork_incoming");
        assert_eq!(events[0].owner, main);
        assert_eq!(events[1].owner, main);
        assert!(
            events
                .iter()
                .all(|event| event.seen_after == event.write_generation)
        );
    }

    #[cfg(windows)]
    #[test]
    fn analysis_histogram_heap_cache_remains_with_its_context_across_mounts() {
        let mut app = crate::app::setup_app_for_test();
        app.analysis_hist_cache = Some(Box::new((
            1.0,
            egui::Vec2::ZERO,
            0,
            [11; 360],
            [12; 256],
            [13; 256],
        )));
        let main_pointer = app.analysis_hist_cache.as_deref().unwrap() as *const _ as usize;
        let sibling = app.build_window_context_for_test(91344, |owner| {
            assert!(owner.analysis_hist_cache.is_none());
            owner.analysis_hist_cache = Some(Box::new((
                2.0,
                egui::vec2(3.0, 4.0),
                1,
                [21; 360],
                [22; 256],
                [23; 256],
            )));
        });
        app.with_viewer_context(sibling, |owner| {
            let cache = owner.analysis_hist_cache.as_deref_mut().unwrap();
            assert_ne!(cache as *const _ as usize, main_pointer);
            assert_eq!((cache.0, cache.1, cache.2), (2.0, egui::vec2(3.0, 4.0), 1));
            assert_eq!((cache.3[359], cache.4[255], cache.5[255]), (21, 22, 23));
            cache.3[0] = 31;
        })
        .unwrap();
        let main = app.analysis_hist_cache.as_deref().unwrap();
        assert_eq!(main as *const _ as usize, main_pointer);
        assert_eq!((main.0, main.1, main.2), (1.0, egui::Vec2::ZERO, 0));
        assert_eq!((main.3[0], main.4[0], main.5[0]), (11, 12, 13));
        app.with_viewer_context(sibling, |owner| {
            assert_eq!(owner.analysis_hist_cache.as_deref().unwrap().3[0], 31);
        })
        .unwrap();
        assert_eq!(
            app.analysis_hist_cache.as_deref().unwrap() as *const _ as usize,
            main_pointer
        );
    }

    #[cfg(windows)]
    #[test]
    fn page_turn_operations_remain_with_their_context_across_mounts() {
        let mut app = crate::app::setup_app_for_test();
        let main = app.page_turn_destination();
        app.page_turn_presentation.seed_operation_for_test(main, 41);
        let sibling = app.build_window_context_for_test(91341, |_| {});
        // Delivery happens after the new context acquires its real window binding.
        app.with_viewer_context(sibling, |owner| {
            assert_eq!(owner.page_turn_presentation.operation_for_test(), None);
            let destination = owner.page_turn_destination();
            owner
                .page_turn_presentation
                .seed_operation_for_test(destination, 72);
        })
        .unwrap();
        assert_eq!(
            app.page_turn_presentation.operation_for_test(),
            Some((main, 41))
        );
        app.with_viewer_context(sibling, |owner| {
            let destination = owner.page_turn_destination();
            assert_eq!(destination.context, sibling);
            assert_eq!(destination.window, Some(91341));
            assert_eq!(
                owner.page_turn_presentation.operation_for_test(),
                Some((destination, 72))
            );
            // Global diagnostics and a sibling's new operation cannot invalidate main's facts.
            owner.input_seq += 1;
            owner
                .page_turn_presentation
                .seed_operation_for_test(destination, 73);
            owner.pause_mounted_background_work_keep_current_frame();
            assert_eq!(
                owner.page_turn_presentation.operation_for_test(),
                Some((destination, 73))
            );
        })
        .unwrap();
        assert_eq!(
            app.page_turn_presentation.operation_for_test(),
            Some((main, 41))
        );
        app.with_viewer_context(sibling, |owner| {
            assert_eq!(
                owner.page_turn_presentation.operation_for_test().unwrap().1,
                73
            );
        })
        .unwrap();
        assert_eq!(
            app.page_turn_presentation.operation_for_test(),
            Some((main, 41))
        );
    }

    #[cfg(windows)]
    #[test]
    fn page_turn_animation_overflow_f12_updates_one_setting_without_reloading_content() {
        let mut app = crate::app::setup_app_for_test();
        app.items = vec![GridItem::Image(app.tmp.path().join("main.png"))];
        app.fullscreen_idx = Some(0);
        app.adopt_page_turn_animation_settings(true, 400);
        app.settings.save();
        let main = app.page_turn_destination();
        app.page_turn_presentation.seed_operation_for_test(main, 41);
        let sibling = app.build_window_context_for_test(91343, |owner| {
            owner.items = vec![GridItem::Image(PathBuf::from("c:/fixtures/sibling.png"))];
            owner.fullscreen_idx = Some(0);
        });
        app.with_viewer_context(sibling, |owner| {
            let destination = owner.page_turn_destination();
            owner
                .page_turn_presentation
                .seed_operation_for_test(destination, 73);
            owner.fs_overflow_panel_state = FsOverflowPanelState::Root;
            let before_items = owner.items.clone();
            let before_input = owner.input_seq;
            owner.toggle_page_turn_animation();
            let stored = crate::settings_db::with_db_result(|db| db.load_into_settings()).unwrap();
            assert!(!stored.page_turn_animation_enabled);
            assert_eq!(stored.page_turn_animation_duration_ms, 400);
            assert!(!owner.settings.page_turn_animation_enabled);
            assert_eq!(owner.settings.page_turn_animation_duration_ms, 400);
            assert_eq!(owner.page_turn_presentation.operation_for_test(), None);
            assert_eq!(owner.items, before_items);
            assert_eq!(owner.fullscreen_idx, Some(0));
            assert_eq!(owner.input_seq, before_input);
            assert_eq!(owner.fs_overflow_panel_state, FsOverflowPanelState::Root);
        })
        .unwrap();
        assert!(!app.settings.page_turn_animation_enabled);
        assert_eq!(app.page_turn_presentation.operation_for_test(), None);
        assert_eq!(app.page_turn_destination(), main);
        assert_eq!(app.fullscreen_idx, Some(0));
        let mut preferences = app.settings.clone();
        preferences.page_turn_animation_enabled = true;
        app.install_preferences_settings(preferences);
        app.with_viewer_context(sibling, |owner| {
            assert!(owner.settings.page_turn_animation_enabled);
            assert_eq!(owner.items.len(), 1);
            assert_eq!(owner.fullscreen_idx, Some(0));
        })
        .unwrap();
    }

    #[cfg(windows)]
    #[test]
    fn page_turn_setting_disable_clears_only_presentations_in_all_contexts() {
        let mut app = crate::app::setup_app_for_test();
        app.settings.page_turn_animation_enabled = true;
        let main = app.page_turn_destination();
        app.page_turn_presentation.seed_operation_for_test(main, 41);
        let sibling = app.build_window_context_for_test(91342, |owner| {
            owner.items = vec![GridItem::Image(PathBuf::from("c:/fixtures/retained.jpg"))];
        });
        app.with_viewer_context(sibling, |owner| {
            let destination = owner.page_turn_destination();
            owner
                .page_turn_presentation
                .seed_operation_for_test(destination, 72);
        })
        .unwrap();
        let mut edited = app.settings.clone();
        edited.page_turn_animation_enabled = false;
        app.install_preferences_settings(edited);
        assert_eq!(app.page_turn_presentation.operation_for_test(), None);
        app.with_viewer_context(sibling, |owner| {
            assert_eq!(owner.page_turn_presentation.operation_for_test(), None);
            assert_eq!(owner.items.len(), 1);
            assert_eq!(
                owner.items[0],
                GridItem::Image(PathBuf::from("c:/fixtures/retained.jpg"))
            );
        })
        .unwrap();
        assert_eq!(app.page_turn_destination(), main);
    }

    #[cfg(windows)]
    #[test]
    fn g1_recipe_mount_fork_close_preserves_sibling_items_workers_and_cache() {
        use super::super::book_materialization::{BookScope, ExtractedSurface, ProjectionRecipe};
        let mut app = crate::app::setup_app_for_test();
        let recipe = ProjectionRecipe::ExtractedResult(ExtractedSurface::Collection);
        app.projection_recipe = recipe.clone();
        app.spread_mode = crate::settings::SpreadMode::Rtl;
        app.items = vec![GridItem::Image(PathBuf::from("C:/result/a.png"))];
        let generation = app.items_generation;
        let cancel = app.cancel_token.clone();
        let sibling = app.build_window_context_for_test(9914, |owner| {
            owner.projection_recipe =
                ProjectionRecipe::Book(BookScope::Document(PathBuf::from("C:/sibling.pdf")));
            owner.items = vec![GridItem::PdfPage {
                pdf_path: PathBuf::from("C:/sibling.pdf"),
                page_num: 80,
                content_type: None,
            }];
            owner.fs_early_dims.insert(0, [123, 456]);
            owner.reload_queue = Some(Arc::new((Mutex::new(Vec::new()), Condvar::new())));
        });
        let sibling_before = app
            .with_viewer_context(sibling, |owner| {
                (
                    owner.projection_recipe.clone(),
                    owner.items.clone(),
                    owner.items_generation,
                    owner.cancel_token.clone(),
                    owner.reload_queue.clone(),
                    owner.fs_early_dims.get(&0).copied(),
                )
            })
            .unwrap();
        let mut fork = app.split_current_context_preserving_main_grid();
        assert_eq!(fork.projection_recipe, recipe);
        app.swap_viewer_context_bundle(&mut fork);
        app.begin_extracted_result_view();
        app.swap_viewer_context_bundle(&mut fork);
        drop(fork);
        assert_eq!(app.projection_recipe, recipe);
        assert_eq!(app.spread_mode, crate::settings::SpreadMode::Rtl);
        assert_eq!(app.items_generation, generation);
        assert!(Arc::ptr_eq(&app.cancel_token, &cancel));
        assert!(!cancel.load(Ordering::Relaxed));
        app.with_viewer_context(sibling, |owner| {
            assert_eq!(owner.projection_recipe, sibling_before.0);
            assert_eq!(owner.items, sibling_before.1);
            assert_eq!(owner.items_generation, sibling_before.2);
            assert!(Arc::ptr_eq(&owner.cancel_token, &sibling_before.3));
            assert!(!owner.cancel_token.load(Ordering::Relaxed));
            assert!(Arc::ptr_eq(
                owner.reload_queue.as_ref().unwrap(),
                sibling_before.4.as_ref().unwrap()
            ));
            assert_eq!(
                owner.reload_queue.as_ref().unwrap().0.lock().unwrap().len(),
                0
            );
            assert_eq!(owner.fs_early_dims.get(&0).copied(), sibling_before.5);
            assert_eq!(owner.fs_early_dims.len(), 1);
        })
        .unwrap();
    }

    #[cfg(windows)]
    #[test]
    fn book_root_rename_preserves_mounted_and_parked_context_payloads() {
        use super::super::spread_shift_owner::ViewerShift;
        use crate::spread_shift::{
            ShiftPersistence, ShiftResolution, SpreadShiftPageRef, SpreadShiftState,
        };
        let mut app = crate::app::setup_app_for_test();
        let seed = |owner: &mut App, root: &str| {
            owner.current_folder = Some(PathBuf::from(root));
            owner.address = root.into();
            owner.archive_source_override = Some(PathBuf::from(root).join("Book.rar"));
            owner.items = (0..4)
                .map(|idx| GridItem::Image(PathBuf::from(format!("{root}/{idx}.jpg"))))
                .collect();
            owner.thumbnails = vec![ThumbnailState::Pending; 4];
            owner.visible_indices = vec![0, 1, 2, 3];
            owner.fullscreen_idx = Some(3);
            owner.page_identity_index = Arc::new(
                super::super::file_type_resume::PreparedPageIdentityIndex::prepare(
                    &owner.items,
                    None,
                    Default::default(),
                ),
            );
            owner.spread_shift.scope = Some(crate::book_scope::BookScopeKey::folder(root).unwrap());
            owner.spread_shift.state = ViewerShift::Book(SpreadShiftState::Adjusted {
                anchor_ref: SpreadShiftPageRef::RelativePath("3.jpg".into()),
                resolution: ShiftResolution::Resolved(3),
                persistence: ShiftPersistence::SessionOnly,
            });
            let entries = ["cover.jpg", "chapter/1.jpg", "chapter/2.jpg"].map(|entry_name| {
                crate::zip_loader::ZipImageEntry {
                    entry_name: entry_name.into(),
                    uncompressed_size: 1,
                    mtime: 0,
                    container_ancestry: Vec::new(),
                }
            });
            let mut nav =
                crate::zip_tree::ZipNavState::new(Arc::new(crate::zip_tree::ZipTree::build(
                    PathBuf::from(root).join("Book.zip"),
                    entries.into(),
                )));
            nav.enter("chapter/");
            owner.zip_nav = Some(nav);
            owner.viewer_navigation_caches.all_items_are_images(
                owner.items_generation,
                crate::settings::ReadingFlow::Paged,
                &owner.items,
            );
            owner.fs_cache.insert(0, FsCacheEntry::Failed);
            let (_tx, rx) = mpsc::channel();
            owner.fs_pending.insert(
                1,
                FsPendingValue::new(
                    Arc::new(AtomicBool::new(false)),
                    rx,
                    owner.input_seq,
                    FsLoadPurpose::Prefetch,
                ),
            );
            owner.fs_upload_backlog.push(FsUploadResult::new(
                2,
                FsLoadResult::Failed,
                owner.input_seq,
                FsLoadPurpose::Prefetch,
            ));
        };
        // Both an accepted main book and a matching F12 book retain their old source.
        seed(&mut app, "C:/Books/Book");
        let main = app.viewer_context_main();
        let matching =
            app.build_window_context_for_test(9821, |owner| seed(owner, "C:/Books/Book"));
        let sibling = app.build_window_context_for_test(9822, |owner| seed(owner, "D:/Books/Book"));
        macro_rules! payload {
            ($owner:expr) => {{
                let owner = $owner;
                let nav = owner.zip_nav.as_ref().unwrap();
                let pending = owner.fs_pending.get(&1).unwrap();
                (
                    (
                        owner.current_folder.clone(),
                        owner.address.clone(),
                        owner.archive_source_override.clone(),
                        owner.items.clone(),
                        Arc::as_ptr(&owner.page_identity_index) as usize,
                        owner.spread_shift.clone(),
                    ),
                    (
                        nav.tree.zip_path.clone(),
                        Arc::as_ptr(&nav.tree) as usize,
                        nav.current().to_vec(),
                        nav.current_parent_zipdir_prefix(),
                        nav.at_root(),
                    ),
                    (
                        owner.items_generation,
                        owner.visible_indices.clone(),
                        owner.fullscreen_idx,
                        Arc::as_ptr(&owner.cancel_token) as usize,
                        owner.cancel_token.load(Ordering::Relaxed),
                        owner.viewer_navigation_caches.has_entries_for_test(),
                    ),
                    (
                        owner.fs_pending.len(),
                        Arc::as_ptr(&pending.cancel) as usize,
                        pending.cancel.load(Ordering::Relaxed),
                        pending.load_seq,
                        owner.fs_cache.len(),
                        matches!(owner.fs_cache.get(&0), Some(FsCacheEntry::Failed)),
                        owner.fs_upload_backlog.current_items_generation(),
                        owner
                            .fs_upload_backlog
                            .iter()
                            .map(|upload| {
                                (
                                    upload.idx,
                                    upload.load_seq,
                                    matches!(upload.result, FsLoadResult::Failed),
                                )
                            })
                            .collect::<Vec<_>>(),
                    ),
                )
            }};
        }
        let main_before = payload!(&app);
        let matching_before = payload!(app.viewer_contexts.table.at_rest(matching).unwrap());
        let sibling_before = payload!(app.viewer_contexts.table.at_rest(sibling).unwrap());
        let source = Path::new("c:/books/book");
        let target = Path::new("C:/Books/Renamed");
        let mappings = [crate::books::BookPathMapping {
            from: source.join("3.jpg"),
            to: target.join("renamed.jpg"),
        }];
        app.migrate_at_rest_spread_shifts(&mappings, Some((source, target)));
        assert_eq!(payload!(&app), main_before);
        assert_eq!(
            payload!(app.viewer_contexts.table.at_rest(matching).unwrap()),
            matching_before,
        );
        assert_eq!(
            payload!(app.viewer_contexts.table.at_rest(sibling).unwrap()),
            sibling_before,
        );
        assert_eq!(app.projected_viewer_context_id(), main);
        // While F12 owns the mounted projection, the parked main payload stays intact too.
        app.with_window_viewer_context(9821, |owner| {
            assert_eq!(payload!(&*owner), matching_before);
            owner.migrate_at_rest_spread_shifts(&mappings, Some((source, target)));
            assert_eq!(payload!(&*owner), matching_before);
            assert_eq!(
                payload!(owner.viewer_contexts.table.at_rest(main).unwrap()),
                main_before,
            );
            assert_eq!(
                payload!(owner.viewer_contexts.table.at_rest(sibling).unwrap()),
                sibling_before,
            );
        })
        .unwrap();
        assert_eq!(payload!(&app), main_before);
        assert_eq!(
            payload!(app.viewer_contexts.table.at_rest(matching).unwrap()),
            matching_before,
        );
        assert_eq!(app.projected_viewer_context_id(), main);
    }

    #[cfg(windows)]
    #[test]
    fn audio_folder_cache_maintenance_preserves_sibling_and_other_drive_terminals() {
        let mut app = crate::app::setup_app_for_test();
        let seed = |owner: &mut App| {
            owner.items = [
                r"C:\Music\empty.mp3",
                r"D:\Music\empty.mp3",
                r"C:\Music\Child\empty.mp3",
                r"C:\Other\empty.mp3",
            ]
            .map(|path| GridItem::Audio(PathBuf::from(path)))
            .into();
            owner.thumbnails = vec![ThumbnailState::NoArt; 4];
        };
        seed(&mut app);
        let parked_id = app.build_window_context_for_test(9813, seed);
        assert_eq!(
            app.invalidate_audio_art_terminals_after_catalog_maintenance(Some(Path::new(
                "c:/MUSIC"
            ))),
            2
        );
        assert!(matches!(app.thumbnails[0], ThumbnailState::Evicted));
        assert!(
            app.thumbnails[1..]
                .iter()
                .all(|state| matches!(state, ThumbnailState::NoArt))
        );
        let parked = app.viewer_contexts.table.at_rest(parked_id).unwrap();
        assert!(matches!(parked.thumbnails[0], ThumbnailState::Evicted));
        assert!(
            parked.thumbnails[1..]
                .iter()
                .all(|state| matches!(state, ThumbnailState::NoArt))
        );
        assert_eq!(
            app.invalidate_audio_art_terminals_after_catalog_maintenance(Some(Path::new(
                "c:/Music"
            ))),
            0
        );
    }

    #[cfg(windows)]
    #[test]
    fn audio_cache_maintenance_reopens_terminals_across_contexts_without_cancel_or_pixel_loss() {
        let ctx = egui::Context::default();
        let mut app = crate::app::setup_app_for_test();
        let seed = |owner: &mut App, prefix: &str| {
            owner.items = vec![
                GridItem::Audio(PathBuf::from(format!("{prefix}/empty.mp3"))),
                GridItem::Audio(PathBuf::from(format!("{prefix}/failed.mp3"))),
                GridItem::Audio(PathBuf::from(format!("{prefix}/loaded.mp3"))),
                GridItem::Audio(PathBuf::from(format!("{prefix}/pending.mp3"))),
                GridItem::Image(PathBuf::from(format!("{prefix}/failed.jpg"))),
                GridItem::Video(PathBuf::from(format!("{prefix}/loaded.mp4"))),
            ];
            let pixels = ThumbnailState::Loaded {
                tex: ctx.load_texture(
                    prefix,
                    egui::ColorImage::filled([1, 1], egui::Color32::WHITE),
                    egui::TextureOptions::LINEAR,
                ),
                origin: crate::thumb_loader::ThumbLoadOrigin::SourceIntrinsic,
                from_edit_preview: false,
                rendered_at_px: 1,
                source_dims: Some((13, 17)),
                layout_dims: None,
            };
            owner.thumbnails = vec![
                ThumbnailState::NoArt,
                ThumbnailState::Failed,
                pixels.clone(),
                ThumbnailState::Pending,
                ThumbnailState::Failed,
                pixels,
            ];
            owner.image_metas = vec![None; 6];
            owner.requested.clear();
            for idx in 0..4 {
                owner.requested.insert(idx, false);
            }
            for idx in [0, 1, 2, 3] {
                owner.requested.mark_finalized_for_test(idx);
            }
            owner.heavy_io_queue = Some(Arc::new((Mutex::new(Vec::new()), Condvar::new())));
        };
        seed(&mut app, "c:/main");
        let main = app.viewer_context_main();
        let first = app.build_window_context_for_test(9811, |owner| seed(owner, "c:/first"));
        let second = app.build_window_context_for_test(9812, |owner| seed(owner, "c:/second"));
        let snapshot = |owner: &App| {
            let ThumbnailState::Loaded { tex, .. } = &owner.thumbnails[2] else {
                panic!("expected pixels")
            };
            (
                owner.items_generation,
                Arc::clone(&owner.cancel_token),
                tex.id(),
                owner.cancel_token.load(Ordering::Relaxed),
            )
        };
        let current_snapshot = snapshot(&app);
        let parked_snapshots = [first, second].map(|id| {
            let owner = app.viewer_contexts.table.at_rest(id).unwrap();
            let ThumbnailState::Loaded { tex, .. } = &owner.thumbnails[2] else {
                panic!("expected pixels")
            };
            (
                id,
                owner.items_generation,
                Arc::clone(&owner.cancel_token),
                tex.id(),
                owner.cancel_token.load(Ordering::Relaxed),
            )
        });
        assert_eq!(app.projected_viewer_context_id(), main);
        assert_eq!(
            app.invalidate_audio_art_terminals_after_catalog_maintenance(None),
            6
        );
        let assert_rows = |states: &[ThumbnailState],
                           requested: &ThumbnailRequests,
                           texture: egui::TextureId| {
            assert!(matches!(states[0], ThumbnailState::Evicted));
            assert!(matches!(states[1], ThumbnailState::Evicted));
            assert!(
                matches!(&states[2],ThumbnailState::Loaded{tex,source_dims:Some((13,17)),..} if tex.id()==texture)
            );
            assert!(matches!(states[3], ThumbnailState::Pending));
            assert!(matches!(states[4], ThumbnailState::Failed));
            assert!(matches!(states[5], ThumbnailState::Loaded { .. }));
            assert!(!requested.contains_key(&0) && !requested.contains_key(&1));
            assert!(requested.contains_key(&2) && requested.contains_key(&3));
            assert!(requested.finalized_image_wait(2) && requested.finalized_image_wait(3));
        };
        assert_rows(&app.thumbnails, &app.requested, current_snapshot.2);
        assert_eq!(app.items_generation, current_snapshot.0);
        assert!(Arc::ptr_eq(&app.cancel_token, &current_snapshot.1));
        assert_eq!(app.cancel_token.load(Ordering::Relaxed), current_snapshot.3);
        for (id, generation, token, texture, canceled) in parked_snapshots {
            let owner = app.viewer_contexts.table.at_rest(id).unwrap();
            assert_rows(&owner.thumbnails, &owner.requested, texture);
            assert_eq!(owner.items_generation, generation);
            assert!(Arc::ptr_eq(&owner.cancel_token, &token));
            assert_eq!(owner.cancel_token.load(Ordering::Relaxed), canceled);
        }
        assert_eq!(app.projected_viewer_context_id(), main);
        assert_eq!(
            app.invalidate_audio_art_terminals_after_catalog_maintenance(None),
            0
        );
        // The real request handler admits one new lookup, then its existing
        // requested owner prevents duplicate enqueue on the following update.
        app.enqueue_priority_thumbnail(0);
        app.enqueue_priority_thumbnail(0);
        assert_eq!(
            app.heavy_io_queue.as_ref().unwrap().0.lock().unwrap().len(),
            1
        );
        assert!(app.requested.contains_key(&0));
        send_thumbnail_for_test!(
            app,
            ThumbMsg {
                request_id: crate::thumb_loader::ThumbnailRequestId::Standalone,
                idx: 0,
                payload: crate::thumb_loader::ThumbMsgPayload::NoArt,
                input_seq: app.input_seq,
                items_gen: app.items_generation,
            }
        )
        .unwrap();
        app.poll_thumbnails(&ctx, ThumbnailConsumptionPolicy::Grid);
        assert!(matches!(app.thumbnails[0], ThumbnailState::NoArt));
        app.enqueue_priority_thumbnail(0);
        assert_eq!(
            app.heavy_io_queue.as_ref().unwrap().0.lock().unwrap().len(),
            1
        );
        assert!(!app.requested.contains_key(&0));
    }

    #[cfg(windows)]
    fn still_seek_committed_center(center: usize) -> crate::ui_fullscreen::StillSeekGesture {
        crate::ui_fullscreen::StillSeekGesture::StripCommitted {
            layout_center_pos: center,
            page_pos_at_commit: 3,
        }
    }

    #[cfg(windows)]
    fn video_zoom_state(
        wheel_notches: f32,
        drag: [f32; 2],
    ) -> crate::video::zoom_view::VideoZoomState {
        let source = crate::video::zoom_view::VideoZoomSourceGeometry::new(
            1920,
            1080,
            1,
            1,
            crate::video::display_metadata::VideoOrientation::IDENTITY,
        );
        let region = [1920.0, 1080.0];
        let mut state = crate::video::zoom_view::VideoZoomState::new();
        assert!(state.apply_wheel(
            wheel_notches * 120.0,
            [region[0] * 0.5, region[1] * 0.5],
            region,
            source,
        ));
        assert!(state.apply_drag(drag, region, source));
        state
    }

    #[cfg(windows)]
    fn similar_navigation_purpose(trace: SimilarMoveTrace, suffix: &str) -> FsNavigationPurpose {
        FsNavigationPurpose::SimilarBookVisit(SimilarBookNavigationIntent {
            origin: SimilarBookLocation {
                container_key: "c:/trace/origin".to_owned(),
                page: crate::snapshot::SnapshotTarget::Fs(PathBuf::from("c:/trace/origin/001.jpg")),
            },
            destination: SimilarBookLocation {
                container_key: format!("c:/trace/{suffix}"),
                page: crate::snapshot::SnapshotTarget::Fs(PathBuf::from(format!(
                    "c:/trace/{suffix}/001.jpg"
                ))),
            },
            diagnostic_trace: Some(trace),
        })
    }

    #[cfg(windows)]
    fn similar_trace_owners(
        accepted_generation: u64,
        pending_trace: SimilarMoveTrace,
        navigation_trace: SimilarMoveTrace,
        cancel: Arc<AtomicBool>,
    ) -> (FolderPaneOpenPending, FsHoldover) {
        let (_tx, rx) = mpsc::channel();
        (
            FolderPaneOpenPending {
                navigation: None,
                epub_restore: None,
                path: PathBuf::from("c:/trace/pending"),
                cancel,
                rx,
                purpose: FolderOpenScanPurpose::RequiredFullscreenTarget {
                    target: crate::snapshot::SnapshotTarget::Fs(PathBuf::from(
                        "c:/trace/pending/001.jpg",
                    )),
                    history_trigger: HistoryTrigger::UserChosen,
                    navigation_purpose: similar_navigation_purpose(pending_trace, "pending"),
                },
            },
            FsHoldover::NavigationSequence(FsNavigationSequence {
                previous: None,
                chrome: FsNavigationChromeContinuation::None,
                purpose: similar_navigation_purpose(navigation_trace, "navigation"),
                opened_at: std::time::Instant::now(),
                target: FsNavigationSequenceTarget::FolderItems {
                    accepted_generation,
                },
            }),
        )
    }

    #[cfg(windows)]
    #[test]
    fn similar_move_p2_trace_owners_terminal_when_mounted_context_is_parked() {
        let target = crate::snapshot::SnapshotTarget::Fs(PathBuf::from("c:/trace/pending/001.jpg"));
        let pending_trace = SimilarMoveTrace::new(SimilarMoveSource::ItemButton, Some(&target));
        let pending_id = pending_trace.id;
        let navigation_trace =
            SimilarMoveTrace::new(SimilarMoveSource::HistoryButton, Some(&target));
        let navigation_id = navigation_trace.id;
        let cancel = Arc::new(AtomicBool::new(false));
        let mut app = crate::app::setup_app_for_test();
        let accepted_generation = app.items_generation;
        let (pending, holdover) = similar_trace_owners(
            accepted_generation,
            pending_trace,
            navigation_trace,
            Arc::clone(&cancel),
        );
        app.folder_pane_open_pending = Some(pending);
        app.fs_holdover_tex = Some(holdover);

        app.pause_mounted_background_work_keep_current_frame();

        assert!(cancel.load(Ordering::Relaxed));
        assert!(app.folder_pane_open_pending.is_none());
        assert!(app.fs_holdover_tex.is_none());
        assert_eq!(
            SimilarMoveTrace::terminal_reasons_for_test(pending_id),
            vec!["context_parked"]
        );
        assert_eq!(
            SimilarMoveTrace::terminal_reasons_for_test(navigation_id),
            vec!["context_parked"]
        );
    }

    #[cfg(windows)]
    #[test]
    fn parking_mounted_context_cancels_its_raw_thumbnail_ticket() {
        let mut app = crate::app::setup_app_for_test();
        let executor = crate::raw::RawDevelopExecutor::new(1).unwrap();
        let (started_tx, started_rx) = std::sync::mpsc::channel();
        let (release_tx, release_rx) = std::sync::mpsc::channel();
        let _blocker = executor.block_one_slot_for_test(started_tx, release_rx);
        started_rx
            .recv_timeout(std::time::Duration::from_secs(5))
            .unwrap();
        let (result_tx, _result_rx) = std::sync::mpsc::channel();
        let ticket = executor.submit_thumbnail_half(
            crate::raw::RawOwnedSource::Path(PathBuf::from("vendor/raw-samples/1018.cr2")),
            crate::raw::RawPriority::Normal,
            result_tx,
        );
        let (tx, rx) = std::sync::mpsc::channel();
        let done = Arc::new(AtomicUsize::new(0));
        let generation = app.items_generation;
        let pending = crate::thumb_loader::RawThumbPending::for_test(
            ticket,
            tx,
            Arc::clone(&done),
            generation,
        );
        {
            let mut map = app.raw_thumb_develop.lock().unwrap();
            map.set_items_generation(generation);
            map.insert(0, pending);
        }
        app.pause_mounted_background_work_keep_current_frame();
        assert!(
            rx.recv_timeout(std::time::Duration::from_secs(5))
                .unwrap()
                .is_canceled()
        );
        assert_eq!(done.load(Ordering::Relaxed), 1);
        assert!(app.raw_thumb_develop.lock().unwrap().is_empty());
        release_tx.send(()).unwrap();
    }

    #[cfg(windows)]
    #[test]
    fn collection_navigation_continuation_is_terminal_on_park() {
        use crate::app::collection_navigation::CollectionNavigationPending;
        let mut app = crate::app::setup_app_for_test();
        let address = app.address.clone();
        let rows = app.items.clone();
        let back = app.folder_nav_back_stack.clone();
        let forward = app.folder_nav_forward_stack.clone();
        app.top_level_grid_view
            .set_collection_navigation_pending(Some(
                CollectionNavigationPending::AwaitingOuterContinuation {
                    steps: 1,
                    fullscreen: false,
                    resume_slideshow: false,
                    native_toast: false,
                },
            ));
        assert!(ContextAsyncOwner::CollectionNavigation.is_pending(ContextRef::mounted(&app)));
        app.pause_mounted_background_work_keep_current_frame();
        assert!(!app.top_level_grid_view.collection_navigation_pending());
        assert!(
            ContextRef::mounted(&app)
                .pending_context_async_owners()
                .is_empty()
        );
        assert_eq!(app.address, address);
        assert_eq!(app.items, rows);
        assert_eq!(app.folder_nav_back_stack, back);
        assert_eq!(app.folder_nav_forward_stack, forward);
    }

    #[cfg(windows)]
    #[test]
    fn folder_dfs_request_is_terminal_on_park() {
        let mut app = crate::app::setup_app_for_test();
        let address = app.address.clone();
        let rows = app.items.clone();
        let back = app.folder_nav_back_stack.clone();
        let forward = app.folder_nav_forward_stack.clone();
        let (sender, receiver) = mpsc::channel();
        let cancel = Arc::new(AtomicBool::new(false));
        app.folder_nav_pending = Some(FolderNavPending {
            cancel: Arc::clone(&cancel),
            rx: receiver,
            forward: true,
            mode: FolderNavMode::Grid,
        });
        app.pending_folder_nav_steps = 2;
        assert!(ContextAsyncOwner::FolderNavigation.is_pending(ContextRef::mounted(&app)));

        app.pause_mounted_background_work_keep_current_frame();

        assert!(cancel.load(Ordering::Relaxed));
        assert!(app.folder_nav_pending.is_none());
        assert_eq!(app.pending_folder_nav_steps, 0);
        assert!(
            ContextRef::mounted(&app)
                .pending_context_async_owners()
                .is_empty()
        );
        assert_eq!(app.address, address);
        assert_eq!(app.items, rows);
        assert_eq!(app.folder_nav_back_stack, back);
        assert_eq!(app.folder_nav_forward_stack, forward);
        drop(sender);
    }

    #[cfg(windows)]
    #[test]
    fn rating_request_reaches_park_terminal() {
        let mut rating = crate::app::setup_app_for_test();
        let source = tempfile::tempdir().unwrap();
        std::fs::write(source.path().join("keep.jpg"), []).unwrap();
        rating.load_folder(source.path().to_path_buf());
        rating.enter_rating_view(5);
        assert!(rating.rating_view_pending.is_some());
        let rating_address = rating.address.clone();
        let rating_rows = rating.items.clone();
        rating.pause_mounted_background_work_keep_current_frame();
        assert!(rating.rating_view_pending.is_none());
        assert_eq!(rating.address, rating_address);
        assert_eq!(rating.items, rating_rows);
    }

    #[cfg(windows)]
    #[test]
    fn bookmark_request_reaches_park_terminal() {
        let mut bookmark = crate::app::setup_app_for_test();
        let source = tempfile::tempdir().unwrap();
        let address = bookmark.address.clone();
        let rows = bookmark.items.clone();
        bookmark.bookmark_open_pending = Some(crate::bookmark_browser::PendingBookmarkOpen::Media(
            crate::bookmark_browser::PendingMediaOpen {
                request_id: crate::bookmark_browser::BookmarkOpenRequestId(1),
                path: source.path().join("later.mp4"),
                pts_secs: 0.0,
                started_at: std::time::Instant::now(),
                last_wait: None,
            },
        ));
        assert!(ContextAsyncOwner::BookmarkOpen.is_pending(ContextRef::mounted(&bookmark)));
        bookmark.pause_mounted_background_work_keep_current_frame();
        assert!(bookmark.bookmark_open_pending.is_none());
        assert_eq!(bookmark.address, address);
        assert_eq!(bookmark.items, rows);
    }

    #[cfg(windows)]
    #[test]
    fn context_async_owner_registry_audits_bundle_pending_fields() {
        // This source audit forces an explicit registration or a documented cache-only
        // exception whenever a pending field is added to ViewerContextBundle.
        let source = include_str!("viewer_context_registry.rs").replace("\r\n", "\n");
        let bundle = source
            .split_once("pub(in crate::app) struct ViewerContextBundle {")
            .unwrap()
            .1
            .split_once("\n}\n")
            .unwrap()
            .0;
        let fields = bundle
            .lines()
            .filter_map(|line| line.trim().split_once(':').map(|(name, _)| name))
            .filter(|name| name.ends_with("pending"))
            .collect::<std::collections::BTreeSet<_>>();
        let registered = [
            "external_rescan_pending",
            "folder_nav_pending",
            "folder_pane_open_pending",
            "fs_pending",
            "rating_view_pending",
            "bookmark_open_pending",
            "pdf_enumerate_pending",
            "zip_enumerate_pending",
            "local_adjust_pending",
            "final_ai_pending",
            "comic_bake_pending",
            "erase_inpaint_pending",
        ]
        .into_iter()
        .collect::<std::collections::BTreeSet<_>>();
        // These are cache/write workers, not a document-open or fullscreen request. Their
        // established mounted frame and resume lifecycles remain separate from park terminal.
        let cache_or_write = [
            "folder_refresh_pending",
            // Edit producers retain their request/claim while parked. Existing mounted
            // polls resume them; bundle apply drains them through the all-context poll.
            // Parking must not use the document-open cancellation inventory for them.
            "local_adjust_lut_pending",
            "local_adjust_segmentation_pending",
            "stack_script_pending",
            "facet_name_cache_pending",
            "details_meta_pending",
            "metadata_pending",
            "tag_prewarm_pending",
            "converted_archive_cache_paths_pending",
            "view_trim_save_pending",
            "page_edit_reconcile_pending",
            "final_effect_pending",
        ]
        .into_iter()
        .collect::<std::collections::BTreeSet<_>>();
        assert!(registered.is_disjoint(&cache_or_write));
        assert_eq!(fields, registered.union(&cache_or_write).copied().collect());

        let names = ContextAsyncOwner::ALL
            .into_iter()
            .map(|owner| format!("{owner:?}"))
            .collect::<std::collections::BTreeSet<_>>();
        assert_eq!(names.len(), ContextAsyncOwner::ALL.len());
        for owner in [
            ContextAsyncOwner::PathClassification,
            ContextAsyncOwner::HistoryTransition,
            ContextAsyncOwner::CollectionGrid,
            ContextAsyncOwner::CollectionNavigation,
            ContextAsyncOwner::RatingNavigation,
            ContextAsyncOwner::BookmarkOpen,
            ContextAsyncOwner::FolderNavigation,
            ContextAsyncOwner::FolderPaneScan,
            ContextAsyncOwner::PdfEnumeration,
            ContextAsyncOwner::ZipEnumeration,
            ContextAsyncOwner::EpubConversion,
            ContextAsyncOwner::PdfPassword,
            ContextAsyncOwner::FullscreenDecode,
            ContextAsyncOwner::FinalAi,
            ContextAsyncOwner::LocalAdjustment,
            ContextAsyncOwner::ComicBake,
            ContextAsyncOwner::EraseInpaint,
            ContextAsyncOwner::SimilarPreview,
        ] {
            assert!(ContextAsyncOwner::ALL.contains(&owner));
        }
        let grid_source = include_str!("top_level_grid_view.rs");
        for field in [
            "collection_session:",
            "collection_navigation_pending:",
            "history_navigation_transition:",
            "open_path_classification:",
        ] {
            assert!(grid_source.contains(field));
        }
        assert!(bundle.contains("similar_panel:"));
    }

    #[test]
    fn epub_pending_and_visible_owner_inventory_is_explicit() {
        let owners = [
            (
                include_str!("../app.rs"),
                "path_owner: Option<crate::pdf_loader::LeasedEpubPath>",
            ),
            (
                include_str!("../app.rs"),
                "target_owner: Option<crate::pdf_loader::LeasedEpubPath>",
            ),
            (
                include_str!("../app.rs"),
                "requested: crate::pdf_loader::LeasedEpubPath",
            ),
            (
                include_str!("../app.rs"),
                "path: crate::pdf_loader::LeasedEpubPath",
            ),
            (
                include_str!("../app.rs"),
                "display_epub_source: Option<crate::pdf_loader::LeasedEpubPath>",
            ),
            (
                include_str!("../app.rs"),
                "fullscreen_epub_source: Option<crate::pdf_loader::LeasedEpubPath>",
            ),
            (
                include_str!("smart_folder.rs"),
                "logical_source: crate::pdf_loader::LeasedEpubPath",
            ),
            (
                include_str!("collection_navigation.rs"),
                "owner: crate::pdf_loader::LeasedEpubPath",
            ),
            (
                include_str!("collection_navigation.rs"),
                "book_owner: Option<crate::pdf_loader::LeasedEpubPath>",
            ),
            (
                include_str!("../bookmark_browser.rs"),
                "bookmark_source: crate::pdf_loader::LeasedEpubPath",
            ),
            (
                include_str!("../ui_dialogs/epub_convert.rs"),
                "src_owner: crate::pdf_loader::LeasedEpubPath",
            ),
        ];
        for (source, owner) in owners {
            assert!(source.contains(owner), "missing EPUB read owner: {owner}");
        }
        let resolver = include_str!("../pdf_loader.rs");
        assert!(
            resolver.contains("enum ReadTargetKind {")
                && resolver.contains("_lease: Arc<EpubReadLease>")
        );
        // The cache manager must rely on RAII owners rather than a post-hoc UI sweep.
        let sync_name = concat!("sync_epub", "_ui_read_leases");
        assert!(
            !concat!(include_str!("../app.rs"), include_str!("update_phases.rs"))
                .contains(sync_name)
        );
        assert!(!include_str!("../ui_dialogs/archive_cache_manager.rs").contains(sync_name));
    }

    #[cfg(windows)]
    #[test]
    fn epub_manager_protects_parked_book_and_invalidates_only_referencing_context() {
        let mut app = crate::app::setup_app_for_test();
        let source = PathBuf::from("C:/books/one.epub");
        let other = PathBuf::from("C:/books/two.epub");
        app.current_folder = Some(source.clone());
        app.items = vec![GridItem::PdfPage {
            pdf_path: source.clone(),
            page_num: 0,
            content_type: None,
        }];
        app.display_epub_source =
            Some(crate::pdf_loader::LeasedEpubPath::try_new(source.clone()).unwrap());
        let parked = app.stash_mounted_and_start_fresh("epub_delete_context_test");
        assert!(crate::pdf_loader::acquire_epub_delete_coverage(&source).is_err());
        app.items = vec![GridItem::PdfFile(other)];
        let parked_before = app
            .with_viewer_context_ref(parked, |context| context.items_generation())
            .unwrap();
        let mounted_before = app.items_generation;
        let removed = crate::epub_cache::GenerationRow {
            generation_id: 7,
            src_path_key: crate::epub_cache::src_key(&source),
            src_path: source.clone(),
            src_state: crate::epub_cache::SourceState {
                size: 1,
                mtime_ticks: 1,
            },
            src_sha256: String::new(),
            src_head_hash: String::new(),
            pdf_file: PathBuf::from("C:/data/book.g7.pdf"),
            pdf_size: 1,
            page_count: 1,
            direction: String::new(),
            profile: String::new(),
            created_at: 0,
            output_version: 1,
        };
        app.invalidate_removed_epub_generations(&[removed]);
        assert!(crate::pdf_loader::acquire_epub_delete_coverage(&source).is_err());
        assert_eq!(app.items_generation, mounted_before);
        assert_eq!(
            app.with_viewer_context_ref(parked, |context| context.items_generation())
                .unwrap(),
            parked_before.wrapping_add(1)
        );
    }

    #[cfg(windows)]
    #[test]
    fn epub_manager_protects_a_pending_open_before_visible_adoption() {
        let mut app = crate::app::setup_app_for_test();
        let source = PathBuf::from("C:/books/pending.epub");
        app.pdf_password_request = Some(super::super::PdfPasswordRequest::legacy(source.clone()));
        let parked = app.stash_mounted_and_start_fresh("epub_pending_delete_test");
        assert!(crate::pdf_loader::acquire_epub_delete_coverage(&source).is_err());
        assert!(
            app.with_viewer_context_ref(parked, |context| context
                .pending_context_async_owners()
                .contains(&ContextAsyncOwner::PdfPassword))
                .unwrap()
        );
    }

    #[cfg(windows)]
    #[test]
    fn epub_classification_candidate_owns_read_lease_before_delete_can_start() {
        let mut app = crate::app::setup_app_for_test();
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("candidate.epub");
        std::fs::write(&source, b"candidate").unwrap();
        assert!(matches!(
            app.start_open_path_classification(
                source.clone(),
                super::super::ClassifiedOpenContinuation::Direct {
                    auto_fullscreen: false,
                    owner: super::super::OpenRequestOwner::Navigation,

                    restore_intent: crate::app::StartupListIntent::ExplicitList,
                },
            ),
            super::super::OpenAdmission::Accepted
        ));
        assert!(app.top_level_grid_view.open_path_classification().is_some());
        let deletion_source = source.clone();
        let result = std::thread::spawn(move || {
            crate::pdf_loader::acquire_epub_delete_coverage(&deletion_source)
                .err()
                .expect("classification owns the source")
        })
        .join()
        .unwrap();
        assert!(result.contains("使用中"));
        app.top_level_grid_view.set_open_path_classification(None);
        assert!(crate::pdf_loader::acquire_epub_delete_coverage(&source).is_ok());
    }

    #[cfg(windows)]
    #[test]
    fn epub_conversion_state_follows_pdf_pending_through_context_stash() {
        let mut app = crate::app::setup_app_for_test();
        app.settings
            .set_archive_file_handling(crate::settings::ArchiveFileHandling::Ask);
        let path = PathBuf::from("C:/books/context.epub");
        assert_eq!(
            app.route_pdf_open_failure(
                super::super::OpenRequestOwner::Navigation,
                &path,
                super::super::PdfOpenFailure::NotConverted,
                crate::app::StartupListIntent::ExplicitList,
            ),
            super::super::PdfOpenFailureRoute::ConversionDialogOpened,
        );
        let allocation = std::ptr::from_ref(ContextRef::mounted(&app).epub_convert().unwrap());
        let stashed = app.stash_mounted_and_start_fresh("epub_test_stash");
        assert!(app.epub_convert.is_none());
        let bundle = app.viewer_contexts.table.at_rest(stashed).unwrap();
        assert_eq!(
            std::ptr::from_ref(ContextRef::at_rest(bundle).epub_convert().unwrap()),
            allocation
        );
        // A Smart request in the fresh main context cannot supersede the stashed viewer's
        // conversion. Exercise the actual dialog poll after mounting that viewer again.
        app.smart_folder_transition_sequence += 1;
        let retained = app
            .with_viewer_context(stashed, |app| {
                let ctx = egui::Context::default();
                let _ = ctx
                    .run_pass(Default::default(), |ctx| app.show_epub_convert_dialog(ctx))
                    .drop_without_applying_deltas();
                assert_eq!(
                    std::ptr::from_ref(ContextRef::mounted(app).epub_convert().unwrap()),
                    allocation
                );
                app.epub_convert
                    .as_ref()
                    .is_some_and(|state| state.src_path == path)
            })
            .unwrap();
        assert!(retained);
    }

    #[cfg(windows)]
    #[test]
    fn staged_epub_park_and_close_drop_only_own_request() {
        let mut app = crate::app::setup_app_for_test();
        app.settings
            .set_archive_file_handling(crate::settings::ArchiveFileHandling::Ask);
        let old = app.tmp.path().join("staged-epub-park-old");
        std::fs::create_dir(&old).unwrap();
        app.active_quick_folder_slot = None;
        app.current_folder = Some(old.clone());
        let epub = app.tmp.path().join("staged-epub-park.epub");
        std::fs::write(&epub, b"source").unwrap();
        let history = app.folder_nav_history_snapshot();
        assert!(app.start_physical_history_transition(
            super::super::PhysicalHistoryIntent::Navigation {
                replay: None,
                auto_fullscreen: false,
            },
            epub.clone(),
            crate::app::StartupListIntent::ExplicitList,
        ));
        app.settle_open_path_classification_for_test();
        let Some(super::super::HistoryNavigationTransition::Physical(mut request)) =
            app.top_level_grid_view.take_history_navigation_transition()
        else {
            panic!("staged EPUB request")
        };
        let request_id = request.request_id;
        request.phase = super::super::PhysicalHistoryPhase::Preflighting {
            preflight: super::super::collection_navigation::PhysicalHistoryPreflight::ready_for_test(
                super::super::collection_navigation::PhysicalHistoryPreflightPayload::PdfOpenFailure(
                    super::super::PdfOpenFailure::NotConverted,
                ),
            ),
            pdf_password_submission: None,
        };
        app.top_level_grid_view
            .set_history_navigation_transition(Some(
                super::super::HistoryNavigationTransition::Physical(request),
            ));
        app.poll_collection_history_transition(&egui::Context::default());
        assert!(app.staged_epub_conversion_is_current(
            app.projected_viewer_context_id(),
            request_id,
            &epub
        ));
        let parked = app.stash_mounted_and_start_fresh("staged_epub_park_test");
        assert!(app.epub_convert.is_none());
        assert!(
            app.top_level_grid_view
                .history_navigation_transition()
                .is_none()
        );
        let parked_was_cancelled = app
            .with_viewer_context(parked, |mounted| {
                assert!(mounted.epub_convert.is_some());
                mounted.pause_mounted_background_work_keep_current_frame();
                mounted.epub_convert.is_none()
                    && mounted
                        .top_level_grid_view
                        .history_navigation_transition()
                        .is_none()
                    && mounted.fs_nav_locked_gen.is_none()
            })
            .unwrap();
        assert!(parked_was_cancelled);
        app.close_and_retire_context(
            parked,
            "staged_epub_park_test_close",
            |mounted| {
                mounted
                    .finish_epub_convert(crate::ui_dialogs::epub_convert::EpubConvertExit::Abort);
            },
            |_| (),
        )
        .unwrap();
        assert_eq!(app.folder_nav_back_stack, history.back_stack);
        assert_eq!(app.folder_nav_forward_stack, history.forward_stack);
    }

    #[cfg(windows)]
    #[test]
    fn detached_epub_dialog_changes_only_detached_bundle_not_main_history() {
        let mut app = crate::app::setup_app_for_test();
        app.settings
            .set_archive_file_handling(crate::settings::ArchiveFileHandling::Ask);
        let main = app.tmp.path().join("main-epub-context");
        app.current_folder = Some(main.clone());
        let main_back = app.tmp.path().join("main-back");
        app.folder_nav_back_stack
            .push(super::super::FolderNavHistoryTarget::Path(main_back).into());
        let history = app.folder_nav_history_snapshot();
        let epub = app.tmp.path().join("detached.epub");
        let detached = app.build_window_context_for_test(981, |mounted| {
            assert_eq!(
                mounted.route_pdf_open_failure(
                    super::super::OpenRequestOwner::Navigation,
                    &epub,
                    super::super::PdfOpenFailure::NotConverted,
                    crate::app::StartupListIntent::ExplicitList,
                ),
                super::super::PdfOpenFailureRoute::ConversionDialogOpened
            );
        });
        assert!(app.epub_convert.is_none());
        assert_eq!(app.folder_nav_back_stack, history.back_stack);
        app.with_viewer_context(detached, |mounted| {
            assert!(mounted.epub_convert.is_some());
            assert!(!mounted.main_folder_history_available());
            mounted.finish_epub_convert(crate::ui_dialogs::epub_convert::EpubConvertExit::Abort);
            assert!(mounted.epub_convert.is_none());
        })
        .unwrap();
        assert_eq!(app.current_folder.as_deref(), Some(main.as_path()));
        assert_eq!(app.folder_nav_back_stack, history.back_stack);
        assert_eq!(app.folder_nav_forward_stack, history.forward_stack);
    }

    #[cfg(windows)]
    #[test]
    fn similar_move_p2_trace_owners_terminal_when_context_bundle_is_dropped() {
        let target = crate::snapshot::SnapshotTarget::Fs(PathBuf::from("c:/trace/pending/001.jpg"));
        let pending_trace = SimilarMoveTrace::new(SimilarMoveSource::ItemButton, Some(&target));
        let pending_id = pending_trace.id;
        let navigation_trace =
            SimilarMoveTrace::new(SimilarMoveSource::HistoryButton, Some(&target));
        let navigation_id = navigation_trace.id;
        let cancel = Arc::new(AtomicBool::new(false));
        let mut bundle = ViewerContextBundle::empty();
        let (pending, holdover) =
            similar_trace_owners(0, pending_trace, navigation_trace, Arc::clone(&cancel));
        bundle.folder_pane_open_pending = Some(pending);
        bundle.fs_holdover_tex = Some(holdover);

        drop(bundle);

        assert!(cancel.load(Ordering::Relaxed));
        assert_eq!(
            SimilarMoveTrace::terminal_reasons_for_test(pending_id),
            vec!["context_retired"]
        );
        assert_eq!(
            SimilarMoveTrace::terminal_reasons_for_test(navigation_id),
            vec!["context_retired"]
        );
    }

    #[cfg(windows)]
    fn two_video_zoom_contexts() -> (
        crate::app::AppTestEnvForTest,
        ViewerContextId,
        ViewerContextId,
        crate::video::zoom_view::VideoZoomState,
        crate::video::zoom_view::VideoZoomState,
    ) {
        let state_a = video_zoom_state(4.0, [-240.0, 80.0]);
        let state_b = video_zoom_state(7.0, [320.0, -120.0]);
        let mut app = crate::app::setup_app_for_test();
        let a = app.build_window_context_for_test(711, |app| {
            app.fullscreen_idx = Some(3);
            app.video_zoom_state = Some(state_a);
        });
        let b = app.build_window_context_for_test(712, |app| {
            app.fullscreen_idx = Some(7);
            app.video_zoom_state = Some(state_b);
        });
        (app, a, b, state_a, state_b)
    }

    #[cfg(windows)]
    #[test]
    fn viewer_context_video_zoom_fresh_context_starts_empty() {
        let state_a = video_zoom_state(4.0, [-240.0, 80.0]);
        let mut app = crate::app::setup_app_for_test();
        let _a = app.build_window_context_for_test(710, |app| {
            app.fullscreen_idx = Some(3);
            app.video_zoom_state = Some(state_a);
        });
        let _b = app.build_window_context_for_test(711, |app| {
            assert_eq!(
                app.video_zoom_state, None,
                "a fresh context must not inherit A's video zoom"
            );
        });
    }

    #[cfg(windows)]
    #[test]
    fn viewer_context_video_zoom_survives_exchange_and_return() {
        let (mut app, a, b, state_a, state_b) = two_video_zoom_contexts();

        for _ in 0..2 {
            app.with_viewer_context(a, |app| {
                assert_eq!(app.video_zoom_state, Some(state_a));
            })
            .unwrap();
            app.with_viewer_context(b, |app| {
                assert_eq!(app.video_zoom_state, Some(state_b));
            })
            .unwrap();
        }
    }

    #[cfg(windows)]
    #[test]
    fn viewer_context_final_cover_preference_survives_exchange_and_return() {
        use crate::settings::FinalCoverSpreadPreference;

        let mut app = crate::app::setup_app_for_test();
        let a = app.build_window_context_for_test(715, |app| {
            app.final_cover_spread_preference = FinalCoverSpreadPreference::On;
        });
        let b = app.build_window_context_for_test(716, |app| {
            app.final_cover_spread_preference = FinalCoverSpreadPreference::Off;
        });

        for _ in 0..2 {
            app.with_viewer_context(a, |app| {
                assert_eq!(
                    app.final_cover_spread_preference,
                    FinalCoverSpreadPreference::On
                );
            })
            .unwrap();
            app.with_viewer_context(b, |app| {
                assert_eq!(
                    app.final_cover_spread_preference,
                    FinalCoverSpreadPreference::Off
                );
            })
            .unwrap();
        }
    }

    #[cfg(windows)]
    #[test]
    fn viewer_context_singleton_placement_preference_survives_exchange_and_return() {
        use crate::settings::SingletonSpreadPlacementPreference;

        let mut app = crate::app::setup_app_for_test();
        let a = app.build_window_context_for_test(721, |app| {
            app.singleton_spread_endpoint_preferences =
                SingletonSpreadPlacementPreference::Place.into();
        });
        let b = app.build_window_context_for_test(722, |app| {
            app.singleton_spread_endpoint_preferences =
                SingletonSpreadPlacementPreference::Center.into();
        });

        for _ in 0..2 {
            app.with_viewer_context(a, |app| {
                assert_eq!(
                    app.singleton_spread_endpoint_preferences,
                    SingletonSpreadPlacementPreference::Place
                );
            })
            .unwrap();
            app.with_viewer_context(b, |app| {
                assert_eq!(
                    app.singleton_spread_endpoint_preferences,
                    SingletonSpreadPlacementPreference::Center
                );
            })
            .unwrap();
        }
    }

    #[cfg(windows)]
    #[test]
    fn page_alone_preference_survives_context_exchange_and_only_inherited_changes_invalidate() {
        use crate::settings::{PageAlonePreference as P, PageAlonePreferences};
        let mut app = crate::app::setup_app_for_test();
        let inherited_after = app.build_window_context_for_test(731, |owner| {
            owner.spread_mode = crate::settings::SpreadMode::LtrCover;
            owner.page_alone_preferences = PageAlonePreferences {
                after_cover: P::FollowGlobal,
                last: P::Off,
            };
            owner.fs_vertical_scroll = 111.0;
        });
        let explicit_after = app.build_window_context_for_test(732, |owner| {
            owner.spread_mode = crate::settings::SpreadMode::LtrCover;
            owner.page_alone_preferences = PageAlonePreferences {
                after_cover: P::On,
                last: P::FollowGlobal,
            };
            owner.fs_vertical_scroll = 222.0;
        });
        let cover_off = app.build_window_context_for_test(733, |owner| {
            owner.spread_mode = crate::settings::SpreadMode::Ltr;
            owner.page_alone_preferences = PageAlonePreferences::default();
            owner.fs_vertical_scroll = 333.0;
        });
        app.invalidate_page_alone_in_parked_contexts(true, false);
        for (id, preference, scroll) in [
            (
                inherited_after,
                PageAlonePreferences {
                    after_cover: P::FollowGlobal,
                    last: P::Off,
                },
                0.0,
            ),
            (
                explicit_after,
                PageAlonePreferences {
                    after_cover: P::On,
                    last: P::FollowGlobal,
                },
                222.0,
            ),
            (cover_off, PageAlonePreferences::default(), 333.0),
        ] {
            app.with_viewer_context(id, |owner| {
                assert_eq!(owner.page_alone_preferences, preference);
                assert_eq!(owner.fs_vertical_scroll, scroll);
            })
            .unwrap();
        }
        app.invalidate_page_alone_in_parked_contexts(false, true);
        app.with_viewer_context(explicit_after, |owner| {
            assert_eq!(owner.fs_vertical_scroll, 0.0);
        })
        .unwrap();
        app.with_viewer_context(cover_off, |owner| {
            assert_eq!(owner.fs_vertical_scroll, 333.0);
        })
        .unwrap();
    }

    #[cfg(windows)]
    #[test]
    fn global_final_cover_change_invalidates_only_parked_follow_global_geometry() {
        use crate::settings::FinalCoverSpreadPreference;

        let mut app = crate::app::setup_app_for_test();
        let inherited = app.build_window_context_for_test(717, |app| {
            app.final_cover_spread_preference = FinalCoverSpreadPreference::FollowGlobal;
            app.fs_vertical_scroll = 123.0;
            app.fullscreen_page_layout
                .begin(crate::displayed_image_transform::FullscreenPageLayoutKind::Continuous);
        });
        let explicit = app.build_window_context_for_test(718, |app| {
            app.final_cover_spread_preference = FinalCoverSpreadPreference::On;
            app.fs_vertical_scroll = 456.0;
            app.fullscreen_page_layout
                .begin(crate::displayed_image_transform::FullscreenPageLayoutKind::Continuous);
        });

        app.invalidate_final_cover_spread_display_in_parked_contexts();

        let inherited = app.viewer_contexts.table.at_rest(inherited).unwrap();
        assert_eq!(inherited.fs_vertical_scroll, 0.0);
        assert_eq!(
            inherited.fullscreen_page_layout.kind(),
            crate::displayed_image_transform::FullscreenPageLayoutKind::Empty
        );
        let explicit = app.viewer_contexts.table.at_rest(explicit).unwrap();
        assert_eq!(explicit.fs_vertical_scroll, 456.0);
        assert_eq!(
            explicit.fullscreen_page_layout.kind(),
            crate::displayed_image_transform::FullscreenPageLayoutKind::Continuous
        );
    }

    #[cfg(windows)]
    #[test]
    fn global_singleton_placement_change_invalidates_only_parked_follow_global_geometry() {
        use crate::settings::{
            SingletonSpreadEndpointPreferences, SingletonSpreadPlacementPreference,
        };

        let mut app = crate::app::setup_app_for_test();
        let inherited = app.build_window_context_for_test(723, |app| {
            app.singleton_spread_endpoint_preferences =
                SingletonSpreadPlacementPreference::FollowGlobal.into();
            app.fs_vertical_scroll = 123.0;
            app.fullscreen_page_layout
                .begin(crate::displayed_image_transform::FullscreenPageLayoutKind::Continuous);
        });
        let explicit = app.build_window_context_for_test(724, |app| {
            app.singleton_spread_endpoint_preferences =
                SingletonSpreadPlacementPreference::Place.into();
            app.fs_vertical_scroll = 456.0;
            app.fullscreen_page_layout
                .begin(crate::displayed_image_transform::FullscreenPageLayoutKind::Continuous);
        });
        let first_explicit = app.build_window_context_for_test(725, |app| {
            app.singleton_spread_endpoint_preferences = SingletonSpreadEndpointPreferences {
                first: SingletonSpreadPlacementPreference::Place,
                last: SingletonSpreadPlacementPreference::FollowGlobal,
            };
            app.fs_vertical_scroll = 725.0;
            app.fullscreen_page_layout
                .begin(crate::displayed_image_transform::FullscreenPageLayoutKind::Continuous);
        });
        let last_explicit = app.build_window_context_for_test(726, |app| {
            app.singleton_spread_endpoint_preferences = SingletonSpreadEndpointPreferences {
                first: SingletonSpreadPlacementPreference::FollowGlobal,
                last: SingletonSpreadPlacementPreference::Center,
            };
            app.fs_vertical_scroll = 726.0;
            app.fullscreen_page_layout
                .begin(crate::displayed_image_transform::FullscreenPageLayoutKind::Continuous);
        });

        app.invalidate_singleton_spread_placement_in_parked_contexts(true, true);

        let inherited = app.viewer_contexts.table.at_rest(inherited).unwrap();
        assert_eq!(inherited.fs_vertical_scroll, 0.0);
        assert_eq!(
            inherited.fullscreen_page_layout.kind(),
            crate::displayed_image_transform::FullscreenPageLayoutKind::Empty
        );
        let explicit = app.viewer_contexts.table.at_rest(explicit).unwrap();
        assert_eq!(explicit.fs_vertical_scroll, 456.0);
        assert_eq!(
            explicit.fullscreen_page_layout.kind(),
            crate::displayed_image_transform::FullscreenPageLayoutKind::Continuous
        );
        for id in [first_explicit, last_explicit] {
            let mixed = app.viewer_contexts.table.at_rest(id).unwrap();
            assert_eq!(mixed.fs_vertical_scroll, 0.0);
            assert_eq!(
                mixed.fullscreen_page_layout.kind(),
                crate::displayed_image_transform::FullscreenPageLayoutKind::Empty
            );
        }
    }

    #[cfg(windows)]
    #[test]
    fn global_endpoint_change_preserves_parked_scroll_and_pan_when_changed_end_is_explicit() {
        use crate::settings::{
            SingletonSpreadEndpointPreferences as Preferences,
            SingletonSpreadPlacementPreference as Preference,
        };

        for (first_changed, last_changed, preferences) in [
            (
                true,
                false,
                Preferences {
                    first: Preference::Place,
                    last: Preference::FollowGlobal,
                },
            ),
            (
                false,
                true,
                Preferences {
                    first: Preference::FollowGlobal,
                    last: Preference::Center,
                },
            ),
        ] {
            let mut app = crate::app::setup_app_for_test();
            let id = app.build_window_context_for_test(727, |app| {
                app.singleton_spread_endpoint_preferences = preferences;
                app.fs_vertical_scroll = 727.0;
                app.fs_pan = egui::vec2(12.0, -34.0);
                app.fullscreen_page_layout
                    .begin(crate::displayed_image_transform::FullscreenPageLayoutKind::Continuous);
            });
            app.invalidate_singleton_spread_placement_in_parked_contexts(
                first_changed,
                last_changed,
            );
            let parked = app.viewer_contexts.table.at_rest(id).unwrap();
            assert_eq!(parked.fs_vertical_scroll, 727.0);
            assert_eq!(parked.fs_pan, egui::vec2(12.0, -34.0));
            assert_eq!(
                parked.fullscreen_page_layout.kind(),
                crate::displayed_image_transform::FullscreenPageLayoutKind::Continuous
            );
        }
    }

    #[cfg(windows)]
    #[test]
    fn final_cover_painted_layout_identity_is_retired_only_for_the_context_that_changes_or_closes()
    {
        use crate::displayed_image_transform::FullscreenPageLayoutKind;

        let mut app = crate::app::setup_app_for_test();
        let first = app.build_window_context_for_test(719, |app| {
            app.fullscreen_page_layout
                .begin(FullscreenPageLayoutKind::Spread);
        });
        let second = app.build_window_context_for_test(720, |app| {
            app.items = vec![crate::grid_item::GridItem::Image(PathBuf::from(
                "c:/second/0.png",
            ))];
            app.thumbnails = vec![crate::app::ThumbnailState::Pending];
            app.fullscreen_idx = Some(0);
            app.fullscreen_page_layout
                .begin(FullscreenPageLayoutKind::Continuous);
        });

        let first_bundle = app.viewer_contexts.table.at_rest_mut(first).unwrap();
        first_bundle.set_items_generation(first_bundle.items_generation.wrapping_add(1));
        assert_eq!(
            first_bundle.fullscreen_page_layout.kind(),
            FullscreenPageLayoutKind::Empty
        );
        assert_eq!(
            app.viewer_contexts
                .table
                .at_rest(second)
                .unwrap()
                .fullscreen_page_layout
                .kind(),
            FullscreenPageLayoutKind::Continuous
        );

        app.with_viewer_context(first, |app| {
            app.fullscreen_page_layout
                .begin(FullscreenPageLayoutKind::Single);
        })
        .unwrap();
        app.with_viewer_context(second, |app| {
            app.close_fullscreen();
            assert_eq!(
                app.fullscreen_page_layout.kind(),
                FullscreenPageLayoutKind::Empty
            );
        })
        .unwrap();
        app.with_viewer_context(first, |app| {
            assert_eq!(
                app.fullscreen_page_layout.kind(),
                FullscreenPageLayoutKind::Single,
                "closing the second viewer must not retire the first viewer's painted layout"
            );
        })
        .unwrap();
    }

    #[cfg(windows)]
    #[test]
    fn viewer_context_video_zoom_item_reset_only_changes_its_owner() {
        let (mut app, a, b, state_a, _) = two_video_zoom_contexts();
        app.with_viewer_context(b, |app| {
            app.video_zoom_state = None;
        })
        .unwrap();
        app.with_viewer_context(a, |app| {
            assert_eq!(
                app.video_zoom_state,
                Some(state_a),
                "B's item-change reset must not clear A"
            );
        })
        .unwrap();
    }

    #[cfg(windows)]
    #[test]
    fn viewer_context_video_zoom_close_only_changes_its_owner() {
        let (mut app, a, b, state_a, _) = two_video_zoom_contexts();
        app.close_and_retire_context(
            b,
            "video_zoom_test_close",
            |app| {
                app.close_fullscreen();
            },
            |_| (),
        )
        .unwrap();
        app.with_viewer_context(a, |app| {
            assert_eq!(
                app.video_zoom_state,
                Some(state_a),
                "closing B must not reset A"
            );
        })
        .unwrap();
    }

    #[cfg(windows)]
    #[test]
    fn viewer_context_video_zoom_split_moves_an_independent_value() {
        let state_a = video_zoom_state(4.0, [-240.0, 80.0]);
        let state_main = video_zoom_state(7.0, [320.0, -120.0]);
        let mut app = crate::app::setup_app_for_test();
        app.fullscreen_idx = Some(3);
        app.video_zoom_state = Some(state_a);

        let mut fork = app.split_current_context_preserving_main_grid();
        assert_eq!(app.video_zoom_state, None);
        app.video_zoom_state = Some(state_main);

        app.swap_viewer_context_bundle(&mut fork);
        assert_eq!(app.video_zoom_state, Some(state_a));
        app.video_zoom_state = None;

        app.swap_viewer_context_bundle(&mut fork);
        assert_eq!(app.video_zoom_state, Some(state_main));
        app.swap_viewer_context_bundle(&mut fork);
        assert_eq!(app.video_zoom_state, None);
    }

    #[cfg(windows)]
    #[test]
    fn virtual_list_route_state_survives_context_swap_without_sibling_leak() {
        let mut app = crate::app::setup_app_for_test();
        app.global_search.active = true;
        app.global_search.query = "main search".into();
        app.rating_view_stars = 3;
        app.rating_view_request_sequence = 41;
        let mut sibling = ViewerContextBundle::empty();
        sibling.rating_view_stars = 5;
        sibling.rating_view_request_sequence = 9;
        app.swap_viewer_context_bundle(&mut sibling);
        assert!(!app.global_search.active);
        assert_eq!(app.rating_view_stars, 5);
        assert_eq!(app.rating_view_request_sequence, 9);
        assert_eq!(sibling.global_search.query, "main search");
        app.swap_viewer_context_bundle(&mut sibling);
        assert!(app.global_search.active);
        assert_eq!(app.rating_view_stars, 3);
        assert_eq!(app.rating_view_request_sequence, 41);
    }

    #[cfg(windows)]
    #[test]
    fn viewer_context_seek_strip_resources_survive_owner_roundtrip_and_move_only_to_live_fork() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("context-a.mp4");
        let mut app = crate::app::setup_app_for_test();
        app.fullscreen_idx = Some(4);
        let worker = crate::video::seek_strip_wave::SeekStripWaveWorker::spawn(
            path.clone(),
            Some(0),
            Some(0),
            None,
        );
        let worker_id = worker.identity_for_test();
        app.seed_video_seek_strip_context_for_test(path, worker, 41, 17, 42);
        let owner = app.video_seek_strip_context_owner_for_test().unwrap();

        let mut sibling = ViewerContextBundle::empty();
        sibling.video_seek_strip_next_session_id = 901;
        app.swap_viewer_context_bundle(&mut sibling);
        assert_eq!(app.video_seek_strip_context_owner_for_test(), None);
        assert_eq!(app.video_seek_strip_next_session_id, 901);
        assert_eq!(
            sibling.video_seek_strip_next_session_id, 42,
            "the next id belongs to the same resource-session owner"
        );
        app.swap_viewer_context_bundle(&mut sibling);
        assert_eq!(app.video_seek_strip_context_owner_for_test(), Some(owner));
        assert_eq!(owner.1, 17);
        assert_eq!(owner.2, worker_id);
        assert_eq!(app.video_seek_strip_next_session_id, 42);
        assert!(matches!(
            sibling.video_seek_strip_runtime,
            crate::app::native_video::VideoSeekStripRuntime::Closed
        ));
        assert_eq!(sibling.video_seek_strip_next_session_id, 901);

        let parked = app.split_current_context_preserving_main_grid();
        assert_eq!(app.video_seek_strip_context_owner_for_test(), None);
        assert_eq!(app.video_seek_strip_next_session_id, 1);
        assert_eq!(
            parked.video_seek_strip_runtime.context_owner_for_test(),
            Some(owner),
            "the live-media fork receives the player-scoped strip resource"
        );
        assert_eq!(parked.video_seek_strip_next_session_id, 42);
        drop(parked);
        drop(app);

        // The Closed-session wave holdover and its identity move through the same transaction.
        let mut held_app = crate::app::setup_app_for_test();
        held_app.fullscreen_idx = Some(6);
        held_app.video_seek_strip_next_session_id = 72;
        let held_path = dir.path().join("held.mp4");
        let held_worker = crate::video::seek_strip_wave::SeekStripWaveWorker::spawn(
            held_path.clone(),
            Some(0),
            Some(0),
            None,
        );
        let held_id = held_worker.identity_for_test();
        held_app.video_seek_strip_wave_holdover =
            Some(crate::app::native_video::HeldSeekStripWaveWorker {
                owner_fs_idx: 6,
                path: held_path,
                audio_stream_index: Some(0),
                source_epoch: 8,
                items_generation: held_app.items_generation,
                worker: held_worker,
            });
        let held_parked = held_app.split_current_context_preserving_main_grid();
        assert!(held_app.video_seek_strip_wave_holdover.is_none());
        assert_eq!(held_app.video_seek_strip_next_session_id, 1);
        assert_eq!(held_parked.video_seek_strip_next_session_id, 72);
        assert_eq!(
            held_parked
                .video_seek_strip_wave_holdover
                .as_ref()
                .map(|held| held.worker.identity_for_test()),
            Some(held_id)
        );
    }

    #[cfg(windows)]
    #[test]
    fn viewer_context_secondary_press_survives_ordinary_deposit_but_not_sibling_mount() {
        let mut app = crate::app::setup_app_for_test();
        let started = std::time::Instant::now();
        let pos = egui::pos2(100.0, 120.0);
        let mut expected = None;
        let a = app.build_window_context_for_test(801, |mounted| {
            mounted.fullscreen_idx = Some(3);
            mounted.fs_secondary_press =
                crate::ui_fullscreen::FullscreenSecondaryPress::armed_for_test(
                    mounted.projected_viewer_context_id(),
                    mounted.items_generation,
                    3,
                    crate::ring_shortcut::RightDragContext::ImageFullscreen,
                    started,
                    pos,
                );
            expected = Some(mounted.fs_secondary_press);
        });
        let b = app.build_window_context_for_test(802, |mounted| {
            mounted.fullscreen_idx = Some(3);
            assert_eq!(
                mounted.fs_secondary_press,
                crate::ui_fullscreen::FullscreenSecondaryPress::Idle
            );
        });
        let expected = expected.unwrap();

        app.with_viewer_context(a, |mounted| {
            assert_eq!(mounted.fs_secondary_press, expected);
            let context_id = mounted.projected_viewer_context_id();
            let items_generation = mounted.items_generation;
            assert_eq!(
                mounted.fs_secondary_press.finish_for_test(
                    context_id,
                    items_generation,
                    3,
                    crate::ring_shortcut::RightDragContext::ImageFullscreen,
                    started + std::time::Duration::from_millis(100),
                    pos,
                    false,
                    true,
                ),
                Some(false),
                "the owning viewer must complete its short release after remount"
            );
            let long_started = started + std::time::Duration::from_secs(1);
            mounted.fs_secondary_press =
                crate::ui_fullscreen::FullscreenSecondaryPress::armed_for_test(
                    context_id,
                    items_generation,
                    3,
                    crate::ring_shortcut::RightDragContext::ImageFullscreen,
                    long_started,
                    pos,
                );
        })
        .unwrap();
        app.with_viewer_context(b, |mounted| {
            assert_eq!(
                mounted.fs_secondary_press,
                crate::ui_fullscreen::FullscreenSecondaryPress::Idle,
                "a sibling with the same fullscreen index must not receive the press"
            );
        })
        .unwrap();
        app.with_viewer_context(a, |mounted| {
            let context_id = mounted.projected_viewer_context_id();
            let items_generation = mounted.items_generation;
            assert_eq!(
                mounted.fs_secondary_press.finish_for_test(
                    context_id,
                    items_generation,
                    3,
                    crate::ring_shortcut::RightDragContext::ImageFullscreen,
                    started
                        + std::time::Duration::from_secs(1)
                        + crate::ring_shortcut::mouse_flick_menu_delay(),
                    pos,
                    true,
                    false,
                ),
                Some(true),
                "ordinary deposit/remount must preserve the owning long-press sequence"
            );
        })
        .unwrap();
    }

    #[cfg(windows)]
    #[test]
    fn viewer_context_secondary_press_is_terminal_at_live_media_fork() {
        let mut app = crate::app::setup_app_for_test();
        app.fullscreen_idx = Some(3);
        app.fs_secondary_press = crate::ui_fullscreen::FullscreenSecondaryPress::armed_for_test(
            app.projected_viewer_context_id(),
            app.items_generation,
            3,
            crate::ring_shortcut::RightDragContext::ImageFullscreen,
            std::time::Instant::now(),
            egui::pos2(100.0, 120.0),
        );

        let mut fork = app.split_current_context_preserving_main_grid();
        assert_eq!(
            app.fs_secondary_press,
            crate::ui_fullscreen::FullscreenSecondaryPress::Idle
        );
        app.swap_viewer_context_bundle(&mut fork);
        assert_eq!(
            app.fs_secondary_press,
            crate::ui_fullscreen::FullscreenSecondaryPress::Idle,
            "a fork cannot revive a physical press after the ownership boundary"
        );
    }

    #[cfg(windows)]
    #[test]
    fn viewer_context_mouse_seek_hold_survives_ordinary_at_rest_roundtrip() {
        use crate::ring_shortcut::MouseButtonSlot;

        let mut app = crate::app::setup_app_for_test();
        let a = app.build_window_context_for_test(713, |app| {
            let context_id = app.projected_viewer_context_id();
            app.native_video_mouse_seek_holds
                .seed_for_test(context_id, MouseButtonSlot::Back);
        });
        let b = app.build_window_context_for_test(714, |app| {
            assert!(
                !app.native_video_mouse_seek_holds
                    .is_armed_for_test(MouseButtonSlot::Back)
            );
        });

        for _ in 0..2 {
            app.with_viewer_context(a, |app| {
                assert!(
                    app.native_video_mouse_seek_holds
                        .is_armed_for_test(MouseButtonSlot::Back)
                );
            })
            .unwrap();
            app.with_viewer_context(b, |app| {
                assert!(
                    !app.native_video_mouse_seek_holds
                        .is_armed_for_test(MouseButtonSlot::Back)
                );
            })
            .unwrap();
        }
    }

    #[cfg(windows)]
    #[test]
    fn parked_live_split_is_a_terminal_for_mouse_seek_hold() {
        use crate::ring_shortcut::MouseButtonSlot;

        let mut app = crate::app::setup_app_for_test();
        let context_id = app.projected_viewer_context_id();
        app.native_video_mouse_seek_holds
            .seed_for_test(context_id, MouseButtonSlot::Forward);

        let parked = app.split_current_context_preserving_main_grid();

        assert!(
            !app.native_video_mouse_seek_holds
                .is_armed_for_test(MouseButtonSlot::Forward)
        );
        assert!(
            !parked
                .native_video_mouse_seek_holds
                .is_armed_for_test(MouseButtonSlot::Forward)
        );
    }

    #[cfg(windows)]
    #[test]
    fn viewer_presentation_close_is_a_terminal_for_mouse_seek_hold() {
        use crate::ring_shortcut::MouseButtonSlot;

        let mut app = crate::app::setup_app_for_test();
        let context_id = app.projected_viewer_context_id();
        app.native_video_mouse_seek_holds
            .seed_for_test(context_id, MouseButtonSlot::Back);

        app.prepare_viewer_presentation_close();

        assert!(
            !app.native_video_mouse_seek_holds
                .is_armed_for_test(MouseButtonSlot::Back)
        );
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
                origin_pointer: egui::pos2(100.0, 80.0),
                layout_center_pos: 440,
                page_pos_at_origin: 3,
                drag_step_width: 40.0,
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
        app.settings.grid_cols = 3;
        let a = crate::settings::FavoriteEntry::new(
            "a".to_owned(),
            std::path::PathBuf::from(r"C:\favorite-a"),
        );
        let b = crate::settings::FavoriteEntry::new(
            "b".to_owned(),
            std::path::PathBuf::from(r"C:\favorite-b"),
        );
        let mut a_state = crate::settings::FavoriteViewState::from_settings(&app.settings);
        a_state.grid_cols = 4;
        let mut b_state = a_state.clone();
        b_state.grid_cols = 7;
        app.favorite_view_states.insert(a.id, a_state);
        app.favorite_view_states.insert(b.id, b_state);
        app.settings.favorites.extend([a.clone(), b.clone()]);
        app.current_folder = Some(a.path.clone());
        app.transition_favorite_view_for_path(Some(&a.path));
        app.settings.grid_cols = 9;

        let mut parked = ViewerContextBundle::empty();
        parked.current_folder = Some(b.path.clone());
        app.swap_viewer_context_bundle(&mut parked);
        assert_eq!(app.settings.grid_cols, 7);
        assert_eq!(app.favorite_view_states[&a.id].grid_cols, 9);

        app.settings.grid_cols = 10;
        app.swap_viewer_context_bundle(&mut parked);
        assert_eq!(app.settings.grid_cols, 9);
        assert_eq!(app.favorite_view_states[&b.id].grid_cols, 10);

        app.transition_favorite_view_for_path(Some(std::path::Path::new(r"C:\outside")));
        assert_eq!(
            app.settings.grid_cols, 3,
            "共通状態はどちらの窓にも上書きされない"
        );
    }

    #[cfg(windows)]
    #[test]
    fn specified_global_fit_change_invalidates_parked_references_without_mounting() {
        let mut app = crate::app::setup_app_for_test();
        let seed = |owner: &mut App| {
            owner.fs_continuous_fit_reference =
                crate::ui_fullscreen::ContinuousSpecifiedFitReference::Specified {
                    unit: crate::ui_fullscreen::ContinuousReadingUnitSpec::pages(0, vec![0]),
                    settings: Default::default(),
                    limits: Default::default(),
                    base: None,
                };
            owner.fs_vertical_scroll = 123.0;
            owner.fs_zoom = 1.7;
        };
        let parked_id = app.build_window_context_for_test(1342, seed);
        seed(&mut app);
        let mounted = app.mounted_viewer_context_id();
        app.invalidate_all_continuous_specified_fit_references();
        assert!(matches!(
            app.fs_continuous_fit_reference,
            crate::ui_fullscreen::ContinuousSpecifiedFitReference::Legacy
        ));
        let parked = app.viewer_contexts.table.at_rest(parked_id).unwrap();
        assert!(matches!(
            parked.fs_continuous_fit_reference,
            crate::ui_fullscreen::ContinuousSpecifiedFitReference::Legacy
        ));
        assert_eq!(parked.fs_vertical_scroll, 123.0);
        assert_eq!(parked.fs_zoom, 1.7);
        assert_eq!(app.mounted_viewer_context_id(), mounted);
        assert_eq!(app.fs_zoom, 1.7);
    }

    #[cfg(windows)]
    #[test]
    fn specified_pdf_metadata_and_demand_move_with_the_owning_bundle() {
        let mut app = crate::app::setup_app_for_test();
        let main_path = app.tmp.path().join("main.pdf");
        let other_path = app.tmp.path().join("other.pdf");
        app.items.push(GridItem::PdfPage {
            pdf_path: main_path,
            page_num: 0,
            content_type: Some(crate::pdf_loader::PdfPageContentType::Vector),
        });
        let main_generation = app.items_generation;
        app.page_dims_cache
            .record_layout(main_generation, 0, (612_000, 612_000));
        assert!(
            app.fs_pdf_rerender_demand
                .note_absolute(0, "main".into(), 5407)
        );
        let main_basis = app.pdf_specified_basis_for_idx(0).unwrap();
        let main_reference = crate::ui_fullscreen::ContinuousSpecifiedFitReference::Specified {
            unit: crate::ui_fullscreen::ContinuousReadingUnitSpec::pages(0, vec![0]),
            settings: crate::settings::FullscreenSpecifiedScale::default(),
            limits: Default::default(),
            base: None,
        };
        app.fs_continuous_fit_reference = main_reference.clone();
        let mut parked = ViewerContextBundle::empty();
        parked.items.push(GridItem::PdfPage {
            pdf_path: other_path,
            page_num: 0,
            content_type: Some(crate::pdf_loader::PdfPageContentType::Raster { w: 100, h: 200 }),
        });
        parked
            .page_dims_cache
            .record_layout(parked.items_generation, 0, (100_000, 200_000));
        assert!(
            parked
                .fs_pdf_rerender_demand
                .note_absolute(0, "other".into(), 200)
        );
        app.swap_viewer_context_bundle(&mut parked);
        let other_basis = app.pdf_specified_basis_for_idx(0).unwrap();
        assert_eq!(other_basis.original_size, egui::vec2(100.0, 200.0));
        assert!(
            !app.fs_pdf_rerender_demand
                .note_absolute(0, "other".into(), 200)
        );
        assert!(
            !parked
                .fs_pdf_rerender_demand
                .note_absolute(0, "main".into(), 5407)
        );
        app.fs_pdf_rerender_demand.reset();
        app.reset_continuous_reading_transform();
        app.swap_viewer_context_bundle(&mut parked);
        assert_eq!(app.pdf_specified_basis_for_idx(0), Some(main_basis));
        assert_eq!(
            format!("{:?}", app.fs_continuous_fit_reference),
            format!("{main_reference:?}"),
            "resetting a same-index sibling must preserve the original unit"
        );
        assert!(matches!(
            parked.fs_continuous_fit_reference,
            crate::ui_fullscreen::ContinuousSpecifiedFitReference::Legacy
        ));
        assert!(
            !app.fs_pdf_rerender_demand
                .note_absolute(0, "main".into(), 5407)
        );
        assert!(
            parked
                .fs_pdf_rerender_demand
                .note_absolute(0, "other".into(), 200)
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
        app.last_viewer_sync_stamp = Some(ViewerSyncStamp {
            idx: 1,
            item_key: "stale".to_owned(),
            items_generation: 10,
        });

        app.activate_mounted_as_independent_detached(37);

        assert_eq!(app.viewer_presentation, ViewerPresentation::DetachedWindow);
        assert!(app.detached_viewer_independent_active);
        assert!(!app.detached_viewer_open_next_still_detached_once);
        assert_eq!(app.detached_viewer_window_id(), Some(37));
        assert_eq!(app.last_viewer_sync_stamp, None);
        assert!(!app.fs_open_intent_from_grid);
        assert!(!app.pending_auto_fs_open);
        assert!(!app.pending_return_to_parent.is_pending());
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

#[cfg(windows)]
fn parked_direct_media_owner(bundle: &ViewerContextBundle) -> bool {
    bundle.snapshot.as_ref().is_some_and(|snapshot| {
        matches!(
            snapshot.source_label,
            crate::snapshot::SnapshotSourceLabel::DirectMedia { .. }
        )
    })
}

impl crate::app::App {
    pub(crate) fn page_turn_destination(&self) -> crate::ui_fullscreen::PresentationDestination {
        #[cfg(windows)]
        let window = self.detached_viewer_window_id();
        #[cfg(not(windows))]
        let window = None;
        let viewport = match self.viewer_chrome_surface() {
            crate::ui_helpers::ViewerChromeSurface::MainEmbedded => egui::ViewportId::ROOT,
            crate::ui_helpers::ViewerChromeSurface::Fullscreen => self.fullscreen_viewport_id(),
            crate::ui_helpers::ViewerChromeSurface::Detached { window_id, .. } => {
                egui::ViewportId::from_hash_of(("detached_image_window", window_id))
            }
        };
        crate::ui_fullscreen::PresentationDestination {
            context: self.projected_viewer_context_id(),
            window,
            viewport,
        }
    }
}

impl App {
    /// Only an ON-origin mandatory flat can keep operation tracking alive after
    /// OFF. Read the same context owners without allocating an inventory.
    pub(crate) fn page_turn_operation_tracking_needed(&self) -> bool {
        if self.settings.page_turn_animation_enabled || self.page_turn_barrier_active() {
            return true;
        }
        #[cfg(windows)]
        return self.viewer_contexts.table.slots.values().any(|slot| matches!(slot, Slot::AtRest(bundle) if bundle.page_turn_presentation.has_flat_completion()));
        #[cfg(not(windows))]
        false
    }

    /// A setting change or tray hide targets every affected presentation explicitly.
    /// Ordinary context mounting never calls this or invalidates a sibling.
    pub(crate) fn deactivate_all_page_turn_presentations(&mut self, reason: &'static str) {
        self.pending_fs_page_turn_ready = None;
        self.page_turn_presentation.deactivate(reason);
        #[cfg(windows)]
        for id in self.viewer_contexts.table.other_ids() {
            if let Some(bundle) = self.viewer_contexts.table.at_rest_mut(id) {
                bundle.pending_fs_page_turn_ready = None;
                bundle.page_turn_presentation.deactivate(reason);
            }
        }
    }

    /// A setting change ends the effect, but cannot discard an ON-origin flat
    /// completion and its one already-resolved continuation.
    pub(crate) fn cancel_all_page_turn_effects_for_settings(&mut self) {
        self.pending_fs_page_turn_ready = None;
        self.page_turn_presentation
            .cancel_for_settings(&mut self.page_turn_leases);
        #[cfg(windows)]
        for id in self.viewer_contexts.table.other_ids() {
            if let Some(bundle) = self.viewer_contexts.table.at_rest_mut(id) {
                bundle.pending_fs_page_turn_ready = None;
                bundle
                    .page_turn_presentation
                    .cancel_for_settings(&mut self.page_turn_leases);
            }
        }
    }
}
