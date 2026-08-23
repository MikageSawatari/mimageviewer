// Stage ②-d wires this state machine to the production viewer-context payload.
#![allow(dead_code)]

use std::collections::HashMap;

/// A payload's identity, independent of its window and of the current main binding.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
struct ViewerContextId(u64);

impl ViewerContextId {
    fn serial(self) -> u64 {
        self.0
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ContextResidence {
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
enum ForkPolicy {
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
enum BindError {
    WindowOwnedBy(ViewerContextId),
    ContextOwnedBy(u64),
    WrongOrigin(Option<ViewerContextId>),
    NotBindable(ContextResidence),
}

#[derive(Debug, PartialEq, Eq)]
struct MountError {
    id: ViewerContextId,
    residence: ContextResidence,
}

#[derive(Debug, PartialEq, Eq)]
enum RetireError {
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
        let main = ViewerContextId(1);
        Self {
            projection: Projection::Mounted(main),
            slots: HashMap::new(),
            main,
            window_of: HashMap::new(),
            context_of: HashMap::new(),
            next_serial: 2,
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

    fn bind_window(&mut self, id: ViewerContextId, window_id: u64) -> Result<(), BindError> {
        assert!(self.pending.is_none());
        self.bind_core(id, window_id)
    }

    fn unbind_window(&mut self, window_id: u64) -> Option<ViewerContextId> {
        assert!(self.pending.is_none());
        self.unbind_window_core(window_id)
    }

    fn transfer_window_binding(
        &mut self,
        window_id: u64,
        from: ViewerContextId,
        to: ViewerContextId,
    ) -> Result<(), BindError> {
        assert!(self.pending.is_none());
        self.transfer_core(window_id, from, to)
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
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;
    use std::panic::{AssertUnwindSafe, catch_unwind};
    use std::rc::Rc;

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

        table.transfer_window_binding(20, from, to).unwrap();
        assert_eq!(table.residence(from), ContextResidence::Mounted);
        assert_eq!(table.residence(to), ContextResidence::AtRest);
        assert_eq!(
            table.locate_window_context(20),
            Some((to, ContextResidence::AtRest))
        );
        assert_eq!(
            table.transfer_window_binding(20, from, from),
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
