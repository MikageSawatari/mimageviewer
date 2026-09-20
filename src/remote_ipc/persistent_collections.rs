use std::collections::HashSet;
use std::sync::Arc;
use std::time::{Duration, Instant};

use crossbeam_channel::{Receiver, select_biased};
use mimageviewer_ipc::{
    MAX_RESPONSE_FRAME_BYTES, PersistentCollectionAnchorResolution,
    PersistentCollectionBoundaryReason, PersistentCollectionCatalogPayload,
    PersistentCollectionCatalogResponse, PersistentCollectionEntry, PersistentCollectionEntryState,
    PersistentCollectionError, PersistentCollectionErrorCode, PersistentCollectionIdentity,
    PersistentCollectionNavigatePayload, PersistentCollectionNavigateRequest,
    PersistentCollectionNavigateResponse, PersistentCollectionNavigationDirection,
    PersistentCollectionNavigationKind, PersistentCollectionNavigationTail,
    PersistentCollectionOrderSummary, PersistentCollectionPageGroup, PersistentCollectionPageSlot,
    PersistentCollectionPositionKind, PersistentCollectionSnapshotPayload,
    PersistentCollectionSnapshotRequest, PersistentCollectionSnapshotResponse,
    PersistentCollectionSparseTarget, PersistentCollectionSummary,
    PersistentCollectionTargetPosition, RemoteAddress, RemoteEntryKind, RemotePagePresentationRole,
    RemoteReadingDirection, RemoteSingletonSpreadPlacement, RemoteSpreadMode, RequestId,
    ServerMessage,
};
use sha2::{Digest, Sha256};
use uuid::Uuid;

use crate::collection_store::{
    CollectionCatalogSnapshot, CollectionEntryId, CollectionId, CollectionNavigationAnchor,
    CollectionNavigationAnchorResolution, CollectionNavigationDirection,
    CollectionNavigationEntryIdentity, CollectionNavigationTail, CollectionNavigationTargetKind,
    CollectionOrderMode, CollectionPreparedNavigationCandidates,
    CollectionPreparedNavigationTarget, CollectionPreparedSnapshot,
    CollectionRemoteProducerControl, CollectionRemoteRequestLease, CollectionResolvedKind,
    CollectionRevisionNotice, CollectionRevisionWatch, CollectionSnapshot,
    CollectionSourcePreparation, CollectionStoreError, PreparedCollectionEntry,
    inspect_collection_source, prepare_collection_snapshot_while,
    resolve_prepared_collection_navigation,
};
use crate::settings::Settings;

use super::session::RemoteOperationCancellation;

const EXACT_REQUEST_BUDGET: Duration = Duration::from_secs(9);
const MAX_EXACT_RESTARTS: usize = 16;
const MAX_REMOTE_COLLECTION_ENTRIES: usize = 100_000;

#[derive(Clone)]
pub(super) struct PersistentCollectionEngine {
    producer: CollectionRemoteProducerControl,
}

struct PersistentCollectionViewFacts {
    token: String,
    configured: RemoteSpreadMode,
    effective: RemoteSpreadMode,
    direction: RemoteReadingDirection,
    entries: Arc<[PersistentCollectionEntry]>,
    groups: Arc<[crate::ui_fullscreen::RemotePageGroupSpec]>,
    remote_eligible: Arc<[RemoteEligibleEntry]>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct RemoteEligibleEntry {
    prepared_index: usize,
    kind: CollectionResolvedKind,
}

struct BoundedWirePrefix {
    entries: Vec<PersistentCollectionEntry>,
    retained_bytes: usize,
    retain_budget: usize,
    accepting: bool,
    observed: usize,
}

impl BoundedWirePrefix {
    fn new(retain_budget: usize) -> Self {
        Self {
            entries: Vec::new(),
            retained_bytes: 0,
            retain_budget,
            accepting: true,
            observed: 0,
        }
    }

    fn observe(&mut self, wire: PersistentCollectionEntry) {
        self.observed += 1;
        if !self.accepting || self.entries.len() >= MAX_REMOTE_COLLECTION_ENTRIES {
            return;
        }
        let cost = wire_entry_budget_cost(&wire);
        if self.retained_bytes.saturating_add(cost) < self.retain_budget {
            self.retained_bytes = self.retained_bytes.saturating_add(cost);
            self.entries.push(wire);
        } else {
            self.accepting = false;
        }
    }
}

fn stream_bounded_wire_entries<T>(
    entries: &[T],
    retain_budget: usize,
    mut keep_running: impl FnMut() -> bool,
    mut convert: impl FnMut(&T) -> PersistentCollectionEntry,
    mut observe_wire: impl FnMut(usize, &PersistentCollectionEntry),
) -> Option<BoundedWirePrefix> {
    let mut retained = BoundedWirePrefix::new(retain_budget);
    for (index, entry) in entries.iter().enumerate() {
        if !keep_running() {
            return None;
        }
        let wire = convert(entry);
        observe_wire(index, &wire);
        retained.observe(wire);
    }
    Some(retained)
}

struct ExactPrepared {
    loaded: CollectionSnapshot,
    prepared: CollectionPreparedSnapshot,
    /// Subscribe-before-load watch retained through response construction.
    watch: CollectionRevisionWatch,
}

struct ExactCatalog {
    catalog: CollectionCatalogSnapshot,
    watch: CollectionRevisionWatch,
}

impl ExactCatalog {
    fn was_invalidated(&self) -> bool {
        self.watch
            .take_latest()
            .as_ref()
            .is_some_and(|notice| notice.catalog_revision > self.catalog.catalog_revision)
    }
}

impl ExactPrepared {
    fn was_invalidated(&self) -> bool {
        notice_invalidates(&self.loaded, self.watch.take_latest().as_ref())
    }
}

#[derive(Clone, Copy)]
struct SpreadRequest {
    mode: Option<RemoteSpreadMode>,
    direction: Option<RemoteReadingDirection>,
    force_single: bool,
}

impl PersistentCollectionEngine {
    pub(super) fn new(producer: CollectionRemoteProducerControl) -> Self {
        Self { producer }
    }

    fn view_facts(
        &self,
        exact: &ExactPrepared,
        settings: &Settings,
        spread: SpreadRequest,
        lease: &CollectionRemoteRequestLease,
        cancellation: &RemoteOperationCancellation,
        deadline: Instant,
    ) -> Result<Arc<PersistentCollectionViewFacts>, PersistentCollectionError> {
        let mut current = || request_is_current(lease, cancellation, deadline);
        let (configured, effective, direction) = super::container::resolve_spread_state(
            spread.mode,
            spread.direction,
            None,
            None,
            crate::app::SpreadRestoreDefaults::NON_BOOK.spread_mode(),
            crate::app::SpreadRestoreDefaults::NON_BOOK.reading_direction(),
            spread.force_single,
        );
        let landscape = if effective == RemoteSpreadMode::Single {
            vec![false; exact.prepared.entries.len()]
        } else {
            super::collections::cached_prepared_collection_landscape_flags_while(
                exact.prepared.entries.as_ref(),
                &mut current,
            )
            .ok_or_else(|| request_interrupted_error(cancellation, deadline))?
        };
        if !current() {
            return Err(request_interrupted_error(cancellation, deadline));
        }
        let image_indices = prepared_image_indices(exact.prepared.entries.as_ref());
        let groups = crate::ui_fullscreen::build_remote_direct_image_page_groups(
            &image_indices,
            super::container::core_spread_mode(effective),
            &landscape,
        );
        const ENVELOPE_RESERVE: usize = 1024 * 1024;
        let retain_budget = MAX_RESPONSE_FRAME_BYTES.saturating_sub(ENVELOPE_RESERVE);
        let mut remote_eligible = Vec::new();
        let mut token_digest = exact_view_token_prefix(exact, settings, spread);
        let retained = stream_bounded_wire_entries(
            exact.prepared.entries.as_ref(),
            retain_budget,
            &mut current,
            wire_entry,
            |prepared_index, wire| {
                if let Some(kind) = wire_available_resolved_kind(wire) {
                    remote_eligible.push(RemoteEligibleEntry {
                        prepared_index,
                        kind,
                    });
                }
                token_digest.update(serde_json::to_vec(wire).unwrap_or_default());
                token_digest.update(b"\0");
            },
        )
        .ok_or_else(|| request_interrupted_error(cancellation, deadline))?;
        let token = finish_exact_view_token(
            token_digest,
            exact,
            settings,
            effective,
            direction,
            &groups,
            &mut current,
        )
        .ok_or_else(|| request_interrupted_error(cancellation, deadline))?;
        let facts = Arc::new(PersistentCollectionViewFacts {
            token,
            configured,
            effective,
            direction,
            entries: retained.entries.into(),
            groups: groups.into(),
            remote_eligible: remote_eligible.into(),
        });
        if !current() {
            return Err(request_interrupted_error(cancellation, deadline));
        }
        Ok(facts)
    }

    pub(super) fn catalog(
        &self,
        request_id: RequestId,
        cancellation: &RemoteOperationCancellation,
    ) -> PersistentCollectionCatalogResponse {
        let Some(lease) = self.producer.begin_request() else {
            return PersistentCollectionCatalogResponse::Error(error(
                PersistentCollectionErrorCode::Unavailable,
                "コレクションを利用できません",
            ));
        };
        let deadline = Instant::now() + EXACT_REQUEST_BUDGET;
        for _ in 0..MAX_EXACT_RESTARTS {
            let exact = match exact_catalog(&lease, cancellation, deadline) {
                Ok(exact) => exact,
                Err(value) => return PersistentCollectionCatalogResponse::Error(value),
            };
            let catalog = &exact.catalog;
            let total = catalog.definitions.len();
            let mut collections = Vec::new();
            let mut serialized_entries = 0usize;
            for definition in catalog
                .definitions
                .iter()
                .take(MAX_REMOTE_COLLECTION_ENTRIES)
            {
                if !request_is_current(&lease, cancellation, deadline) {
                    return PersistentCollectionCatalogResponse::Error(request_interrupted_error(
                        cancellation,
                        deadline,
                    ));
                }
                let summary = PersistentCollectionSummary {
                    collection_id: definition.id.to_string(),
                    name: definition.name.clone(),
                    order: order_summary(definition.order_mode, definition.standard_sort),
                    collection_revision: definition.revision,
                };
                let next = super::serialized_json_len(&summary).saturating_add(1);
                if serialized_entries.saturating_add(next)
                    >= MAX_RESPONSE_FRAME_BYTES.saturating_sub(64 * 1024)
                {
                    break;
                }
                serialized_entries = serialized_entries.saturating_add(next);
                collections.push(summary);
            }
            loop {
                if !request_is_current(&lease, cancellation, deadline) {
                    return PersistentCollectionCatalogResponse::Error(request_interrupted_error(
                        cancellation,
                        deadline,
                    ));
                }
                let response = ServerMessage::PersistentCollectionCatalog {
                    id: request_id,
                    response: PersistentCollectionCatalogResponse::Success(
                        PersistentCollectionCatalogPayload {
                            catalog_revision: catalog.catalog_revision,
                            collections: collections.clone(),
                            limit: collections.len(),
                            truncated: collections.len() < total,
                        },
                    ),
                };
                if super::serialized_json_len(&response) < MAX_RESPONSE_FRAME_BYTES {
                    break;
                }
                if collections.pop().is_none() {
                    return PersistentCollectionCatalogResponse::Error(error(
                        PersistentCollectionErrorCode::Internal,
                        "コレクション一覧応答を作成できませんでした",
                    ));
                }
            }
            let payload = PersistentCollectionCatalogPayload {
                catalog_revision: catalog.catalog_revision,
                limit: collections.len(),
                truncated: collections.len() < total,
                collections,
            };
            if exact.was_invalidated() {
                continue;
            }
            if !request_is_current(&lease, cancellation, deadline) {
                return PersistentCollectionCatalogResponse::Error(request_interrupted_error(
                    cancellation,
                    deadline,
                ));
            }
            return PersistentCollectionCatalogResponse::Success(payload);
        }
        PersistentCollectionCatalogResponse::Error(busy_error())
    }

    pub(super) fn snapshot(
        &self,
        request_id: RequestId,
        request: PersistentCollectionSnapshotRequest,
        cancellation: &RemoteOperationCancellation,
    ) -> PersistentCollectionSnapshotResponse {
        let id = match parse_collection_id(&request.collection_id) {
            Ok(id) => id,
            Err(value) => return PersistentCollectionSnapshotResponse::Error(value),
        };
        let Some(lease) = self.producer.begin_request() else {
            return PersistentCollectionSnapshotResponse::Error(error(
                PersistentCollectionErrorCode::Unavailable,
                "コレクションを利用できません",
            ));
        };
        let settings = match load_settings() {
            Ok(settings) => settings,
            Err(value) => return PersistentCollectionSnapshotResponse::Error(value),
        };
        let spread = SpreadRequest {
            mode: request.spread_mode,
            direction: request.reading_direction,
            force_single: request.force_single_page,
        };
        let deadline = Instant::now() + EXACT_REQUEST_BUDGET;
        'exact: for _ in 0..MAX_EXACT_RESTARTS {
            let exact = match exact_prepared(&lease, cancellation, id, &settings, deadline) {
                Ok(value) => value,
                Err(value) => return PersistentCollectionSnapshotResponse::Error(value),
            };
            let facts =
                match self.view_facts(&exact, &settings, spread, &lease, cancellation, deadline) {
                    Ok(facts) => facts,
                    Err(value) => return PersistentCollectionSnapshotResponse::Error(value),
                };
            let payload = match bounded_snapshot_payload(
                request_id,
                &exact,
                &settings,
                &facts,
                &lease,
                cancellation,
                deadline,
            ) {
                Ok(payload) => payload,
                Err(value) => return PersistentCollectionSnapshotResponse::Error(value),
            };
            if exact.was_invalidated() {
                continue 'exact;
            }
            if !request_is_current(&lease, cancellation, deadline) {
                return PersistentCollectionSnapshotResponse::Error(request_interrupted_error(
                    cancellation,
                    deadline,
                ));
            }
            return PersistentCollectionSnapshotResponse::Success(payload);
        }
        PersistentCollectionSnapshotResponse::Error(busy_error())
    }

    pub(super) fn navigate(
        &self,
        request_id: RequestId,
        request: PersistentCollectionNavigateRequest,
        cancellation: &RemoteOperationCancellation,
    ) -> PersistentCollectionNavigateResponse {
        let id = match parse_collection_id(&request.collection_id) {
            Ok(id) => id,
            Err(value) => return PersistentCollectionNavigateResponse::Error(value),
        };
        if request
            .locate_entry_id
            .as_ref()
            .is_some_and(|value| Uuid::parse_str(value).is_err())
            || (request.locate_entry_id.is_some() && request.locate_ordinal.is_some())
            || (request.direction != PersistentCollectionNavigationDirection::Current
                && (request.locate_entry_id.is_some() || request.locate_ordinal.is_some()))
        {
            return PersistentCollectionNavigateResponse::Error(error(
                PersistentCollectionErrorCode::BadRequest,
                "コレクション位置の指定が不正です",
            ));
        }
        let Some(lease) = self.producer.begin_request() else {
            return PersistentCollectionNavigateResponse::Error(error(
                PersistentCollectionErrorCode::Unavailable,
                "コレクションを利用できません",
            ));
        };
        let settings = match load_settings() {
            Ok(settings) => settings,
            Err(value) => return PersistentCollectionNavigateResponse::Error(value),
        };
        let spread = SpreadRequest {
            mode: request.spread_mode,
            direction: request.reading_direction,
            force_single: request.force_single_page,
        };
        let deadline = Instant::now() + EXACT_REQUEST_BUDGET;
        'exact: for _ in 0..MAX_EXACT_RESTARTS {
            let exact = match exact_prepared(&lease, cancellation, id, &settings, deadline) {
                Ok(value) => value,
                Err(value) => return PersistentCollectionNavigateResponse::Error(value),
            };
            let facts =
                match self.view_facts(&exact, &settings, spread, &lease, cancellation, deadline) {
                    Ok(facts) => facts,
                    Err(value) => return PersistentCollectionNavigateResponse::Error(value),
                };
            let exact_view_token = facts.token.clone();
            let anchor = match request
                .anchor
                .as_ref()
                .map(|value| {
                    let primary = navigation_identity(&exact.prepared, &value.primary)?;
                    let partner = value
                        .partner
                        .as_ref()
                        .map(|identity| navigation_identity(&exact.prepared, identity))
                        .transpose()?
                        .flatten();
                    Ok(match (primary, partner) {
                        (Some(primary), partner) => {
                            Some(CollectionNavigationAnchor { primary, partner })
                        }
                        (None, Some(partner)) => Some(CollectionNavigationAnchor {
                            primary: partner,
                            partner: None,
                        }),
                        (None, None) => None,
                    })
                })
                .transpose()
            {
                Ok(value) => value.flatten(),
                Err(value) => return PersistentCollectionNavigateResponse::Error(value),
            };
            let mut tried = HashSet::new();
            let eligible_targets =
                remote_eligible_target_set(&exact.prepared, &facts.remote_eligible);
            let target_kind = collection_navigation_target_kind(request.target_kind);
            let candidates = match request.direction {
                PersistentCollectionNavigationDirection::Forward
                | PersistentCollectionNavigationDirection::Backward => {
                    resolve_prepared_collection_navigation(
                        &exact.prepared,
                        anchor.as_ref(),
                        if request.direction == PersistentCollectionNavigationDirection::Forward {
                            CollectionNavigationDirection::Forward
                        } else {
                            CollectionNavigationDirection::Backward
                        },
                        target_kind,
                        match request.tail {
                            PersistentCollectionNavigationTail::Stop => {
                                CollectionNavigationTail::Stop
                            }
                            PersistentCollectionNavigationTail::Loop => {
                                CollectionNavigationTail::Loop
                            }
                        },
                        &tried,
                    )
                }
                PersistentCollectionNavigationDirection::First
                | PersistentCollectionNavigationDirection::Last => endpoint_candidates(
                    &exact.prepared,
                    &facts.remote_eligible,
                    target_kind,
                    request.direction == PersistentCollectionNavigationDirection::Last,
                ),
                PersistentCollectionNavigationDirection::Current => {
                    if let Some(ordinal) = request.locate_ordinal {
                        current_ordinal_candidates(
                            &exact.prepared,
                            &facts.remote_eligible,
                            ordinal,
                            target_kind,
                        )
                    } else {
                        current_candidates(
                            &exact.prepared,
                            anchor.as_ref(),
                            request.locate_entry_id.as_deref(),
                            target_kind,
                        )
                    }
                }
            };
            let anchor_resolution = if request.locate_ordinal.is_some() {
                PersistentCollectionAnchorResolution::Ordinal
            } else {
                wire_anchor_resolution(candidates.anchor_resolution)
            };
            let candidate_count = candidates.targets.len();
            let eligible_count = remote_eligible_entries(
                &exact.prepared,
                &facts.remote_eligible,
                request.target_kind,
            )
            .len();
            for target in candidates.targets {
                if !request_is_current(&lease, cancellation, deadline) {
                    return PersistentCollectionNavigateResponse::Error(request_interrupted_error(
                        cancellation,
                        deadline,
                    ));
                }
                tried.insert(target.entry_id);
                if !eligible_targets.contains(&(target.entry_id, target.resolved_kind)) {
                    continue;
                }
                let target_wire = match target.resolved_kind {
                    CollectionResolvedKind::Image => {
                        let Some(group) = image_display_unit_for_target(
                            &exact,
                            &facts,
                            &eligible_targets,
                            target.entry_id,
                            || request_is_current(&lease, cancellation, deadline),
                        ) else {
                            continue;
                        };
                        PersistentCollectionSparseTarget::DirectImageDisplayUnit { group }
                    }
                    CollectionResolvedKind::Video | CollectionResolvedKind::Audio => {
                        let CollectionSourcePreparation::Available { kind, .. } =
                            inspect_collection_source(&target.source_path)
                        else {
                            continue;
                        };
                        if kind != target.resolved_kind {
                            continue;
                        }
                        let resolved = match super::path_guard::resolve_existing(
                            target.source_path.to_string_lossy().as_ref(),
                        ) {
                            Ok(value) => value,
                            Err(_) => continue,
                        };
                        let identity = wire_identity(target.entry_id, &target.source_key);
                        let address =
                            RemoteAddress::file(resolved.logical.to_string_lossy().into_owned());
                        match target.resolved_kind {
                            CollectionResolvedKind::Video => {
                                PersistentCollectionSparseTarget::DirectVideo { identity, address }
                            }
                            CollectionResolvedKind::Audio => {
                                PersistentCollectionSparseTarget::DirectAudio { identity, address }
                            }
                            _ => unreachable!(),
                        }
                    }
                    _ => continue,
                };
                let landed_entry_id = match &target_wire {
                    PersistentCollectionSparseTarget::DirectImageDisplayUnit { group } => {
                        let Ok(anchor_id) = Uuid::parse_str(&group.anchor.entry_id) else {
                            continue;
                        };
                        crate::collection_store::CollectionEntryId::from_uuid(anchor_id)
                    }
                    _ => target.entry_id,
                };
                let Some(position) = landed_target_position(
                    &exact.prepared,
                    &facts.remote_eligible,
                    &target_wire,
                    landed_entry_id,
                ) else {
                    continue;
                };
                let replacement_needed = request.presented_view_token != exact_view_token
                    || request.presented_revision != exact.prepared.collection_revision;
                let response = match bounded_landed_response(
                    request_id,
                    &exact,
                    &settings,
                    &facts,
                    &exact_view_token,
                    &lease,
                    cancellation,
                    deadline,
                    replacement_needed,
                    target_wire,
                    position,
                    anchor_resolution.clone(),
                ) {
                    Ok(value) => value,
                    Err(value) => return PersistentCollectionNavigateResponse::Error(value),
                };
                if exact.was_invalidated() {
                    continue 'exact;
                }
                if !request_is_current(&lease, cancellation, deadline) {
                    return PersistentCollectionNavigateResponse::Error(request_interrupted_error(
                        cancellation,
                        deadline,
                    ));
                }
                return response;
            }
            let reason = if eligible_count == 0 {
                PersistentCollectionBoundaryReason::Empty
            } else if candidate_count == 0 {
                match request.direction {
                    PersistentCollectionNavigationDirection::Forward => {
                        PersistentCollectionBoundaryReason::End
                    }
                    PersistentCollectionNavigationDirection::Backward => {
                        PersistentCollectionBoundaryReason::Start
                    }
                    PersistentCollectionNavigationDirection::First
                    | PersistentCollectionNavigationDirection::Last
                    | PersistentCollectionNavigationDirection::Current => {
                        PersistentCollectionBoundaryReason::TargetUnavailable
                    }
                }
            } else {
                PersistentCollectionBoundaryReason::TargetUnavailable
            };
            let response = PersistentCollectionNavigateResponse::Success(
                PersistentCollectionNavigatePayload::Boundary {
                    exact_revision: exact.prepared.collection_revision,
                    exact_view_token: exact_view_token.clone(),
                    anchor_resolution,
                    reason,
                },
            );
            if exact.was_invalidated() {
                continue 'exact;
            }
            if !request_is_current(&lease, cancellation, deadline) {
                return PersistentCollectionNavigateResponse::Error(request_interrupted_error(
                    cancellation,
                    deadline,
                ));
            }
            return response;
        }
        PersistentCollectionNavigateResponse::Error(busy_error())
    }
}

fn exact_catalog(
    lease: &CollectionRemoteRequestLease,
    cancellation: &RemoteOperationCancellation,
    deadline: Instant,
) -> Result<ExactCatalog, PersistentCollectionError> {
    for _ in 0..MAX_EXACT_RESTARTS {
        if Instant::now() >= deadline {
            return Err(busy_error());
        }
        let watch = lease.client().subscribe().map_err(map_store_error)?;
        let perf_start = crate::perf::is_enabled().then(Instant::now);
        let reply = lease.client().list_catalog().map_err(map_store_error)?;
        let actor_result = wait_actor_reply(reply, lease, cancellation, deadline);
        if let Some(start) = perf_start {
            let entries = actor_result
                .as_ref()
                .ok()
                .and_then(|result| result.as_ref().ok())
                .map_or(0, |catalog| catalog.definitions.len());
            remote_actor_rtt_event(
                "list_catalog",
                None,
                entries,
                start,
                &actor_result,
                lease,
                cancellation,
                deadline,
            );
        }
        let catalog = actor_result??;
        let latest = watch.take_latest();
        if latest
            .as_ref()
            .is_some_and(|notice| notice.catalog_revision > catalog.catalog_revision)
        {
            remote_exact_perf_event(
                "catalog",
                None,
                catalog.definitions.len(),
                catalog.catalog_revision,
                "stale",
            );
            continue;
        }
        if !request_is_current(lease, cancellation, deadline) {
            remote_exact_perf_event(
                "catalog",
                None,
                catalog.definitions.len(),
                catalog.catalog_revision,
                "cancelled",
            );
            return Err(request_interrupted_error(cancellation, deadline));
        }
        remote_exact_perf_event(
            "catalog",
            None,
            catalog.definitions.len(),
            catalog.catalog_revision,
            "accepted",
        );
        return Ok(ExactCatalog { catalog, watch });
    }
    Err(busy_error())
}

fn exact_prepared(
    lease: &CollectionRemoteRequestLease,
    cancellation: &RemoteOperationCancellation,
    collection_id: CollectionId,
    settings: &Settings,
    deadline: Instant,
) -> Result<ExactPrepared, PersistentCollectionError> {
    for _ in 0..MAX_EXACT_RESTARTS {
        if Instant::now() >= deadline {
            return Err(busy_error());
        }
        let watch = lease.client().subscribe().map_err(map_store_error)?;
        let perf_start = crate::perf::is_enabled().then(Instant::now);
        let reply = lease
            .client()
            .load_collection(collection_id)
            .map_err(map_store_error)?;
        let actor_result = wait_actor_reply(reply, lease, cancellation, deadline);
        if let Some(start) = perf_start {
            let entries = actor_result
                .as_ref()
                .ok()
                .and_then(|result| result.as_ref().ok())
                .map_or(0, |snapshot| snapshot.entries.len());
            remote_actor_rtt_event(
                "load_collection",
                Some(collection_id),
                entries,
                start,
                &actor_result,
                lease,
                cancellation,
                deadline,
            );
        }
        let loaded = actor_result??;
        let mut observed = watch.take_latest();
        let prepare_start = crate::perf::is_enabled().then(Instant::now);
        let prepared = prepare_collection_snapshot_while(
            &loaded,
            &settings.grid_display_order,
            || {
                if let Some(notice) = watch.take_latest() {
                    observed = Some(notice);
                }
                !cancellation.is_cancelled() && lease.is_current() && Instant::now() < deadline
            },
            |_, _| {},
        );
        if let Some(start) = prepare_start {
            crate::perf::event(
                "collection",
                "remote_prepare",
                None,
                0,
                &[
                    (
                        "collection_id",
                        serde_json::Value::from(collection_id.as_uuid().to_string()),
                    ),
                    ("revision", serde_json::Value::from(loaded.revision())),
                    ("entries", serde_json::Value::from(loaded.entries.len())),
                    (
                        "ms",
                        serde_json::Value::from(start.elapsed().as_secs_f64() * 1000.0),
                    ),
                    (
                        "outcome",
                        serde_json::Value::from(match &prepared {
                            Ok(_) => "ok",
                            Err(crate::collection_store::CollectionPrepareError::Cancelled) => {
                                "cancelled"
                            }
                            Err(_) => "error",
                        }),
                    ),
                ],
            );
        }
        let prepared = prepared.map_err(|value| match value {
            crate::collection_store::CollectionPrepareError::Cancelled => cancelled_error(),
            _ => error(
                PersistentCollectionErrorCode::PrepareFailed,
                "コレクションの内容を確認できませんでした",
            ),
        })?;
        if let Some(notice) = watch.take_latest() {
            observed = Some(notice);
        }
        if notice_invalidates(&loaded, observed.as_ref()) {
            remote_exact_perf_event(
                "snapshot",
                Some(collection_id),
                loaded.entries.len(),
                loaded.revision(),
                "stale",
            );
            continue;
        }
        if !request_is_current(lease, cancellation, deadline) {
            remote_exact_perf_event(
                "snapshot",
                Some(collection_id),
                loaded.entries.len(),
                loaded.revision(),
                "cancelled",
            );
            return Err(request_interrupted_error(cancellation, deadline));
        }
        remote_exact_perf_event(
            "snapshot",
            Some(collection_id),
            loaded.entries.len(),
            loaded.revision(),
            "accepted",
        );
        return Ok(ExactPrepared {
            loaded,
            prepared,
            watch,
        });
    }
    Err(busy_error())
}

fn remote_actor_rtt_event<T>(
    operation: &'static str,
    collection_id: Option<CollectionId>,
    entries: usize,
    start: Instant,
    result: &Result<Result<T, PersistentCollectionError>, PersistentCollectionError>,
    lease: &CollectionRemoteRequestLease,
    cancellation: &RemoteOperationCancellation,
    deadline: Instant,
) {
    let outcome = if matches!(result, Ok(Ok(_))) {
        "reply_ok"
    } else if matches!(result, Ok(Err(_))) {
        "store_error"
    } else if cancellation.is_cancelled() || !lease.is_current() {
        "cancelled"
    } else if Instant::now() >= deadline {
        "timeout"
    } else {
        "error"
    };
    crate::perf::event(
        "collection",
        "actor_rtt",
        None,
        0,
        &[
            ("source", serde_json::Value::from("remote")),
            ("operation", serde_json::Value::from(operation)),
            (
                "collection_id",
                serde_json::Value::from(collection_id.map(|id| id.as_uuid().to_string())),
            ),
            ("entries", serde_json::Value::from(entries)),
            (
                "ms",
                serde_json::Value::from(start.elapsed().as_secs_f64() * 1000.0),
            ),
            ("outcome", serde_json::Value::from(outcome)),
        ],
    );
}

fn remote_exact_perf_event(
    operation: &'static str,
    collection_id: Option<CollectionId>,
    entries: usize,
    revision: u64,
    outcome: &'static str,
) {
    if !crate::perf::is_enabled() {
        return;
    }
    crate::perf::event(
        "collection",
        "remote_exact",
        None,
        0,
        &[
            ("operation", serde_json::Value::from(operation)),
            (
                "collection_id",
                serde_json::Value::from(collection_id.map(|id| id.as_uuid().to_string())),
            ),
            ("entries", serde_json::Value::from(entries)),
            ("revision", serde_json::Value::from(revision)),
            ("outcome", serde_json::Value::from(outcome)),
        ],
    );
}

fn wait_actor_reply<T>(
    reply: Receiver<Result<T, CollectionStoreError>>,
    lease: &CollectionRemoteRequestLease,
    cancellation: &RemoteOperationCancellation,
    deadline: Instant,
) -> Result<Result<T, PersistentCollectionError>, PersistentCollectionError> {
    let session_wake = cancellation.wake_receiver();
    let producer_wake = lease.close_receiver();
    let remaining = deadline.saturating_duration_since(Instant::now());
    select_biased! {
        recv(session_wake) -> _ => Err(cancelled_error()),
        recv(producer_wake) -> _ => Err(cancelled_error()),
        recv(reply) -> value => value
            .map_err(|_| error(PersistentCollectionErrorCode::Unavailable, "コレクションを利用できません"))
            .map(|value| value.map_err(map_store_error)),
        default(remaining) => Err(busy_error()),
    }
}

fn notice_invalidates(
    loaded: &CollectionSnapshot,
    notice: Option<&CollectionRevisionNotice>,
) -> bool {
    let Some(notice) = notice else { return false };
    if notice.catalog_revision <= loaded.catalog_revision {
        return false;
    }
    match notice
        .collection_revisions
        .iter()
        .find(|(id, _)| *id == loaded.collection_id())
    {
        None => true,
        Some((_, revision)) => *revision > loaded.revision(),
    }
}

fn wire_entry_budget_cost(wire: &PersistentCollectionEntry) -> usize {
    const GROUP_FRAMING_RESERVE: usize = 256;
    let mut cost = super::serialized_json_len(wire).saturating_add(1);
    if let PersistentCollectionEntryState::Available { address, kind, .. } = &wire.state
        && *kind == RemoteEntryKind::Image
    {
        let slot = PersistentCollectionPageSlot {
            identity: wire.identity.clone(),
            address: address.clone(),
            role: RemotePagePresentationRole::Navigation,
        };
        cost = cost
            .saturating_add(super::serialized_json_len(&slot))
            .saturating_add(GROUP_FRAMING_RESERVE);
    }
    cost
}

/// Rounds an entry prefix down so it never splits a still-image display unit.
/// A spread can span over interleaved video/audio entries.
fn complete_display_unit_prefix(
    facts: &PersistentCollectionViewFacts,
    maximum: usize,
    mut keep_running: impl FnMut() -> bool,
) -> Option<usize> {
    let mut cut = maximum;
    for group in facts.groups.iter() {
        if !keep_running() {
            return None;
        }
        let Some(first) = group.indices.iter().copied().min() else {
            continue;
        };
        let Some(last) = group.indices.iter().copied().max() else {
            continue;
        };
        if first < cut && cut <= last {
            cut = first;
        }
    }
    Some(cut)
}

fn bounded_snapshot_payload(
    request_id: RequestId,
    exact: &ExactPrepared,
    settings: &Settings,
    facts: &PersistentCollectionViewFacts,
    lease: &CollectionRemoteRequestLease,
    cancellation: &RemoteOperationCancellation,
    deadline: Instant,
) -> Result<PersistentCollectionSnapshotPayload, PersistentCollectionError> {
    let mut current = || request_is_current(lease, cancellation, deadline);
    let limit = complete_display_unit_prefix(facts, facts.entries.len(), &mut current)
        .ok_or_else(|| request_interrupted_error(cancellation, deadline))?;
    let payload = wire_snapshot(exact, settings, facts, limit, &facts.token, &mut current)
        .ok_or_else(|| request_interrupted_error(cancellation, deadline))?;
    let envelope = ServerMessage::PersistentCollectionSnapshot {
        id: request_id,
        response: PersistentCollectionSnapshotResponse::Success(payload.clone()),
    };
    if super::serialized_json_len(&envelope) >= MAX_RESPONSE_FRAME_BYTES {
        return Err(error(
            PersistentCollectionErrorCode::Internal,
            "コレクション応答を作成できませんでした",
        ));
    }
    if !current() {
        return Err(request_interrupted_error(cancellation, deadline));
    }
    Ok(payload)
}

#[allow(clippy::too_many_arguments)]
fn bounded_landed_response(
    request_id: RequestId,
    exact: &ExactPrepared,
    settings: &Settings,
    facts: &PersistentCollectionViewFacts,
    exact_view_token: &str,
    lease: &CollectionRemoteRequestLease,
    cancellation: &RemoteOperationCancellation,
    deadline: Instant,
    replacement_needed: bool,
    target: PersistentCollectionSparseTarget,
    position: PersistentCollectionTargetPosition,
    anchor_resolution: PersistentCollectionAnchorResolution,
) -> Result<PersistentCollectionNavigateResponse, PersistentCollectionError> {
    let mut current = || request_is_current(lease, cancellation, deadline);
    let make_response = |replacement| {
        PersistentCollectionNavigateResponse::Success(PersistentCollectionNavigatePayload::Landed {
            exact_revision: exact.prepared.collection_revision,
            exact_view_token: exact_view_token.to_owned(),
            replacement,
            target: target.clone(),
            position,
            anchor_resolution: anchor_resolution.clone(),
        })
    };
    if !replacement_needed {
        let response = make_response(None);
        let envelope = ServerMessage::PersistentCollectionNavigate {
            id: request_id,
            response: response.clone(),
        };
        if super::serialized_json_len(&envelope) >= MAX_RESPONSE_FRAME_BYTES {
            return Err(error(
                PersistentCollectionErrorCode::Internal,
                "コレクション応答を作成できませんでした",
            ));
        }
        if !current() {
            return Err(request_interrupted_error(cancellation, deadline));
        }
        return Ok(response);
    }
    let limit = complete_display_unit_prefix(facts, facts.entries.len(), &mut current)
        .ok_or_else(|| request_interrupted_error(cancellation, deadline))?;
    let response = make_response(Some(
        wire_snapshot(
            exact,
            settings,
            facts,
            limit,
            exact_view_token,
            &mut current,
        )
        .ok_or_else(|| request_interrupted_error(cancellation, deadline))?,
    ));
    let envelope = ServerMessage::PersistentCollectionNavigate {
        id: request_id,
        response: response.clone(),
    };
    if super::serialized_json_len(&envelope) >= MAX_RESPONSE_FRAME_BYTES {
        return Err(error(
            PersistentCollectionErrorCode::Internal,
            "コレクション応答を作成できませんでした",
        ));
    }
    if !current() {
        return Err(request_interrupted_error(cancellation, deadline));
    }
    Ok(response)
}

/// Builds only the selected display unit and revalidates every presentation slot.
/// A vanished partner shrinks to the surviving page; if both vanished the caller
/// marks this candidate tried and continues the finite traversal.
fn image_display_unit_for_target(
    exact: &ExactPrepared,
    facts: &PersistentCollectionViewFacts,
    eligible_targets: &HashSet<(CollectionEntryId, CollectionResolvedKind)>,
    target_entry_id: crate::collection_store::CollectionEntryId,
    mut keep_running: impl FnMut() -> bool,
) -> Option<PersistentCollectionPageGroup> {
    if !keep_running() {
        return None;
    }
    let spec = facts
        .groups
        .iter()
        .find(|group| {
            group.indices.iter().any(|index| {
                exact
                    .prepared
                    .entries
                    .get(*index)
                    .is_some_and(|entry| entry.entry_id == target_entry_id)
            })
        })?
        .clone();
    let mut pages = Vec::with_capacity(spec.indices.len());
    for index in spec.indices {
        if !keep_running() {
            return None;
        }
        let entry = exact.prepared.entries.get(index)?;
        if !eligible_targets.contains(&(entry.entry_id, CollectionResolvedKind::Image)) {
            continue;
        }
        let CollectionSourcePreparation::Available { kind, .. } =
            inspect_collection_source(&entry.source_path)
        else {
            continue;
        };
        if kind != CollectionResolvedKind::Image {
            continue;
        }
        let resolved =
            match super::path_guard::resolve_existing(entry.source_path.to_string_lossy().as_ref())
            {
                Ok(resolved) => resolved,
                Err(_) => continue,
            };
        pages.push(PersistentCollectionPageSlot {
            identity: wire_identity(entry.entry_id, &entry.source_key),
            address: RemoteAddress::file(resolved.logical.to_string_lossy().into_owned()),
            role: RemotePagePresentationRole::Navigation,
        });
    }
    if !image_group_retains_target(&pages, target_entry_id) {
        return None;
    }
    let anchor = if facts.effective.is_rtl() && pages.len() == 2 {
        pages[1].identity.clone()
    } else {
        pages[0].identity.clone()
    };
    Some(PersistentCollectionPageGroup {
        anchor,
        pages,
        slice: crate::ui_fullscreen::remote_page_slice(spec.slice),
        singleton_placement: RemoteSingletonSpreadPlacement::Center,
    })
}

fn image_group_retains_target(
    pages: &[PersistentCollectionPageSlot],
    target_entry_id: CollectionEntryId,
) -> bool {
    let target_id = target_entry_id.to_string();
    pages.iter().any(|page| page.identity.entry_id == target_id)
}

fn wire_snapshot(
    exact: &ExactPrepared,
    settings: &Settings,
    facts: &PersistentCollectionViewFacts,
    limit: usize,
    exact_view_token: &str,
    mut keep_running: impl FnMut() -> bool,
) -> Option<PersistentCollectionSnapshotPayload> {
    let returned = exact.prepared.entries.len().min(limit);
    let mut entries = Vec::with_capacity(returned);
    for entry in facts.entries.iter().take(returned) {
        if !keep_running() {
            return None;
        }
        entries.push(entry.clone());
    }
    let mut groups = Vec::new();
    for group in facts.groups.iter().filter(|group| {
        !group.indices.is_empty() && group.indices.iter().all(|index| *index < returned)
    }) {
        if !keep_running() {
            return None;
        }
        let mut pages = Vec::with_capacity(group.indices.len());
        for index in group.indices.iter().copied() {
            if !keep_running() {
                return None;
            }
            if let Some(slot) = entries.get(index).and_then(|wire| {
                let PersistentCollectionEntryState::Available { address, kind, .. } = &wire.state
                else {
                    return None;
                };
                (*kind == RemoteEntryKind::Image).then(|| PersistentCollectionPageSlot {
                    identity: wire.identity.clone(),
                    address: address.clone(),
                    role: RemotePagePresentationRole::Navigation,
                })
            }) {
                pages.push(slot);
            }
        }
        let anchor = if facts.effective.is_rtl() && pages.len() == 2 {
            pages.get(1).map(|slot| slot.identity.clone())
        } else {
            pages.first().map(|slot| slot.identity.clone())
        };
        if let Some(anchor) = anchor {
            groups.push(PersistentCollectionPageGroup {
                anchor,
                pages,
                slice: crate::ui_fullscreen::remote_page_slice(group.slice),
                singleton_placement: RemoteSingletonSpreadPlacement::Center,
            });
        }
    }
    Some(PersistentCollectionSnapshotPayload {
        collection_id: exact.prepared.collection_id.to_string(),
        collection_revision: exact.prepared.collection_revision,
        view_token: exact_view_token.to_owned(),
        title: exact.prepared.collection_name.clone(),
        order: order_summary(
            exact.loaded.definition.order_mode,
            exact.loaded.definition.standard_sort,
        ),
        image_count: facts
            .remote_eligible
            .iter()
            .filter(|entry| entry.kind == CollectionResolvedKind::Image)
            .count(),
        entries,
        configured_spread_mode: facts.configured,
        effective_spread_mode: facts.effective,
        reading_direction: facts.direction,
        page_groups: groups,
        spread_page_gap_px: settings.spread_page_gap_px,
        entry_limit: returned,
        truncated: returned < exact.prepared.entries.len(),
    })
}

fn wire_entry(entry: &PreparedCollectionEntry) -> PersistentCollectionEntry {
    let identity = wire_identity(entry.entry_id, &entry.source_key);
    let state = match &entry.availability {
        CollectionSourcePreparation::Available { kind, .. } => {
            let current = inspect_collection_source(&entry.source_path);
            if !matches!(
                current,
                CollectionSourcePreparation::Available {
                    kind: current_kind,
                    ..
                } if current_kind == *kind
            ) {
                let last_known_kind = Some(remote_kind(*kind));
                let state = match current {
                    CollectionSourcePreparation::Missing => {
                        PersistentCollectionEntryState::Missing { last_known_kind }
                    }
                    CollectionSourcePreparation::AccessError(_) => {
                        PersistentCollectionEntryState::AccessError { last_known_kind }
                    }
                    CollectionSourcePreparation::Unsupported
                    | CollectionSourcePreparation::Available { .. } => {
                        PersistentCollectionEntryState::Unsupported { last_known_kind }
                    }
                };
                return PersistentCollectionEntry {
                    identity,
                    name: entry_name(&entry.source_path),
                    state,
                };
            }
            match super::path_guard::resolve_existing(entry.source_path.to_string_lossy().as_ref())
            {
                Ok(resolved) => PersistentCollectionEntryState::Available {
                    address: RemoteAddress::file(resolved.logical.to_string_lossy().into_owned()),
                    kind: remote_kind(*kind),
                    thumbnail_address: None,
                    detail: None,
                    rating: None,
                },
                Err(
                    super::path_guard::ResolveError::NetworkPath
                    | super::path_guard::ResolveError::InvalidPath,
                ) => PersistentCollectionEntryState::BlockedByRemotePolicy {
                    last_known_kind: Some(remote_kind(*kind)),
                },
                Err(super::path_guard::ResolveError::Unavailable) => {
                    PersistentCollectionEntryState::AccessError {
                        last_known_kind: Some(remote_kind(*kind)),
                    }
                }
            }
        }
        CollectionSourcePreparation::Missing => PersistentCollectionEntryState::Missing {
            last_known_kind: None,
        },
        CollectionSourcePreparation::Unsupported => PersistentCollectionEntryState::Unsupported {
            last_known_kind: None,
        },
        CollectionSourcePreparation::AccessError(_) => {
            PersistentCollectionEntryState::AccessError {
                last_known_kind: None,
            }
        }
    };
    PersistentCollectionEntry {
        identity,
        name: entry_name(&entry.source_path),
        state,
    }
}

fn wire_available_resolved_kind(
    entry: &PersistentCollectionEntry,
) -> Option<CollectionResolvedKind> {
    let PersistentCollectionEntryState::Available { kind, .. } = entry.state else {
        return None;
    };
    Some(match kind {
        RemoteEntryKind::Image => CollectionResolvedKind::Image,
        RemoteEntryKind::Video => CollectionResolvedKind::Video,
        RemoteEntryKind::Audio => CollectionResolvedKind::Audio,
        RemoteEntryKind::Folder => CollectionResolvedKind::Folder,
        RemoteEntryKind::Zip => CollectionResolvedKind::Zip,
        RemoteEntryKind::Pdf => CollectionResolvedKind::Pdf,
        RemoteEntryKind::Archive => CollectionResolvedKind::ConvertibleArchive,
        RemoteEntryKind::Other => CollectionResolvedKind::Unresolved,
    })
}

fn prepared_image_indices(entries: &[PreparedCollectionEntry]) -> Vec<usize> {
    entries
        .iter()
        .enumerate()
        .filter_map(|(index, entry)| {
            matches!(
                entry.availability,
                CollectionSourcePreparation::Available {
                    kind: CollectionResolvedKind::Image,
                    ..
                }
            )
            .then_some(index)
        })
        .collect()
}

fn navigation_identity(
    prepared: &CollectionPreparedSnapshot,
    identity: &PersistentCollectionIdentity,
) -> Result<Option<CollectionNavigationEntryIdentity>, PersistentCollectionError> {
    if identity.source_identity.len() != 64
        || !identity
            .source_identity
            .bytes()
            .all(|value| value.is_ascii_digit() || (b'a'..=b'f').contains(&value))
    {
        return Err(error(
            PersistentCollectionErrorCode::BadRequest,
            "コレクション項目の識別子が正しくありません",
        ));
    }
    let parsed = Uuid::parse_str(&identity.entry_id)
        .map(crate::collection_store::CollectionEntryId::from_uuid)
        .map_err(|_| {
            error(
                PersistentCollectionErrorCode::BadRequest,
                "コレクション項目の識別子が正しくありません",
            )
        })?;
    if let Some(entry) = prepared
        .entries
        .iter()
        .find(|entry| parsed == entry.entry_id)
    {
        // Entry ID is the canonical first choice. A relink can legitimately
        // change its source token between the presented and exact snapshot.
        return Ok(Some(CollectionNavigationEntryIdentity {
            entry_id: entry.entry_id,
            source_key: entry.source_key.clone(),
        }));
    }
    Ok(prepared
        .entries
        .iter()
        .find(|entry| source_identity(&entry.source_key) == identity.source_identity)
        .map(|entry| CollectionNavigationEntryIdentity {
            // Keep the stale ID so the pure resolver records SourceKey rather
            // than misreporting the fallback as an EntryId hit.
            entry_id: parsed,
            source_key: entry.source_key.clone(),
        }))
}

fn wire_identity(
    entry_id: crate::collection_store::CollectionEntryId,
    source_key: &crate::collection_store::CollectionSourcePathKey,
) -> PersistentCollectionIdentity {
    PersistentCollectionIdentity {
        entry_id: entry_id.to_string(),
        source_identity: source_identity(source_key),
    }
}

fn source_identity(key: &crate::collection_store::CollectionSourcePathKey) -> String {
    let mut digest = Sha256::new();
    digest.update(b"mimageviewer:persistent-collection-source:v1\0");
    digest.update(key.namespace().as_str().as_bytes());
    digest.update(b"\0");
    digest.update(key.normalized_path().as_bytes());
    lower_hex(&digest.finalize())
}

#[allow(clippy::too_many_arguments)]
fn exact_view_token_prefix(
    exact: &ExactPrepared,
    settings: &Settings,
    spread: SpreadRequest,
) -> Sha256 {
    let mut digest = Sha256::new();
    digest.update(b"mimageviewer:persistent-collection-view:v2\0");
    digest.update(exact.prepared.collection_id.to_string().as_bytes());
    digest.update(exact.prepared.collection_revision.to_le_bytes());
    digest.update(format!("{:?}", settings.grid_display_order).as_bytes());
    digest.update(serde_json::to_vec(&spread.mode).unwrap_or_default());
    digest.update(serde_json::to_vec(&spread.direction).unwrap_or_default());
    digest.update([u8::from(spread.force_single)]);
    digest
}

fn finish_exact_view_token(
    mut digest: Sha256,
    exact: &ExactPrepared,
    settings: &Settings,
    effective: RemoteSpreadMode,
    direction: RemoteReadingDirection,
    groups: &[crate::ui_fullscreen::RemotePageGroupSpec],
    keep_running: &mut impl FnMut() -> bool,
) -> Option<String> {
    digest.update(serde_json::to_vec(&effective).unwrap_or_default());
    digest.update(serde_json::to_vec(&direction).unwrap_or_default());
    digest.update(settings.spread_page_gap_px.to_le_bytes());
    for group in groups {
        if !keep_running() {
            return None;
        }
        digest.update(format!("{:?}:{:?}", group.slice, group.singleton_placement).as_bytes());
        for index in &group.indices {
            if let Some(entry) = exact.prepared.entries.get(*index) {
                digest.update(entry.entry_id.to_string().as_bytes());
                digest.update(b"\0");
                digest.update(source_identity(&entry.source_key).as_bytes());
                digest.update(b"\0");
            }
        }
    }
    Some(lower_hex(&digest.finalize()))
}

fn request_is_current(
    lease: &CollectionRemoteRequestLease,
    cancellation: &RemoteOperationCancellation,
    deadline: Instant,
) -> bool {
    Instant::now() < deadline && lease.is_current() && !cancellation.is_cancelled()
}

fn request_interrupted_error(
    cancellation: &RemoteOperationCancellation,
    deadline: Instant,
) -> PersistentCollectionError {
    if Instant::now() >= deadline {
        busy_error()
    } else if cancellation.is_cancelled() {
        cancelled_error()
    } else {
        cancelled_error()
    }
}

fn resolved_kind_matches_navigation(
    kind: CollectionResolvedKind,
    target: PersistentCollectionNavigationKind,
) -> bool {
    matches!(
        (target, kind),
        (
            PersistentCollectionNavigationKind::NavigableMedia,
            CollectionResolvedKind::Image
                | CollectionResolvedKind::Video
                | CollectionResolvedKind::Audio
        ) | (
            PersistentCollectionNavigationKind::StillImage,
            CollectionResolvedKind::Image
        ) | (
            PersistentCollectionNavigationKind::Video,
            CollectionResolvedKind::Video
        ) | (
            PersistentCollectionNavigationKind::Audio,
            CollectionResolvedKind::Audio
        )
    )
}

fn remote_eligible_entries<'a>(
    prepared: &'a CollectionPreparedSnapshot,
    remote_eligible: &[RemoteEligibleEntry],
    target: PersistentCollectionNavigationKind,
) -> Vec<&'a PreparedCollectionEntry> {
    remote_eligible
        .iter()
        .filter(|entry| resolved_kind_matches_navigation(entry.kind, target))
        .filter_map(|entry| prepared.entries.get(entry.prepared_index))
        .collect()
}

fn remote_eligible_target_set(
    prepared: &CollectionPreparedSnapshot,
    remote_eligible: &[RemoteEligibleEntry],
) -> HashSet<(CollectionEntryId, CollectionResolvedKind)> {
    remote_eligible
        .iter()
        .filter_map(|eligible| {
            prepared
                .entries
                .get(eligible.prepared_index)
                .map(|entry| (entry.entry_id, eligible.kind))
        })
        .collect()
}

fn landed_target_position(
    prepared: &CollectionPreparedSnapshot,
    remote_eligible: &[RemoteEligibleEntry],
    target: &PersistentCollectionSparseTarget,
    landed_entry_id: crate::collection_store::CollectionEntryId,
) -> Option<PersistentCollectionTargetPosition> {
    let (kind, projection) = match target {
        PersistentCollectionSparseTarget::DirectImageDisplayUnit { .. } => (
            PersistentCollectionPositionKind::StillImage,
            PersistentCollectionNavigationKind::StillImage,
        ),
        PersistentCollectionSparseTarget::DirectVideo { .. } => (
            PersistentCollectionPositionKind::Video,
            PersistentCollectionNavigationKind::Video,
        ),
        PersistentCollectionSparseTarget::DirectAudio { .. } => (
            PersistentCollectionPositionKind::Audio,
            PersistentCollectionNavigationKind::Audio,
        ),
    };
    let eligible = remote_eligible_entries(prepared, remote_eligible, projection);
    let ordinal = eligible
        .iter()
        .position(|entry| entry.entry_id == landed_entry_id)?;
    Some(PersistentCollectionTargetPosition {
        kind,
        ordinal,
        count: eligible.len(),
    })
}

fn collection_navigation_target_kind(
    value: PersistentCollectionNavigationKind,
) -> CollectionNavigationTargetKind {
    match value {
        PersistentCollectionNavigationKind::NavigableMedia => {
            CollectionNavigationTargetKind::NavigableMedia
        }
        PersistentCollectionNavigationKind::StillImage => {
            CollectionNavigationTargetKind::StillImage
        }
        PersistentCollectionNavigationKind::Video => CollectionNavigationTargetKind::Video,
        PersistentCollectionNavigationKind::Audio => CollectionNavigationTargetKind::Audio,
    }
}

fn prepared_navigation_target(
    entry: &PreparedCollectionEntry,
    target_kind: CollectionNavigationTargetKind,
) -> Option<CollectionPreparedNavigationTarget> {
    let CollectionSourcePreparation::Available { kind, .. } = entry.availability else {
        return None;
    };
    let matches = resolved_kind_matches_target(kind, target_kind);
    matches.then(|| CollectionPreparedNavigationTarget {
        entry_id: entry.entry_id,
        source_key: entry.source_key.clone(),
        source_path: entry.source_path.clone(),
        resolved_kind: kind,
    })
}

fn resolved_kind_matches_target(
    kind: CollectionResolvedKind,
    target_kind: CollectionNavigationTargetKind,
) -> bool {
    match target_kind {
        CollectionNavigationTargetKind::StillImage => kind == CollectionResolvedKind::Image,
        CollectionNavigationTargetKind::Video => kind == CollectionResolvedKind::Video,
        CollectionNavigationTargetKind::Audio => kind == CollectionResolvedKind::Audio,
        CollectionNavigationTargetKind::NavigableMedia => matches!(
            kind,
            CollectionResolvedKind::Image
                | CollectionResolvedKind::Video
                | CollectionResolvedKind::Audio
        ),
        CollectionNavigationTargetKind::OuterContainer => matches!(
            kind,
            CollectionResolvedKind::Folder
                | CollectionResolvedKind::Zip
                | CollectionResolvedKind::Pdf
                | CollectionResolvedKind::ConvertibleArchive
        ),
    }
}

fn endpoint_candidates(
    prepared: &CollectionPreparedSnapshot,
    remote_eligible: &[RemoteEligibleEntry],
    target_kind: CollectionNavigationTargetKind,
    reverse: bool,
) -> CollectionPreparedNavigationCandidates {
    let mut targets = remote_eligible
        .iter()
        .filter_map(|entry| prepared.entries.get(entry.prepared_index))
        .filter_map(|entry| prepared_navigation_target(entry, target_kind))
        .collect::<Vec<_>>();
    if reverse {
        targets.reverse();
    }
    CollectionPreparedNavigationCandidates {
        anchor_resolution: CollectionNavigationAnchorResolution::Head,
        targets,
    }
}

fn current_candidates(
    prepared: &CollectionPreparedSnapshot,
    anchor: Option<&CollectionNavigationAnchor>,
    locate_entry_id: Option<&str>,
    target_kind: CollectionNavigationTargetKind,
) -> CollectionPreparedNavigationCandidates {
    let mut resolution = CollectionNavigationAnchorResolution::Head;
    let parsed_locate = locate_entry_id.and_then(|value| {
        Uuid::parse_str(value)
            .ok()
            .map(crate::collection_store::CollectionEntryId::from_uuid)
    });
    let entry = if let Some(parsed) = parsed_locate {
        prepared
            .entries
            .iter()
            .find(|entry| entry.entry_id == parsed)
            .inspect(|_| resolution = CollectionNavigationAnchorResolution::EntryId)
            .or_else(|| {
                // History state may supply the old source identity only for the
                // same URL entry ID. It cannot redirect a deep link elsewhere.
                let anchor = anchor.filter(|anchor| anchor.primary.entry_id == parsed)?;
                prepared
                    .entries
                    .iter()
                    .find(|entry| entry.source_key == anchor.primary.source_key)
                    .inspect(|_| resolution = CollectionNavigationAnchorResolution::SourceKey)
            })
    } else {
        anchor.and_then(|anchor| {
            prepared
                .entries
                .iter()
                .find(|entry| entry.entry_id == anchor.primary.entry_id)
                .inspect(|_| resolution = CollectionNavigationAnchorResolution::EntryId)
                .or_else(|| {
                    prepared
                        .entries
                        .iter()
                        .find(|entry| entry.source_key == anchor.primary.source_key)
                        .inspect(|_| resolution = CollectionNavigationAnchorResolution::SourceKey)
                })
        })
    };
    CollectionPreparedNavigationCandidates {
        anchor_resolution: resolution,
        targets: entry
            .and_then(|entry| prepared_navigation_target(entry, target_kind))
            .into_iter()
            .collect(),
    }
}

fn current_ordinal_candidates(
    prepared: &CollectionPreparedSnapshot,
    remote_eligible: &[RemoteEligibleEntry],
    ordinal: usize,
    target_kind: CollectionNavigationTargetKind,
) -> CollectionPreparedNavigationCandidates {
    let target = remote_eligible
        .iter()
        .filter(|entry| resolved_kind_matches_target(entry.kind, target_kind))
        .nth(ordinal)
        .and_then(|entry| prepared.entries.get(entry.prepared_index))
        .and_then(|entry| prepared_navigation_target(entry, target_kind));
    CollectionPreparedNavigationCandidates {
        anchor_resolution: CollectionNavigationAnchorResolution::Head,
        targets: target.into_iter().collect(),
    }
}

fn wire_anchor_resolution(
    value: CollectionNavigationAnchorResolution,
) -> PersistentCollectionAnchorResolution {
    match value {
        CollectionNavigationAnchorResolution::EntryId
        | CollectionNavigationAnchorResolution::DisplayUnit => {
            PersistentCollectionAnchorResolution::EntryId
        }
        CollectionNavigationAnchorResolution::SourceKey => {
            PersistentCollectionAnchorResolution::SourceIdentity
        }
        CollectionNavigationAnchorResolution::Head => PersistentCollectionAnchorResolution::Head,
    }
}

fn remote_kind(kind: CollectionResolvedKind) -> RemoteEntryKind {
    match kind {
        CollectionResolvedKind::Image => RemoteEntryKind::Image,
        CollectionResolvedKind::Video => RemoteEntryKind::Video,
        CollectionResolvedKind::Audio => RemoteEntryKind::Audio,
        CollectionResolvedKind::Folder => RemoteEntryKind::Folder,
        CollectionResolvedKind::Zip => RemoteEntryKind::Zip,
        CollectionResolvedKind::Pdf => RemoteEntryKind::Pdf,
        CollectionResolvedKind::ConvertibleArchive => RemoteEntryKind::Archive,
        CollectionResolvedKind::Unresolved => RemoteEntryKind::Other,
    }
}

fn order_summary(
    mode: CollectionOrderMode,
    sort: crate::settings::SortOrder,
) -> PersistentCollectionOrderSummary {
    match mode {
        CollectionOrderMode::Manual => PersistentCollectionOrderSummary::Manual,
        CollectionOrderMode::Shuffle => PersistentCollectionOrderSummary::Shuffle,
        CollectionOrderMode::Standard => PersistentCollectionOrderSummary::Standard {
            value: super::sort_order_wire_value(sort),
            label: sort.label().to_owned(),
            short_label: sort.short_label().to_owned(),
        },
    }
}

fn parse_collection_id(value: &str) -> Result<CollectionId, PersistentCollectionError> {
    Uuid::parse_str(value)
        .map(CollectionId::from_uuid)
        .map_err(|_| {
            error(
                PersistentCollectionErrorCode::BadRequest,
                "コレクションIDが正しくありません",
            )
        })
}

fn load_settings() -> Result<Settings, PersistentCollectionError> {
    crate::settings_db::with_db_result(|db| db.load_into_settings()).map_err(|_| {
        error(
            PersistentCollectionErrorCode::Unavailable,
            "最新の表示設定を読み込めませんでした",
        )
    })
}

fn map_store_error(value: CollectionStoreError) -> PersistentCollectionError {
    let code = match value {
        CollectionStoreError::Starting => PersistentCollectionErrorCode::Starting,
        CollectionStoreError::Busy => PersistentCollectionErrorCode::Busy,
        CollectionStoreError::Unavailable => PersistentCollectionErrorCode::Unavailable,
        CollectionStoreError::NotFound => PersistentCollectionErrorCode::NotFound,
        CollectionStoreError::Conflict { .. } => PersistentCollectionErrorCode::Conflict,
        CollectionStoreError::IncompatibleSchema(_) => PersistentCollectionErrorCode::Incompatible,
        CollectionStoreError::InvalidName
        | CollectionStoreError::InvalidPath(_)
        | CollectionStoreError::InvalidOrder
        | CollectionStoreError::ManualOrderInactive
        | CollectionStoreError::DuplicateSource(_) => PersistentCollectionErrorCode::BadRequest,
        CollectionStoreError::Persistence(_) => PersistentCollectionErrorCode::Internal,
    };
    error(
        code,
        match code {
            PersistentCollectionErrorCode::Starting => "コレクションを準備しています",
            PersistentCollectionErrorCode::Busy => "コレクションの更新中です。再試行してください",
            PersistentCollectionErrorCode::NotFound => "コレクションが見つかりません",
            PersistentCollectionErrorCode::Conflict => "コレクションが更新されました",
            PersistentCollectionErrorCode::Incompatible => "コレクションDBの版が対応していません",
            PersistentCollectionErrorCode::Unavailable => "コレクションを利用できません",
            _ => "コレクション要求を処理できませんでした",
        },
    )
}

fn error(code: PersistentCollectionErrorCode, message: &'static str) -> PersistentCollectionError {
    PersistentCollectionError::new(code, message)
}

fn cancelled_error() -> PersistentCollectionError {
    error(
        PersistentCollectionErrorCode::Cancelled,
        "コレクション要求を中止しました",
    )
}

fn busy_error() -> PersistentCollectionError {
    error(
        PersistentCollectionErrorCode::Busy,
        "コレクションの更新中です。再試行してください",
    )
}

fn entry_name(path: &std::path::Path) -> String {
    path.file_name()
        .unwrap_or_else(|| path.as_os_str())
        .to_string_lossy()
        .into_owned()
}

fn lower_hex(bytes: &[u8]) -> String {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    let mut value = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        value.push(DIGITS[(byte >> 4) as usize] as char);
        value.push(DIGITS[(byte & 0x0f) as usize] as char);
    }
    value
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::*;
    use crate::collection_store::CollectionSourcePath;

    fn prepared_entry(id: &str, path: &str) -> PreparedCollectionEntry {
        let source = CollectionSourcePath::from_trusted(Path::new(path)).unwrap();
        let source_path = source.path().to_path_buf();
        PreparedCollectionEntry {
            entry_id: crate::collection_store::CollectionEntryId::from_uuid(
                Uuid::parse_str(id).unwrap(),
            ),
            source_key: source.key().clone(),
            source_path: source_path.clone(),
            availability: CollectionSourcePreparation::Available {
                kind: CollectionResolvedKind::Image,
                mtime: 1,
                file_size: Some(1),
            },
            item: crate::grid_item::GridItem::Image(source_path),
            display_meta: Some((1, 1)),
        }
    }

    fn prepared(entries: Vec<PreparedCollectionEntry>) -> CollectionPreparedSnapshot {
        CollectionPreparedSnapshot {
            collection_id: CollectionId::new(),
            collection_revision: 1,
            collection_name: "test".to_owned(),
            order_mode: CollectionOrderMode::Manual,
            standard_sort: crate::settings::SortOrder::FileName,
            entries: entries.into(),
        }
    }

    #[test]
    fn current_locator_prefers_url_entry_id_then_matching_history_source() {
        let old_id = "11111111-1111-4111-8111-111111111111";
        let replacement_id = "22222222-2222-4222-8222-222222222222";
        let old = prepared_entry(old_id, r"C:\collection\old.jpg");
        let historical = CollectionNavigationAnchor {
            primary: CollectionNavigationEntryIdentity {
                entry_id: old.entry_id,
                source_key: old.source_key.clone(),
            },
            partner: None,
        };

        let relinked = prepared(vec![
            prepared_entry(old_id, r"C:\collection\new.jpg"),
            prepared_entry(replacement_id, r"C:\collection\old.jpg"),
        ]);
        let by_id = current_candidates(
            &relinked,
            Some(&historical),
            Some(old_id),
            CollectionNavigationTargetKind::StillImage,
        );
        assert_eq!(
            by_id.anchor_resolution,
            CollectionNavigationAnchorResolution::EntryId
        );
        assert_eq!(by_id.targets[0].entry_id, relinked.entries[0].entry_id);

        let readded = prepared(vec![prepared_entry(
            replacement_id,
            r"C:\collection\old.jpg",
        )]);
        let by_source = current_candidates(
            &readded,
            Some(&historical),
            Some(old_id),
            CollectionNavigationTargetKind::StillImage,
        );
        assert_eq!(
            by_source.anchor_resolution,
            CollectionNavigationAnchorResolution::SourceKey
        );
        assert_eq!(by_source.targets[0].entry_id, readded.entries[0].entry_id);
    }

    #[test]
    fn current_locator_uses_the_remote_eligible_ordinal_projection() {
        let first = prepared_entry(
            "11111111-1111-4111-8111-111111111111",
            r"C:\collection\first.jpg",
        );
        let second = prepared_entry(
            "22222222-2222-4222-8222-222222222222",
            r"C:\collection\second.jpg",
        );
        let snapshot = prepared(vec![first, second.clone()]);
        // The first prepared image models an entry rejected by the Remote path projection.
        // Ordinal zero must therefore resolve the second, remotely available image.
        let remote_eligible = [RemoteEligibleEntry {
            prepared_index: 1,
            kind: CollectionResolvedKind::Image,
        }];
        let candidates = current_ordinal_candidates(
            &snapshot,
            &remote_eligible,
            0,
            CollectionNavigationTargetKind::StillImage,
        );
        assert_eq!(candidates.targets.len(), 1);
        assert_eq!(candidates.targets[0].entry_id, second.entry_id);
        assert!(
            current_ordinal_candidates(
                &snapshot,
                &remote_eligible,
                1,
                CollectionNavigationTargetKind::StillImage,
            )
            .targets
            .is_empty()
        );
    }

    #[test]
    fn landed_position_uses_the_actual_media_projection_not_the_route_search_kind() {
        let entries = [
            (
                "11111111-1111-4111-8111-111111111111",
                "one.jpg",
                CollectionResolvedKind::Image,
            ),
            (
                "22222222-2222-4222-8222-222222222222",
                "one.mp4",
                CollectionResolvedKind::Video,
            ),
            (
                "33333333-3333-4333-8333-333333333333",
                "blocked.jpg",
                CollectionResolvedKind::Image,
            ),
            (
                "44444444-4444-4444-8444-444444444444",
                "one.mp3",
                CollectionResolvedKind::Audio,
            ),
            (
                "55555555-5555-4555-8555-555555555555",
                "two.jpg",
                CollectionResolvedKind::Image,
            ),
            (
                "66666666-6666-4666-8666-666666666666",
                "two.mp4",
                CollectionResolvedKind::Video,
            ),
            (
                "77777777-7777-4777-8777-777777777777",
                "two.mp3",
                CollectionResolvedKind::Audio,
            ),
        ]
        .into_iter()
        .map(|(id, name, kind)| {
            let mut entry = prepared_entry(id, &format!(r"C:\collection\{name}"));
            entry.availability = CollectionSourcePreparation::Available {
                kind,
                mtime: 1,
                file_size: Some(1),
            };
            entry.item = match kind {
                CollectionResolvedKind::Video => {
                    crate::grid_item::GridItem::Video(entry.source_path.clone())
                }
                CollectionResolvedKind::Audio => {
                    crate::grid_item::GridItem::Audio(entry.source_path.clone())
                }
                _ => entry.item,
            };
            entry
        })
        .collect::<Vec<_>>();
        let snapshot = prepared(entries);
        let remote_eligible = [
            (0, CollectionResolvedKind::Image),
            (1, CollectionResolvedKind::Video),
            (3, CollectionResolvedKind::Audio),
            (4, CollectionResolvedKind::Image),
            (5, CollectionResolvedKind::Video),
            (6, CollectionResolvedKind::Audio),
        ]
        .map(|(prepared_index, kind)| RemoteEligibleEntry {
            prepared_index,
            kind,
        });
        let eligible_targets = remote_eligible_target_set(&snapshot, &remote_eligible);
        assert!(
            !eligible_targets
                .contains(&(snapshot.entries[2].entry_id, CollectionResolvedKind::Image))
        );
        assert!(
            eligible_targets
                .contains(&(snapshot.entries[4].entry_id, CollectionResolvedKind::Image))
        );
        let blocked_route = current_candidates(
            &snapshot,
            None,
            Some("33333333-3333-4333-8333-333333333333"),
            CollectionNavigationTargetKind::NavigableMedia,
        );
        assert_eq!(blocked_route.targets.len(), 1);
        assert!(!eligible_targets.contains(&(
            blocked_route.targets[0].entry_id,
            blocked_route.targets[0].resolved_kind
        )));
        for index in [0, 1, 3, 4, 5, 6] {
            let target = prepared_navigation_target(
                &snapshot.entries[index],
                CollectionNavigationTargetKind::NavigableMedia,
            )
            .unwrap();
            assert!(eligible_targets.contains(&(target.entry_id, target.resolved_kind)));
        }
        let image = &snapshot.entries[4];
        let identity = wire_identity(image.entry_id, &image.source_key);
        let image_target = PersistentCollectionSparseTarget::DirectImageDisplayUnit {
            group: PersistentCollectionPageGroup {
                anchor: identity.clone(),
                pages: vec![PersistentCollectionPageSlot {
                    identity,
                    address: RemoteAddress::file(r"C:\collection\two.jpg"),
                    role: RemotePagePresentationRole::Navigation,
                }],
                slice: crate::ui_fullscreen::remote_page_slice(crate::page_split::PageSlice::Full),
                singleton_placement: RemoteSingletonSpreadPlacement::Center,
            },
        };
        assert_eq!(
            landed_target_position(&snapshot, &remote_eligible, &image_target, image.entry_id),
            Some(PersistentCollectionTargetPosition {
                kind: PersistentCollectionPositionKind::StillImage,
                ordinal: 1,
                count: 2,
            })
        );
        for (index, kind, target) in [
            (
                5,
                PersistentCollectionPositionKind::Video,
                PersistentCollectionSparseTarget::DirectVideo {
                    identity: wire_identity(
                        snapshot.entries[5].entry_id,
                        &snapshot.entries[5].source_key,
                    ),
                    address: RemoteAddress::file(r"C:\collection\two.mp4"),
                },
            ),
            (
                6,
                PersistentCollectionPositionKind::Audio,
                PersistentCollectionSparseTarget::DirectAudio {
                    identity: wire_identity(
                        snapshot.entries[6].entry_id,
                        &snapshot.entries[6].source_key,
                    ),
                    address: RemoteAddress::file(r"C:\collection\two.mp3"),
                },
            ),
        ] {
            assert_eq!(
                landed_target_position(
                    &snapshot,
                    &remote_eligible,
                    &target,
                    snapshot.entries[index].entry_id
                ),
                Some(PersistentCollectionTargetPosition {
                    kind,
                    ordinal: 1,
                    count: 2
                })
            );
        }
        assert_eq!(
            landed_target_position(
                &snapshot,
                &remote_eligible,
                &image_target,
                snapshot.entries[2].entry_id
            ),
            None,
            "a blocked image cannot silently become ordinal zero"
        );
        assert!(
            current_ordinal_candidates(
                &snapshot,
                &remote_eligible,
                2,
                CollectionNavigationTargetKind::StillImage,
            )
            .targets
            .is_empty()
        );
    }

    #[test]
    fn image_group_does_not_promote_a_partner_when_the_requested_target_disappears() {
        let target_id = CollectionEntryId::from_uuid(
            Uuid::parse_str("11111111-1111-4111-8111-111111111111").unwrap(),
        );
        let partner_id = CollectionEntryId::from_uuid(
            Uuid::parse_str("22222222-2222-4222-8222-222222222222").unwrap(),
        );
        let slot = |id: CollectionEntryId| PersistentCollectionPageSlot {
            identity: PersistentCollectionIdentity {
                entry_id: id.to_string(),
                source_identity: "a".repeat(64),
            },
            address: RemoteAddress::file(r"C:\collection\image.jpg"),
            role: RemotePagePresentationRole::Navigation,
        };
        assert!(!image_group_retains_target(&[], target_id));
        assert!(!image_group_retains_target(&[slot(partner_id)], target_id));
        assert!(image_group_retains_target(&[slot(target_id)], target_id));
        assert!(image_group_retains_target(
            &[slot(target_id), slot(partner_id)],
            target_id,
        ));
    }

    #[test]
    fn blocked_wire_entry_is_excluded_from_count_first_and_ordinal_zero() {
        let first = prepared_entry(
            "11111111-1111-4111-8111-111111111111",
            r"C:\collection\blocked.jpg",
        );
        let second = prepared_entry(
            "22222222-2222-4222-8222-222222222222",
            r"C:\collection\available.jpg",
        );
        let snapshot = prepared(vec![first, second.clone()]);
        let blocked = PersistentCollectionEntry {
            identity: wire_identity(
                snapshot.entries[0].entry_id,
                &snapshot.entries[0].source_key,
            ),
            name: "blocked.jpg".to_owned(),
            state: PersistentCollectionEntryState::BlockedByRemotePolicy {
                last_known_kind: Some(RemoteEntryKind::Image),
            },
        };
        let available = PersistentCollectionEntry {
            identity: wire_identity(
                snapshot.entries[1].entry_id,
                &snapshot.entries[1].source_key,
            ),
            name: "available.jpg".to_owned(),
            state: PersistentCollectionEntryState::Available {
                address: RemoteAddress::file(r"C:\collection\available.jpg"),
                kind: RemoteEntryKind::Image,
                thumbnail_address: None,
                detail: None,
                rating: None,
            },
        };
        let wires = [blocked, available];
        let mut projection = Vec::new();
        let retained = stream_bounded_wire_entries(
            &wires,
            64 * 1024,
            || true,
            Clone::clone,
            |prepared_index, wire| {
                if let Some(kind) = wire_available_resolved_kind(wire) {
                    projection.push(RemoteEligibleEntry {
                        prepared_index,
                        kind,
                    });
                }
            },
        )
        .unwrap();
        assert_eq!(
            retained.entries.len(),
            2,
            "blocked rows remain in the root prefix"
        );
        assert_eq!(
            projection
                .iter()
                .filter(|entry| entry.kind == CollectionResolvedKind::Image)
                .count(),
            1,
            "snapshot image_count uses the Remote projection"
        );
        let first = endpoint_candidates(
            &snapshot,
            &projection,
            CollectionNavigationTargetKind::StillImage,
            false,
        );
        assert_eq!(first.targets[0].entry_id, second.entry_id);
        let ordinal = current_ordinal_candidates(
            &snapshot,
            &projection,
            0,
            CollectionNavigationTargetKind::StillImage,
        );
        assert_eq!(ordinal.targets[0].entry_id, second.entry_id);
        assert_eq!(
            remote_eligible_entries(
                &snapshot,
                &projection,
                PersistentCollectionNavigationKind::StillImage,
            )
            .len(),
            1,
            "landed target_count uses the same Remote projection"
        );
    }

    #[test]
    fn response_prefix_never_cuts_an_interleaved_spread_unit() {
        let facts = PersistentCollectionViewFacts {
            token: "token".to_owned(),
            configured: RemoteSpreadMode::Ltr,
            effective: RemoteSpreadMode::Ltr,
            direction: RemoteReadingDirection::Ltr,
            entries: Arc::from([]),
            remote_eligible: Arc::from([]),
            groups: Arc::from([crate::ui_fullscreen::RemotePageGroupSpec {
                indices: vec![0, 2],
                slice: crate::page_split::PageSlice::Full,
                singleton_placement:
                    crate::displayed_image_transform::SingletonSpreadPlacement::Center,
                presentation: None,
            }]),
        };
        assert_eq!(complete_display_unit_prefix(&facts, 2, || true), Some(0));
        assert_eq!(complete_display_unit_prefix(&facts, 3, || true), Some(3));
    }

    #[test]
    fn long_field_wire_retention_is_single_pass_and_budget_bounded() {
        let inputs = vec![(); 100_000];
        let started = Instant::now();
        let mut converted = 0usize;
        let retained = stream_bounded_wire_entries(
            &inputs,
            32 * 1024,
            || true,
            |_| {
                converted += 1;
                PersistentCollectionEntry {
                    identity: PersistentCollectionIdentity {
                        entry_id: "11111111-1111-4111-8111-111111111111".to_owned(),
                        source_identity: "a".repeat(64),
                    },
                    name: "n".repeat(1024),
                    state: PersistentCollectionEntryState::Available {
                        address: RemoteAddress::file(format!(
                            r"C:\long\{}\image.jpg",
                            "p".repeat(2048)
                        )),
                        kind: RemoteEntryKind::Image,
                        thumbnail_address: None,
                        detail: None,
                        rating: None,
                    },
                }
            },
            |_, _| {},
        )
        .unwrap();
        assert_eq!(retained.observed, 100_000);
        assert_eq!(
            converted, 100_000,
            "each prepared entry is converted exactly once"
        );
        assert!(started.elapsed() < EXACT_REQUEST_BUDGET);
        assert!(retained.retained_bytes < 32 * 1024);
        assert!(retained.entries.len() < 100_000);
        assert!(!retained.accepting);
    }

    #[test]
    fn wire_entry_does_not_publish_a_stale_prepared_kind() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("changed.jpg");
        std::fs::write(&path, b"image").unwrap();
        let source = CollectionSourcePath::from_trusted(&path).unwrap();
        let entry = PreparedCollectionEntry {
            entry_id: crate::collection_store::CollectionEntryId::new(),
            source_key: source.key().clone(),
            source_path: path.clone(),
            availability: CollectionSourcePreparation::Available {
                kind: CollectionResolvedKind::Image,
                mtime: 1,
                file_size: Some(5),
            },
            item: crate::grid_item::GridItem::Image(path.clone()),
            display_meta: Some((1, 1)),
        };
        assert!(matches!(
            wire_entry(&entry).state,
            PersistentCollectionEntryState::Available {
                kind: RemoteEntryKind::Image,
                ..
            }
        ));

        std::fs::remove_file(&path).unwrap();
        std::fs::create_dir(&path).unwrap();
        assert!(matches!(
            wire_entry(&entry).state,
            PersistentCollectionEntryState::Unsupported {
                last_known_kind: Some(RemoteEntryKind::Image)
            }
        ));
    }
}
