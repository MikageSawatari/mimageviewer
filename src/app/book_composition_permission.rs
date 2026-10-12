//! Context-owned visible demand and one qualified fullscreen-open continuation.
use super::{App, DeferredFsReopen, FsOpenMaterialization, FsPageLoadContract, HistoryTrigger};
use crate::book_composition::*;
use std::{collections::HashMap, sync::mpsc};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum BookTargetPurpose {
    Explicit,
    Next,
    Previous,
    FirstInBook,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum BookDfsContinuation {
    Navigation { forward: bool },
    Slideshow,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum BookIdentityPermission {
    Checking,
    Allowed,
    Hidden,
    Refused,
}
#[derive(Clone, Debug, PartialEq, Eq)]
enum BookTargetRoute {
    FullscreenOpen,
    FullscreenNavigation,
    ContinuousSeek,
    ZipDfs(BookDfsContinuation),
    DeferredBook(Box<DeferredFsReopen>),
    Slideshow {
        slice: Option<crate::page_split::PageSlice>,
    },
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct OpenIntent {
    context_id: u64,
    items_generation: u64,
    idx: usize,
    history_trigger: HistoryTrigger,
    materialization: FsOpenMaterialization,
    contract: FsPageLoadContract,
    purpose: BookTargetPurpose,
    route: BookTargetRoute,
}
impl OpenIntent {
    fn presentation_demand(
        &self,
        mut demand: Vec<PresentationOccurrence>,
    ) -> Vec<PresentationOccurrence> {
        if let BookTargetRoute::Slideshow { slice: Some(slice) } = &self.route {
            let slice = match slice {
                crate::page_split::PageSlice::Full => PresentationSlice::Full,
                crate::page_split::PageSlice::Left => PresentationSlice::Left,
                crate::page_split::PageSlice::Right => PresentationSlice::Right,
            };
            for occurrence in &mut demand {
                if occurrence.item_index == self.idx
                    && occurrence.role == PresentationRole::Navigation
                {
                    occurrence.slice = slice;
                }
            }
        }
        demand
    }
    fn clear_skipped_source_slice(&mut self) {
        if let BookTargetRoute::Slideshow { slice } = &mut self.route {
            *slice = None;
        }
    }
}
#[derive(Default)]
enum OpenPermission {
    #[default]
    Idle,
    // Only source/hydration completion transfers a deferred reopen here. Before
    // that boundary, the continuation belongs to the source enumeration owner.
    AwaitingProjection {
        context_id: u64,
        items_generation: u64,
        scope: crate::book_scope::BookScopeKey,
        deferred: DeferredFsReopen,
    },
    Checking {
        intent: OpenIntent,
        candidates: Vec<OpenCandidate>,
    },
    Ready {
        intent: OpenIntent,
        candidates: Vec<OpenCandidate>,
    },
    Resuming {
        intent: OpenIntent,
        candidates: Vec<OpenCandidate>,
    },
    IdentityChecking {
        operation_id: u64,
        correlation: CandidateCorrelation,
        candidate: CandidatePage,
        receiver: mpsc::Receiver<CandidatePermissionReply>,
    },
    IdentityComplete {
        operation_id: u64,
        correlation: CandidateCorrelation,
        candidate: CandidatePage,
        result: BookIdentityPermission,
        permission: Option<CandidatePermission>,
    },
    ContinueDfs(BookDfsContinuation),
}
struct OpenCandidate {
    correlation: CandidateCorrelation,
    candidate: CandidatePage,
    state: OpenCandidateState,
}
enum OpenCandidateState {
    Checking(mpsc::Receiver<CandidatePermissionReply>),
    Complete(CandidatePermission),
}
impl OpenCandidate {
    fn occurrence(&self) -> PresentationOccurrence {
        self.correlation
            .occurrence
            .presented()
            .expect("an indexed open owns presentation candidates")
    }
    fn is_current(&self, snapshot: &CompositionSnapshot) -> bool {
        matches!(&self.state, OpenCandidateState::Complete(permission) if permission.is_current(snapshot))
    }
    fn is_allowed(&self) -> bool {
        matches!(
            &self.state,
            OpenCandidateState::Complete(CandidatePermission::Allowed(_))
        )
    }
}
impl PartialEq for OpenCandidate {
    fn eq(&self, other: &Self) -> bool {
        self.correlation == other.correlation
            && match (&self.state, &other.state) {
                (OpenCandidateState::Checking(_), OpenCandidateState::Checking(_)) => true,
                (OpenCandidateState::Complete(a), OpenCandidateState::Complete(b)) => a == b,
                _ => false,
            }
    }
}
enum OccurrencePermission {
    Checking {
        correlation: CandidateCorrelation,
        candidate: CandidatePage,
        receiver: mpsc::Receiver<CandidatePermissionReply>,
    },
    Complete(CandidatePermission),
    Blocked,
}
#[derive(Default)]
pub(crate) struct ViewerPresentationPermission {
    binding: Option<(u64, u64)>,
    sequence: u64,
    occurrences: HashMap<PresentationOccurrence, OccurrencePermission>,
    open: OpenPermission,
}
impl Clone for ViewerPresentationPermission {
    fn clone(&self) -> Self {
        Self {
            binding: None,
            sequence: self.sequence,
            occurrences: self
                .occurrences
                .iter()
                .filter_map(|(occurrence, state)| match state {
                    OccurrencePermission::Complete(
                        permission @ CandidatePermission::Allowed(_),
                    ) => Some((
                        *occurrence,
                        OccurrencePermission::Complete(permission.clone()),
                    )),
                    _ => None,
                })
                .collect(),
            open: OpenPermission::Idle,
        }
    }
}
impl std::fmt::Debug for ViewerPresentationPermission {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ViewerPresentationPermission")
            .field("binding", &self.binding)
            .field("occurrences", &self.occurrences.len())
            .finish()
    }
}
impl PartialEq for ViewerPresentationPermission {
    fn eq(&self, other: &Self) -> bool {
        self.binding == other.binding
            && self.sequence == other.sequence
            && self.occurrences.len() == other.occurrences.len()
            && match (&self.open, &other.open) {
                (OpenPermission::Idle, OpenPermission::Idle) => true,
                (
                    OpenPermission::AwaitingProjection {
                        context_id: a,
                        items_generation: b,
                        scope: c,
                        deferred: d,
                    },
                    OpenPermission::AwaitingProjection {
                        context_id: w,
                        items_generation: x,
                        scope: y,
                        deferred: z,
                    },
                ) => (a, b, c, d) == (w, x, y, z),
                (
                    OpenPermission::Checking {
                        intent: a,
                        candidates: b,
                    },
                    OpenPermission::Checking {
                        intent: x,
                        candidates: y,
                    },
                ) => (a, b) == (x, y),
                (
                    OpenPermission::Ready {
                        intent: a,
                        candidates: b,
                    },
                    OpenPermission::Ready {
                        intent: x,
                        candidates: y,
                    },
                ) => (a, b) == (x, y),
                (
                    OpenPermission::Resuming {
                        intent: a,
                        candidates: b,
                    },
                    OpenPermission::Resuming {
                        intent: x,
                        candidates: y,
                    },
                ) => (a, b) == (x, y),
                (
                    OpenPermission::IdentityChecking {
                        operation_id: a,
                        correlation: b,
                        ..
                    },
                    OpenPermission::IdentityChecking {
                        operation_id: x,
                        correlation: y,
                        ..
                    },
                ) => (a, b) == (x, y),
                (
                    OpenPermission::IdentityComplete {
                        operation_id: a,
                        correlation: b,
                        result: c,
                        permission: d,
                        ..
                    },
                    OpenPermission::IdentityComplete {
                        operation_id: x,
                        correlation: y,
                        result: z,
                        permission: w,
                        ..
                    },
                ) => (a, b, c, d) == (x, y, z, w),
                (OpenPermission::ContinueDfs(a), OpenPermission::ContinueDfs(b)) => a == b,
                _ => false,
            }
            && self.occurrences.iter().all(|(key, state)| {
                match (state, other.occurrences.get(key)) {
                    (
                        OccurrencePermission::Complete(a),
                        Some(OccurrencePermission::Complete(b)),
                    ) => a == b,
                    (
                        OccurrencePermission::Checking { correlation: a, .. },
                        Some(OccurrencePermission::Checking { correlation: b, .. }),
                    ) => a == b,
                    (OccurrencePermission::Blocked, Some(OccurrencePermission::Blocked)) => true,
                    _ => false,
                }
            })
    }
}
impl ViewerPresentationPermission {
    pub(crate) fn pending(&self) -> bool {
        matches!(
            self.open,
            OpenPermission::AwaitingProjection { .. }
                | OpenPermission::Checking { .. }
                | OpenPermission::Ready { .. }
                | OpenPermission::IdentityChecking { .. }
                | OpenPermission::ContinueDfs(_)
        ) || self
            .occurrences
            .values()
            .any(|state| matches!(state, OccurrencePermission::Checking { .. }))
    }
    fn bind(&mut self, context: u64, generation: u64) {
        if self
            .binding
            .is_some_and(|binding| binding != (context, generation))
        {
            self.occurrences.clear();
            self.open = OpenPermission::Idle;
        }
        self.binding = Some((context, generation));
    }
    fn correlation(
        &mut self,
        occurrence: PresentationOccurrence,
        candidate: &CandidatePage,
    ) -> CandidateCorrelation {
        self.correlation_for(CandidateOccurrence::Presented(occurrence), candidate)
    }
    fn correlation_for(
        &mut self,
        occurrence: CandidateOccurrence,
        candidate: &CandidatePage,
    ) -> CandidateCorrelation {
        self.sequence = self
            .sequence
            .checked_add(1)
            .expect("presentation permission request exhausted");
        let (context_id, items_generation) =
            self.binding.expect("presentation owner must be bound");
        CandidateCorrelation {
            context_id,
            items_generation,
            request_id: self.sequence,
            occurrence,
            identity: candidate.identity.clone(),
        }
    }
    fn decision(&self, demand: &[PresentationOccurrence]) -> PresentationDecision {
        let mut allowed = Vec::with_capacity(demand.len());
        for occurrence in demand {
            match self.occurrences.get(occurrence) {
                Some(OccurrencePermission::Complete(CandidatePermission::Allowed(_))) => {
                    allowed.push(true)
                }
                Some(OccurrencePermission::Complete(CandidatePermission::Hidden(_))) => {
                    allowed.push(false)
                }
                _ => return PresentationDecision::Checking,
            }
        }
        if allowed.iter().any(|value| *value) {
            PresentationDecision::Allowed(allowed)
        } else {
            PresentationDecision::Hidden
        }
    }
}

impl App {
    pub(crate) fn defer_book_projection_open(&mut self, deferred: DeferredFsReopen) {
        let scope = self
            .book_composition
            .scope()
            .cloned()
            .expect("pending book projection has a scope");
        let context_id = self.spread_shift_context_id().serial();
        self.book_composition_permission
            .bind(context_id, self.items_generation);
        self.book_composition_permission.open = OpenPermission::AwaitingProjection {
            context_id,
            items_generation: self.items_generation,
            scope,
            deferred,
        };
    }

    pub(crate) fn book_deferred_open_wait_active(&self) -> bool {
        match &self.book_composition_permission.open {
            OpenPermission::AwaitingProjection {
                context_id,
                items_generation,
                ..
            } => {
                *context_id == self.spread_shift_context_id().serial()
                    && *items_generation == self.items_generation
            }
            OpenPermission::Checking { intent, .. }
            | OpenPermission::Ready { intent, .. }
            | OpenPermission::Resuming { intent, .. } => {
                matches!(intent.route, BookTargetRoute::DeferredBook(_))
                    && intent.context_id == self.spread_shift_context_id().serial()
                    && intent.items_generation == self.items_generation
            }
            _ => false,
        }
    }

    pub(crate) fn retire_book_deferred_open(&mut self) {
        if matches!(
            &self.book_composition_permission.open,
            OpenPermission::AwaitingProjection { .. }
        ) || matches!(&self.book_composition_permission.open,
            OpenPermission::Checking { intent, .. }
                | OpenPermission::Ready { intent, .. }
                | OpenPermission::Resuming { intent, .. }
                if matches!(intent.route, BookTargetRoute::DeferredBook(_))
        ) {
            self.book_composition_permission.open = OpenPermission::Idle;
        }
    }

    /// Row replacement retires old presentation proofs but carries the source's
    /// identity-based reopen through this exact scope/generation adoption.
    pub(crate) fn retire_book_permission_after_projection(
        &mut self,
        scope: &crate::book_scope::BookScopeKey,
        previous_generation: u64,
        generation: u64,
    ) {
        let waiting = std::mem::take(&mut self.book_composition_permission.open);
        self.retire_book_presentation_permission();
        self.carry_deferred_book_projection(waiting, scope, previous_generation, generation);
    }

    fn carry_deferred_book_projection(
        &mut self,
        waiting: OpenPermission,
        accepted_scope: &crate::book_scope::BookScopeKey,
        previous_generation: u64,
        generation: u64,
    ) {
        if let OpenPermission::AwaitingProjection {
            context_id,
            items_generation,
            scope,
            deferred,
        } = waiting
        {
            if context_id == self.spread_shift_context_id().serial()
                && items_generation == previous_generation
                && scope == *accepted_scope
            {
                self.book_composition_permission.binding = Some((context_id, generation));
                self.book_composition_permission.open = OpenPermission::AwaitingProjection {
                    context_id,
                    items_generation: generation,
                    scope,
                    deferred,
                };
            }
        }
    }

    pub(crate) fn presentation_candidate(&self, idx: usize) -> Option<CandidatePage> {
        if matches!(
            self.book_composition,
            super::book_composition_runtime::ViewerBookComposition::Ignored { .. }
        ) {
            return None;
        }
        let item = self.items.get(idx)?;
        // Folder compositions are outside the viewing scope. Old/imported
        // ImageFolder rows remain stored, but ordinary images need no receipt.
        if matches!(item, crate::grid_item::GridItem::Image(_)) {
            return None;
        }
        let logical = match item {
            crate::grid_item::GridItem::ZipImage { zip_path, .. }
                if self.zip_nav.as_ref().is_some_and(|nav| {
                    crate::path_key::normalize_keep_drive(&nav.tree.zip_path)
                        == crate::path_key::normalize_keep_drive(zip_path)
                }) || self.current_folder.as_ref().is_some_and(|folder| {
                    crate::path_key::normalize_keep_drive(folder)
                        == crate::path_key::normalize_keep_drive(zip_path)
                }) =>
            {
                self.archive_source_override.as_deref()
            }
            _ => None,
        };
        let identity = crate::book_resume_db::PageIdentity::for_item(item, logical)?;
        let physical_archive = match item {
            crate::grid_item::GridItem::ZipImage { zip_path, .. } => Some(zip_path.clone()),
            _ => None,
        };
        let raw_tree = self
            .zip_nav
            .as_ref()
            .filter(|nav| {
                physical_archive.as_ref().is_some_and(|path| {
                    crate::path_key::normalize_keep_drive(path)
                        == crate::path_key::normalize_keep_drive(&nav.tree.zip_path)
                })
            })
            .map(|nav| nav.tree.clone());
        Some(CandidatePage {
            identity,
            physical_archive,
            raw_tree,
        })
    }

    /// Hidden occurrences were skipped, never presented. A new viewport search
    /// retires those exclusions, while overlapping visible/pending occurrences
    /// keep their requests until the measured visible-demand diff retires them.
    pub(crate) fn retire_hidden_book_presentation_demand(&mut self) {
        self.book_composition_permission
            .occurrences
            .retain(|_, state| {
                !matches!(
                    state,
                    OccurrencePermission::Complete(CandidatePermission::Hidden(_))
                )
            });
    }

    pub(crate) fn bind_book_presentation_demand(&mut self) {
        let context = self.spread_shift_context_id().serial();
        self.book_composition_permission
            .bind(context, self.items_generation);
    }

    pub(crate) fn retain_book_presentation_demand(&mut self, demand: &[PresentationOccurrence]) {
        self.bind_book_presentation_demand();
        self.book_composition_permission
            .occurrences
            .retain(|key, _| demand.contains(key));
        for occurrence in demand {
            if self
                .book_composition_permission
                .occurrences
                .contains_key(occurrence)
            {
                continue;
            }
            let state = if let (Some(candidate), Some(service)) = (
                self.presentation_candidate(occurrence.item_index),
                self.book_composition_service.as_ref(),
            ) {
                let correlation = self
                    .book_composition_permission
                    .correlation(*occurrence, &candidate);
                let receiver = service.check_candidate(correlation.clone(), candidate.clone());
                OccurrencePermission::Checking {
                    correlation,
                    candidate,
                    receiver,
                }
            } else {
                // Ordinary images, videos/audio and an application without a composition service
                // have no composition-page permission to acquire.
                OccurrencePermission::Complete(CandidatePermission::Allowed(None))
            };
            self.book_composition_permission
                .occurrences
                .insert(*occurrence, state);
        }
        self.poll_visible_presentation_permission();
    }
    pub(crate) fn book_presentation_allowed(
        &self,
        demand: &[PresentationOccurrence],
    ) -> PresentationDecision {
        if (self.book_composition.is_pending()
            || self.pairing_edit_availability()
                == crate::spread_shift::PairingEditAvailability::Unreadable)
            && self.composed_book_layout().is_none()
        {
            return PresentationDecision::Checking;
        }
        if self.book_composition_service.is_none() {
            let mask = demand
                .iter()
                .map(|occurrence| {
                    !matches!(
                        self.book_composition_permission.occurrences.get(occurrence),
                        Some(OccurrencePermission::Complete(CandidatePermission::Hidden(
                            _
                        )))
                    )
                })
                .collect::<Vec<_>>();
            return if mask.iter().any(|allowed| *allowed) {
                PresentationDecision::Allowed(mask)
            } else {
                PresentationDecision::Hidden
            };
        }
        self.book_composition_permission.decision(demand)
    }
    pub(crate) fn book_presentation_current(
        &self,
        demand: &[PresentationOccurrence],
    ) -> PresentationDecision {
        self.book_presentation_allowed(demand)
    }
    pub(crate) fn book_presentation_demand_snapshot(&self) -> Vec<PresentationOccurrence> {
        self.book_composition_permission
            .occurrences
            .keys()
            .copied()
            .collect()
    }
    #[cfg(test)]
    pub(crate) fn install_hidden_presentation_for_test(
        &mut self,
        demand: &[PresentationOccurrence],
    ) {
        self.book_composition_permission.bind(
            self.spread_shift_context_id().serial(),
            self.items_generation,
        );
        for occurrence in demand {
            let scope = self
                .presentation_test_scope(occurrence.item_index)
                .unwrap_or_else(|| {
                    crate::book_scope::BookScopeKey::document("C:/presentation-test.pdf").unwrap()
                });
            let token = self
                .book_composition_service
                .as_ref()
                .map(|service| service.snapshot().token_for(&scope))
                .unwrap_or(CompositionToken {
                    service_epoch: 0,
                    publication_sequence: 0,
                });
            self.book_composition_permission.occurrences.insert(
                *occurrence,
                OccurrencePermission::Complete(CandidatePermission::Hidden(ScopePermissionToken {
                    scope: scope.clone(),
                    token,
                })),
            );
        }
    }
    #[cfg(test)]
    pub(crate) fn install_checking_presentation_for_test(
        &mut self,
        demand: &[PresentationOccurrence],
    ) -> Vec<mpsc::Sender<CandidatePermissionReply>> {
        self.book_composition_permission.bind(
            self.spread_shift_context_id().serial(),
            self.items_generation,
        );
        demand
            .iter()
            .map(|occurrence| {
                let candidate = self
                    .presentation_candidate(occurrence.item_index)
                    .expect("test candidate");
                let correlation = self
                    .book_composition_permission
                    .correlation(*occurrence, &candidate);
                let (sender, receiver) = mpsc::channel();
                self.book_composition_permission.occurrences.insert(
                    *occurrence,
                    OccurrencePermission::Checking {
                        correlation,
                        candidate,
                        receiver,
                    },
                );
                sender
            })
            .collect()
    }
    #[cfg(test)]
    pub(crate) fn checking_presentation_correlations_for_test(
        &self,
        demand: &[PresentationOccurrence],
    ) -> Vec<CandidateCorrelation> {
        demand
            .iter()
            .map(
                |occurrence| match self.book_composition_permission.occurrences.get(occurrence) {
                    Some(OccurrencePermission::Checking { correlation, .. }) => correlation.clone(),
                    _ => panic!("test demand must own an exact Checking occurrence"),
                },
            )
            .collect()
    }
    #[cfg(test)]
    fn presentation_test_scope(&self, idx: usize) -> Option<crate::book_scope::BookScopeKey> {
        let candidate = self.presentation_candidate(idx)?;
        // The shared resolver is pure for documents/images and an already available raw tree.
        // Never let a test fixture perform the resolver's worker-only archive enumeration.
        let can_resolve = match &candidate.identity {
            crate::book_resume_db::PageIdentity::Archive { container, .. } => {
                candidate.raw_tree.as_ref().is_some_and(|tree| {
                    let physical = candidate
                        .physical_archive
                        .as_deref()
                        .unwrap_or_else(|| std::path::Path::new(container));
                    crate::path_key::normalize_keep_drive(&tree.zip_path)
                        == crate::path_key::normalize_keep_drive(physical)
                })
            }
            _ => true,
        };
        can_resolve
            .then(|| {
                resolve_candidate_scope(&candidate)
                    .ok()
                    .map(|(scope, _, _)| scope)
            })
            .flatten()
            .or_else(|| self.current_spread_shift_scope())
    }
    #[cfg(test)]
    pub(crate) fn install_allowed_presentation_for_test(
        &mut self,
        demand: &[PresentationOccurrence],
    ) {
        self.book_composition_permission.bind(
            self.spread_shift_context_id().serial(),
            self.items_generation,
        );
        for occurrence in demand {
            let scope = self.presentation_test_scope(occurrence.item_index);
            let proof = scope.and_then(|scope| {
                self.book_composition_service
                    .as_ref()
                    .map(|service| ScopePermissionToken {
                        token: service.snapshot().token_for(&scope),
                        scope,
                    })
            });
            self.book_composition_permission.occurrences.insert(
                *occurrence,
                OccurrencePermission::Complete(CandidatePermission::Allowed(proof)),
            );
        }
    }
    #[cfg(test)]
    pub(crate) fn presentation_demand_count_for_test(&self) -> usize {
        self.book_composition_permission.occurrences.len()
    }
    pub(crate) fn retire_book_presentation_permission(&mut self) {
        self.book_composition_permission = ViewerPresentationPermission::default();
        self.spread_display_units_cache
            .retire_continuous_admission();
    }
    pub(crate) fn retire_book_identity_permission(&mut self, operation_id: u64) {
        if matches!(&self.book_composition_permission.open, OpenPermission::IdentityChecking { operation_id: requested, .. } | OpenPermission::IdentityComplete { operation_id: requested, .. } if *requested == operation_id)
        {
            self.book_composition_permission.open = OpenPermission::Idle;
        }
    }
    pub(crate) fn has_book_dfs_continuation(&self) -> bool {
        matches!(
            self.book_composition_permission.open,
            OpenPermission::ContinueDfs(_)
        )
    }
    pub(crate) fn has_book_dfs_open_continuation(&self) -> bool {
        matches!(&self.book_composition_permission.open, OpenPermission::Checking { intent, .. } | OpenPermission::Ready { intent, .. } | OpenPermission::Resuming { intent, .. } if matches!(&intent.route, BookTargetRoute::ZipDfs(_)))
    }
    pub(crate) fn has_book_slideshow_continuation(&self) -> bool {
        matches!(&self.book_composition_permission.open, OpenPermission::Checking { intent, .. } | OpenPermission::Ready { intent, .. } | OpenPermission::Resuming { intent, .. } if matches!(&intent.route, BookTargetRoute::Slideshow { .. }))
    }
    /// The existing external-open request owns its captured continuation. This
    /// owner qualifies its logical identity before any source or selection change.
    pub(crate) fn qualify_book_identity_permission(
        &mut self,
        ctx: &egui::Context,
        operation_id: u64,
        candidate: CandidatePage,
    ) -> BookIdentityPermission {
        if let super::book_composition_runtime::ViewerBookComposition::Ignored { scope, .. } =
            &self.book_composition
            && composition_page_from_identity(scope, &candidate.identity).is_some_and(|page| {
                let domain = if scope.level_prefix.is_empty()
                    && matches!(page, CompositionPageRef::PdfPage { .. })
                {
                    PageDomain::Document
                } else {
                    PageDomain::ArchiveLevel {
                        prefix: scope.level_prefix.clone(),
                    }
                };
                domain.validate_page(&page).is_ok()
            })
        {
            return BookIdentityPermission::Allowed;
        }
        let context_id = self.spread_shift_context_id().serial();
        self.book_composition_permission
            .bind(context_id, self.items_generation);
        let Some(service) = self.book_composition_service.clone() else {
            return BookIdentityPermission::Allowed;
        };
        let state = std::mem::take(&mut self.book_composition_permission.open);
        let matching = match &state {
            OpenPermission::IdentityChecking {
                operation_id: requested,
                correlation,
                candidate: captured,
                ..
            }
            | OpenPermission::IdentityComplete {
                operation_id: requested,
                correlation,
                candidate: captured,
                ..
            } => {
                *requested == operation_id
                    && correlation.context_id == context_id
                    && correlation.items_generation == self.items_generation
                    && captured.identity == candidate.identity
                    && captured.physical_archive == candidate.physical_archive
            }
            _ => false,
        };
        let state = if matching {
            state
        } else {
            let correlation = self
                .book_composition_permission
                .correlation_for(CandidateOccurrence::IdentityOnly, &candidate);
            let receiver = service.check_candidate(correlation.clone(), candidate.clone());
            OpenPermission::IdentityChecking {
                operation_id,
                correlation,
                candidate,
                receiver,
            }
        };
        let state = match state {
            OpenPermission::IdentityComplete {
                operation_id,
                correlation,
                candidate,
                result,
                permission,
            } => {
                if permission
                    .as_ref()
                    .is_none_or(|permission| permission.is_current(&service.snapshot()))
                {
                    self.book_composition_permission.open = OpenPermission::IdentityComplete {
                        operation_id,
                        correlation,
                        candidate,
                        result,
                        permission,
                    };
                    return result;
                }
                let receiver = service.check_candidate(correlation.clone(), candidate.clone());
                OpenPermission::IdentityChecking {
                    operation_id,
                    correlation,
                    candidate,
                    receiver,
                }
            }
            state => state,
        };
        let OpenPermission::IdentityChecking {
            operation_id,
            correlation,
            candidate,
            receiver,
        } = state
        else {
            unreachable!()
        };
        let reply = match receiver.try_recv() {
            Ok(reply) => reply,
            Err(mpsc::TryRecvError::Empty) => {
                self.book_composition_permission.open = OpenPermission::IdentityChecking {
                    operation_id,
                    correlation,
                    candidate,
                    receiver,
                };
                ctx.request_repaint_after_for(
                    std::time::Duration::from_millis(50),
                    egui::ViewportId::ROOT,
                );
                return BookIdentityPermission::Checking;
            }
            Err(mpsc::TryRecvError::Disconnected) => CandidatePermissionReply {
                correlation: correlation.clone(),
                status: Err(CandidatePermissionError::Unavailable(
                    "composition identity worker stopped".into(),
                )),
            },
        };
        if reply.correlation != correlation {
            return BookIdentityPermission::Refused;
        }
        let (result, permission) = match reply.status {
            Ok(permission) if permission.is_current(&service.snapshot()) => {
                let result = if matches!(permission, CandidatePermission::Hidden(_)) {
                    BookIdentityPermission::Hidden
                } else {
                    BookIdentityPermission::Allowed
                };
                if result == BookIdentityPermission::Hidden {
                    self.show_feedback_toast("本の構成で非表示に設定されています".into());
                }
                (result, Some(permission))
            }
            Ok(_) => {
                let receiver = service.check_candidate(correlation.clone(), candidate.clone());
                self.book_composition_permission.open = OpenPermission::IdentityChecking {
                    operation_id,
                    correlation,
                    candidate,
                    receiver,
                };
                ctx.request_repaint();
                return BookIdentityPermission::Checking;
            }
            Err(error @ CandidatePermissionError::UnsupportedSource(_)) => {
                self.notify_composition_permission_fallback(error);
                (
                    BookIdentityPermission::Allowed,
                    Some(CandidatePermission::Allowed(None)),
                )
            }
            Err(error @ CandidatePermissionError::Unavailable(_)) => {
                self.notify_composition_permission_fallback(error);
                (BookIdentityPermission::Refused, None)
            }
        };
        self.book_composition_permission.open = OpenPermission::IdentityComplete {
            operation_id,
            correlation,
            candidate,
            result,
            permission,
        };
        result
    }
    /// Called in the same adoption as new rows, before numeric-slot continuations
    /// can run. A pending explicit target remains its captured logical identity.
    pub(crate) fn remap_after_book_projection(
        &mut self,
        projection: &AcceptedCompositionProjection,
        previous_generation: u64,
        new_generation: u64,
        refresh_landings: &HashMap<usize, Option<usize>>,
    ) {
        let open = std::mem::take(&mut self.book_composition_permission.open);
        self.book_composition_permission.occurrences.clear();
        self.book_composition_permission.binding =
            Some((self.spread_shift_context_id().serial(), new_generation));
        if matches!(&open, OpenPermission::AwaitingProjection { .. }) {
            self.carry_deferred_book_projection(
                open,
                &projection.scope,
                previous_generation,
                new_generation,
            );
            return;
        }
        let (mut intent, candidates) = match open {
            OpenPermission::Checking { intent, candidates }
            | OpenPermission::Ready { intent, candidates }
            | OpenPermission::Resuming { intent, candidates } => (intent, candidates),
            _ => return,
        };
        if intent.purpose == BookTargetPurpose::FirstInBook {
            if let Some(item_index) = projection
                .resolved
                .visible_pages
                .first()
                .map(|page| page.item_index)
            {
                intent.idx = item_index;
                intent.items_generation = new_generation;
                let demand =
                    self.book_target_presentation_demand_from_projection(projection, item_index);
                self.begin_qualified_book_unit(intent, demand);
            } else if let BookTargetRoute::ZipDfs(continuation) = &intent.route {
                self.book_composition_permission.open = OpenPermission::ContinueDfs(*continuation);
            }
            return;
        }
        let Some(target) = candidates.iter().find(|candidate| {
            candidate.occurrence().item_index == intent.idx
                && candidate.occurrence().role == PresentationRole::Navigation
        }) else {
            return;
        };
        let position = crate::book_resume_db::ReadingPosition {
            page: 0,
            identity: Some(target.correlation.identity.clone()),
        };
        match projection.resolve_position(&position) {
            PageStatus::Visible { item_index } => {
                intent.idx = item_index;
                intent.items_generation = new_generation;
                let demand =
                    self.book_target_presentation_demand_from_projection(projection, item_index);
                self.begin_qualified_book_unit(intent, demand);
            }
            PageStatus::HiddenByComposition if intent.purpose == BookTargetPurpose::Explicit => {
                self.handle_deferred_fs_open_outcome(
                    super::DeferredFsOpenOutcome::RequiredTargetHidden,
                )
            }
            PageStatus::HiddenByComposition => {
                let survivor = candidates
                    .iter()
                    .filter(|candidate| candidate.occurrence().role == PresentationRole::Navigation)
                    .find_map(|candidate| {
                        let position = crate::book_resume_db::ReadingPosition {
                            page: 0,
                            identity: Some(candidate.correlation.identity.clone()),
                        };
                        match projection.resolve_position(&position) {
                            PageStatus::Visible { item_index } => Some(item_index),
                            _ => None,
                        }
                    })
                    .or_else(|| refresh_landings.get(&intent.idx).copied().flatten());
                if let Some(item_index) = survivor {
                    intent.clear_skipped_source_slice();
                    intent.idx = item_index;
                    intent.items_generation = new_generation;
                    let demand = self
                        .book_target_presentation_demand_from_projection(projection, item_index);
                    self.begin_qualified_book_unit(intent, demand);
                } else {
                    self.close_fullscreen();
                }
            }
            _ => self.show_feedback_toast("開こうとしたページが見つかりません".into()),
        }
    }

    fn poll_visible_presentation_permission(&mut self) {
        let Some(service) = self.book_composition_service.clone() else {
            return;
        };
        let snapshot = service.snapshot();
        let mut errors = Vec::new();
        for state in self.book_composition_permission.occurrences.values_mut() {
            let OccurrencePermission::Checking {
                correlation,
                candidate,
                receiver,
            } = state
            else {
                continue;
            };
            let reply = match receiver.try_recv() {
                Ok(reply) => reply,
                Err(mpsc::TryRecvError::Empty) => continue,
                Err(mpsc::TryRecvError::Disconnected) => {
                    errors.push(CandidatePermissionError::Unavailable(
                        "composition permission worker stopped".into(),
                    ));
                    *state = OccurrencePermission::Blocked;
                    continue;
                }
            };
            if reply.correlation != *correlation {
                continue;
            }
            match reply.status {
                Ok(permission) if permission.is_current(&snapshot) => {
                    *state = OccurrencePermission::Complete(permission)
                }
                Ok(_) => {
                    *receiver = service.check_candidate(correlation.clone(), candidate.clone())
                }
                Err(CandidatePermissionError::UnsupportedSource(error)) => {
                    errors.push(CandidatePermissionError::UnsupportedSource(error));
                    *state = OccurrencePermission::Complete(CandidatePermission::Allowed(None));
                }
                Err(CandidatePermissionError::Unavailable(error)) => {
                    errors.push(CandidatePermissionError::Unavailable(error));
                    *state = OccurrencePermission::Blocked;
                }
            }
        }
        for error in errors {
            self.notify_composition_permission_fallback(error);
        }
    }
    fn notify_composition_permission_fallback(&mut self, error: CandidatePermissionError) {
        match error {
            CandidatePermissionError::UnsupportedSource(error) => {
                crate::logger::log(format!(
                    "book_composition: unsupported candidate source: {error}"
                ));
                self.show_feedback_toast(UNSUPPORTED_COMPOSITION_NOTICE.into());
            }
            CandidatePermissionError::Unavailable(error) => {
                crate::logger::log(format!(
                    "book_composition: candidate permission unavailable: {error}"
                ));
                self.show_feedback_toast("本の構成を確認できませんでした".into());
            }
        }
    }

    pub(crate) fn defer_book_composition_fullscreen_open(
        &mut self,
        idx: usize,
        history_trigger: HistoryTrigger,
        materialization: FsOpenMaterialization,
        contract: FsPageLoadContract,
        purpose: BookTargetPurpose,
    ) -> bool {
        self.defer_book_composition_target_route(
            idx,
            history_trigger,
            materialization,
            contract,
            purpose,
            BookTargetRoute::FullscreenOpen,
        )
    }
    pub(crate) fn defer_book_composition_navigation_open(
        &mut self,
        idx: usize,
        history_trigger: HistoryTrigger,
        materialization: FsOpenMaterialization,
        contract: FsPageLoadContract,
        purpose: BookTargetPurpose,
    ) -> bool {
        self.defer_book_composition_target_route(
            idx,
            history_trigger,
            materialization,
            contract,
            purpose,
            BookTargetRoute::FullscreenNavigation,
        )
    }
    pub(crate) fn defer_book_composition_slideshow_open(
        &mut self,
        idx: usize,
        slice: Option<crate::page_split::PageSlice>,
        purpose: BookTargetPurpose,
    ) -> bool {
        self.defer_book_composition_target_route(
            idx,
            HistoryTrigger::AutoAdvance,
            FsOpenMaterialization::Eager,
            FsPageLoadContract::Sequential,
            purpose,
            BookTargetRoute::Slideshow { slice },
        )
    }
    pub(crate) fn defer_book_composition_slideshow_slice(
        &mut self,
        idx: usize,
        slice: crate::page_split::PageSlice,
        purpose: BookTargetPurpose,
    ) -> bool {
        if self.defer_book_composition_slideshow_open(idx, Some(slice), purpose) {
            return true;
        }
        // Same-source half movement has no fullscreen-open/materialization continuation.
        // Consume its exact current proof at the slice commit boundary instead.
        let open = std::mem::take(&mut self.book_composition_permission.open);
        match open {
            OpenPermission::Ready { intent, candidates }
            | OpenPermission::Resuming { intent, candidates }
                if intent.idx == idx
                    && matches!(&intent.route, BookTargetRoute::Slideshow { slice: Some(requested) } if *requested == slice) =>
            {
                self.publish_qualified_book_candidates(&candidates);
            }
            other => self.book_composition_permission.open = other,
        }
        false
    }
    pub(crate) fn defer_book_composition_continuous_seek(
        &mut self,
        idx: usize,
        contract: FsPageLoadContract,
        purpose: BookTargetPurpose,
    ) -> bool {
        self.defer_book_composition_target_route(
            idx,
            HistoryTrigger::UserChosen,
            FsOpenMaterialization::Eager,
            contract,
            purpose,
            BookTargetRoute::ContinuousSeek,
        )
    }
    pub(crate) fn defer_book_composition_zip_dfs_open(
        &mut self,
        idx: usize,
        continuation: BookDfsContinuation,
    ) -> bool {
        self.defer_book_composition_target_route(
            idx,
            HistoryTrigger::UserChosen,
            FsOpenMaterialization::Eager,
            FsPageLoadContract::Sequential,
            BookTargetPurpose::FirstInBook,
            BookTargetRoute::ZipDfs(continuation),
        )
    }
    pub(crate) fn defer_book_composition_deferred_open(
        &mut self,
        idx: usize,
        deferred: DeferredFsReopen,
        purpose: BookTargetPurpose,
    ) -> bool {
        self.defer_book_composition_target_route(
            idx,
            deferred.history_trigger,
            FsOpenMaterialization::Eager,
            FsPageLoadContract::Sequential,
            purpose,
            BookTargetRoute::DeferredBook(Box::new(deferred)),
        )
    }
    fn begin_qualified_book_unit(
        &mut self,
        intent: OpenIntent,
        demand: Vec<PresentationOccurrence>,
    ) {
        let Some(service) = self.book_composition_service.clone() else {
            return;
        };
        let demand = intent.presentation_demand(demand);
        let mut candidates = Vec::with_capacity(demand.len());
        for occurrence in demand {
            let Some(candidate) = self.presentation_candidate(occurrence.item_index) else {
                self.show_feedback_toast("開こうとしたページが見つかりません".into());
                self.book_composition_permission.open = OpenPermission::Idle;
                return;
            };
            let correlation = self
                .book_composition_permission
                .correlation(occurrence, &candidate);
            let receiver = service.check_candidate(correlation.clone(), candidate.clone());
            candidates.push(OpenCandidate {
                correlation,
                candidate,
                state: OpenCandidateState::Checking(receiver),
            });
        }
        self.book_composition_permission.open = OpenPermission::Checking { intent, candidates };
    }

    fn defer_book_composition_target_route(
        &mut self,
        idx: usize,
        history_trigger: HistoryTrigger,
        materialization: FsOpenMaterialization,
        contract: FsPageLoadContract,
        purpose: BookTargetPurpose,
        route: BookTargetRoute,
    ) -> bool {
        let context_id = self.spread_shift_context_id().serial();
        self.book_composition_permission
            .bind(context_id, self.items_generation);
        if !self.book_composition.is_pending()
            && self.pairing_edit_availability()
                == crate::spread_shift::PairingEditAvailability::Unreadable
            && self.composed_book_layout().is_none()
        {
            self.book_composition_permission.open = OpenPermission::Idle;
            return true;
        }
        if self.book_composition_service.is_none() || self.presentation_candidate(idx).is_none() {
            return false;
        }
        let mut intent = OpenIntent {
            context_id,
            items_generation: self.items_generation,
            idx,
            history_trigger,
            materialization,
            contract,
            purpose,
            route: route.clone(),
        };
        // Internal common-open callbacks retain their typed continuation on a stale-proof
        // recheck. External opens can supersede Checking; Resuming marks this callback.
        let captured = match &self.book_composition_permission.open {
            OpenPermission::Resuming {
                intent: accepted, ..
            } if matches!(&route, BookTargetRoute::FullscreenOpen) => Some(accepted),
            OpenPermission::Checking {
                intent: accepted, ..
            }
            | OpenPermission::Ready {
                intent: accepted, ..
            }
            | OpenPermission::Resuming {
                intent: accepted, ..
            } if matches!(
                &route,
                BookTargetRoute::FullscreenOpen | BookTargetRoute::FullscreenNavigation
            ) && matches!(&accepted.route, BookTargetRoute::Slideshow { .. }) =>
            {
                Some(accepted)
            }
            _ => None,
        };
        if let Some(accepted) = captured {
            if accepted.context_id == intent.context_id
                && accepted.items_generation == intent.items_generation
                && accepted.idx == intent.idx
                && accepted.history_trigger == intent.history_trigger
            {
                intent = accepted.clone();
            }
        }
        let demand = intent.presentation_demand(self.book_target_presentation_demand(idx));
        if matches!(&self.book_composition_permission.open, OpenPermission::Ready { intent: accepted, candidates } | OpenPermission::Resuming { intent: accepted, candidates } if accepted.context_id == intent.context_id && accepted.items_generation == intent.items_generation && accepted.idx == intent.idx && accepted.history_trigger == intent.history_trigger && candidates.iter().map(|candidate| candidate.occurrence()).eq(demand.iter().copied()))
        {
            if self.book_composition.is_pending() {
                return true;
            }
            let snapshot = self.book_composition_service.as_ref().unwrap().snapshot();
            let current = matches!(&self.book_composition_permission.open, OpenPermission::Ready { candidates, .. } | OpenPermission::Resuming { candidates, .. } if candidates.iter().all(|candidate| candidate.is_current(&snapshot)));
            if !current {
                self.begin_qualified_book_unit(intent, demand);
                return true;
            }
            if !matches!(
                route,
                BookTargetRoute::FullscreenNavigation
                    | BookTargetRoute::ZipDfs(_)
                    | BookTargetRoute::DeferredBook(_)
                    | BookTargetRoute::Slideshow { .. }
            ) {
                self.book_composition_permission.open = OpenPermission::Idle;
            }
            return false;
        }
        if matches!(&self.book_composition_permission.open, OpenPermission::Checking { intent: pending, candidates } if *pending == intent && candidates.iter().map(|candidate| candidate.occurrence()).eq(demand.iter().copied()))
        {
            return true;
        }
        self.begin_qualified_book_unit(intent, demand);
        true
    }

    pub(crate) fn poll_book_presentation_permission(&mut self, ctx: &egui::Context) {
        self.poll_visible_presentation_permission();
        let open = std::mem::take(&mut self.book_composition_permission.open);
        let (mut intent, mut candidates) = match open {
            waiting @ OpenPermission::AwaitingProjection { .. } => {
                let OpenPermission::AwaitingProjection {
                    context_id,
                    items_generation,
                    ref scope,
                    ..
                } = waiting
                else {
                    unreachable!()
                };
                if context_id != self.spread_shift_context_id().serial()
                    || items_generation != self.items_generation
                    || self.book_composition.scope() != Some(scope)
                {
                    return;
                }
                if self.book_composition.is_pending() || self.sidecar_restore_active() {
                    self.book_composition_permission.open = waiting;
                    return;
                }
                let OpenPermission::AwaitingProjection { deferred, .. } = waiting else {
                    unreachable!()
                };
                self.complete_deferred_book_composition_open(deferred);
                return;
            }
            OpenPermission::Checking { intent, candidates }
            | OpenPermission::Ready { intent, candidates } => (intent, candidates),
            state @ OpenPermission::Resuming { .. } => {
                self.book_composition_permission.open = state;
                return;
            }
            state @ (OpenPermission::IdentityChecking { .. }
            | OpenPermission::IdentityComplete { .. }) => {
                self.book_composition_permission.open = state;
                return;
            }
            OpenPermission::ContinueDfs(continuation) => {
                self.continue_book_zip_dfs_after_hidden(ctx, continuation);
                return;
            }
            OpenPermission::Idle => return,
        };
        if intent.context_id != self.spread_shift_context_id().serial()
            || intent.items_generation != self.items_generation
        {
            return;
        }
        if matches!(&intent.route, BookTargetRoute::Slideshow { .. }) && !self.slideshow_playing {
            return;
        }
        let Some(service) = self.book_composition_service.clone() else {
            return;
        };
        let snapshot = service.snapshot();
        let mut complete = true;
        for candidate in &mut candidates {
            match &candidate.state {
                OpenCandidateState::Complete(_) if candidate.is_current(&snapshot) => continue,
                OpenCandidateState::Complete(_) => {
                    candidate.state = OpenCandidateState::Checking(service.check_candidate(
                        candidate.correlation.clone(),
                        candidate.candidate.clone(),
                    ));
                    complete = false;
                    continue;
                }
                OpenCandidateState::Checking(_) => {}
            }
            let OpenCandidateState::Checking(receiver) = &candidate.state else {
                unreachable!()
            };
            let reply = match receiver.try_recv() {
                Ok(reply) => reply,
                Err(mpsc::TryRecvError::Empty) => {
                    complete = false;
                    continue;
                }
                Err(mpsc::TryRecvError::Disconnected) => {
                    self.notify_composition_permission_fallback(
                        CandidatePermissionError::Unavailable(
                            "composition permission worker stopped".into(),
                        ),
                    );
                    return;
                }
            };
            if reply.correlation != candidate.correlation {
                return;
            }
            let permission = match reply.status {
                Ok(permission) if permission.is_current(&snapshot) => permission,
                Ok(_) => {
                    candidate.state = OpenCandidateState::Checking(service.check_candidate(
                        candidate.correlation.clone(),
                        candidate.candidate.clone(),
                    ));
                    complete = false;
                    continue;
                }
                Err(CandidatePermissionError::UnsupportedSource(error)) => {
                    self.notify_composition_permission_fallback(
                        CandidatePermissionError::UnsupportedSource(error),
                    );
                    CandidatePermission::Allowed(None)
                }
                Err(CandidatePermissionError::Unavailable(error)) => {
                    self.notify_composition_permission_fallback(
                        CandidatePermissionError::Unavailable(error),
                    );
                    return;
                }
            };
            candidate.state = OpenCandidateState::Complete(permission);
        }
        if !complete {
            self.book_composition_permission.open = OpenPermission::Checking { intent, candidates };
            ctx.request_repaint_after_for(
                std::time::Duration::from_millis(16),
                egui::ViewportId::ROOT,
            );
            return;
        }
        // Hidden navigation is resolved in the accepted unit order, too. A
        // fresh projection may reorder the next real unit, so keep its captured
        // identity until the same adoption remaps this continuation.
        if self.book_composition.is_pending() {
            self.book_composition_permission.open = OpenPermission::Ready { intent, candidates };
            return;
        }
        if self.pairing_edit_availability()
            == crate::spread_shift::PairingEditAvailability::Unreadable
            && self.composed_book_layout().is_none()
        {
            return;
        }
        let target_allowed = candidates.iter().any(|candidate| {
            candidate.occurrence().item_index == intent.idx
                && candidate.occurrence().role == PresentationRole::Navigation
                && candidate.is_allowed()
        });
        if !target_allowed {
            match intent.purpose {
                BookTargetPurpose::Explicit => self.handle_deferred_fs_open_outcome(
                    super::DeferredFsOpenOutcome::RequiredTargetHidden,
                ),
                BookTargetPurpose::Next
                | BookTargetPurpose::Previous
                | BookTargetPurpose::FirstInBook => {
                    // A permitted real in this exact unit survives in its original
                    // slot. Supplements never become navigation destinations.
                    if let Some(survivor) = candidates
                        .iter()
                        .find(|candidate| {
                            candidate.occurrence().role == PresentationRole::Navigation
                                && candidate.is_allowed()
                        })
                        .map(|candidate| candidate.occurrence().item_index)
                    {
                        intent.clear_skipped_source_slice();
                        intent.idx = survivor;
                        let demand = self.book_target_presentation_demand(survivor);
                        self.begin_qualified_book_unit(intent, demand);
                    } else {
                        let dir = if intent.purpose == BookTargetPurpose::Previous {
                            -1
                        } else {
                            1
                        };
                        let nav = self.get_nav_indices();
                        let target = match self.spread_page_nav_for_indices(&nav, intent.idx, dir) {
                            crate::ui_fullscreen::FsPageNav::Target(target) => Some(target),
                            crate::ui_fullscreen::FsPageNav::Delta(delta) => nav
                                .iter()
                                .position(|idx| *idx == intent.idx)
                                .and_then(|pos| pos.checked_add_signed(delta as isize))
                                .and_then(|pos| nav.get(pos))
                                .copied(),
                            _ => None,
                        };
                        if let Some(target) = target {
                            intent.clear_skipped_source_slice();
                            intent.idx = target;
                            let demand = self.book_target_presentation_demand(target);
                            self.begin_qualified_book_unit(intent, demand);
                        } else if let BookTargetRoute::ZipDfs(continuation) = &intent.route {
                            self.book_composition_permission.open =
                                OpenPermission::ContinueDfs(*continuation);
                        } else {
                            self.close_fullscreen();
                        }
                    }
                }
            }
            ctx.request_repaint();
            return;
        }
        // Publish the exact qualified occurrence into this context's visible episode before
        // its continuation commits slice/selection/timer effects. A prior half cannot grant it.
        self.publish_qualified_book_candidates(&candidates);
        self.book_composition_permission.open = OpenPermission::Resuming {
            intent: intent.clone(),
            candidates,
        };
        self.resume_qualified_book_target(ctx, &intent);
    }
    fn publish_qualified_book_candidates(&mut self, candidates: &[OpenCandidate]) {
        for candidate in candidates {
            if let OpenCandidateState::Complete(permission) = &candidate.state {
                self.book_composition_permission.occurrences.insert(
                    candidate.occurrence(),
                    OccurrencePermission::Complete(permission.clone()),
                );
            }
        }
    }
    fn resume_qualified_book_target(&mut self, ctx: &egui::Context, intent: &OpenIntent) {
        match &intent.route {
            BookTargetRoute::FullscreenOpen => self
                .open_fullscreen_with_materialization_and_contract(
                    intent.idx,
                    intent.history_trigger,
                    intent.materialization,
                    intent.contract,
                    None,
                ),
            BookTargetRoute::FullscreenNavigation => self.complete_book_navigation_target(
                ctx,
                intent.idx,
                intent.history_trigger,
                intent.contract,
                intent.purpose,
            ),
            BookTargetRoute::ContinuousSeek => {
                self.seek_to_continuous_page_with_contract(ctx, intent.idx, intent.contract)
            }
            BookTargetRoute::ZipDfs(continuation) => {
                self.complete_book_zip_dfs_target(ctx, intent.idx, *continuation)
            }
            BookTargetRoute::DeferredBook(deferred) => {
                self.complete_deferred_book_composition_open((**deferred).clone())
            }
            BookTargetRoute::Slideshow { slice } => {
                self.complete_book_slideshow_target(ctx, intent.idx, *slice, intent.purpose)
            }
        }
        ctx.request_repaint();
    }
}

#[cfg(test)]
#[path = "book_composition_permission_tests.rs"]
mod tests;
