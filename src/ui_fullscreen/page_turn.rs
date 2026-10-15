//! Paged presentation owns neither navigation nor producers. A prepared unit is submitted once.
use super::*;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, Weak};
use std::time::{Duration, Instant};

mod barrier;
#[cfg(test)]
mod cover_tests;
#[cfg(test)]
mod input_regression_tests;
#[cfg(test)]
mod landscape_tests;
#[cfg(all(test, windows))]
mod merge_master_tests;
#[cfg(all(test, windows))]
mod merge_tests;
#[cfg(test)]
mod nonfinal_tests;
#[cfg(all(test, windows))]
mod permission_service_regression_tests;
#[cfg(test)]
mod roi_tests;
#[cfg(test)]
mod slideshow_regression_tests;
#[cfg(test)]
mod spine_budget_tests;
use barrier::{FlatCompletion, ResolvedPageTurnNavigation};

static NEXT_OPERATION_BATCH: AtomicU64 = AtomicU64::new(1);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct PresentationDestination {
    pub(crate) context: crate::app::ViewerContextId,
    pub(crate) window: Option<u64>,
    pub(crate) viewport: egui::ViewportId,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ViewerOperationId {
    Key(crate::key_input::KeyOperationId),
    Pointer {
        batch: u64,
        source: egui::ViewportId,
        ordinal: u32,
    },
    Pad {
        batch: u64,
        ordinal: u32,
    },
    Slideshow(u64),
    /// Diagnostic identity at an accepted handler boundary with missing ingress.
    /// Never eligible to start an effect or invent a physical input edge.
    UnattributedNavigation(u64),
}

pub(crate) fn next_operation_batch() -> u64 {
    NEXT_OPERATION_BATCH.fetch_add(1, Ordering::Relaxed)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum OperationPhase {
    Press,
    Repeat,
    Continuous,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct ViewerOperationFact {
    pub(crate) destination: PresentationDestination,
    pub(crate) operation_id: ViewerOperationId,
    pub(crate) source: egui::ViewportId,
    pub(crate) phase: OperationPhase,
    pub(crate) cause: &'static str,
    pub(crate) diagnostic_input_seq: u64,
}

/// Raw pointer and focus-loss deliveries survive consumers and egui replay.
/// Passive hover creates no operation, and focus-loss is never a navigation candidate.
#[derive(Clone, Default)]
struct PointerTransaction {
    frame: u64,
    facts: Vec<(PointerDelivery, OperationPhase, &'static str)>,
}

#[derive(Clone, Copy, Default)]
enum PointerDelivery {
    #[default]
    Empty,
    Unrouted(ViewerOperationId),
    Routed(ViewerOperationId, PresentationDestination),
}

impl PointerDelivery {
    fn id(self) -> Option<ViewerOperationId> {
        match self {
            Self::Empty => None,
            Self::Unrouted(id) | Self::Routed(id, _) => Some(id),
        }
    }
    fn route(self, destination: PresentationDestination) -> Option<Self> {
        match self {
            Self::Empty => None,
            Self::Unrouted(id) => Some(Self::Routed(id, destination)),
            Self::Routed(_, original) if original != destination => None,
            already => Some(already),
        }
    }
}

#[derive(Clone, Default)]
struct PointerPresses {
    buttons: [PointerDelivery; 5],
    touches: Vec<((egui::TouchDeviceId, egui::TouchId), PointerDelivery)>,
}

fn observe_pointer_transaction(ctx: &egui::Context) {
    let viewport = ctx.viewport_id();
    let id = egui::Id::new(("page_turn_pointer_transaction", viewport));
    let frame = ctx.cumulative_frame_nr();
    if ctx
        .data(|d| d.get_temp::<PointerTransaction>(id))
        .is_some_and(|t| t.frame == frame)
    {
        return;
    }
    let mut facts = Vec::new();
    let batch = next_operation_batch();
    let press_id = egui::Id::new(("page_turn_pointer_presses", viewport));
    let mut presses = ctx
        .data(|d| d.get_temp::<PointerPresses>(press_id))
        .unwrap_or_default();
    ctx.input(|i| {
        for (ordinal, event) in i.raw.events.iter().enumerate() {
            let operation_id = ViewerOperationId::Pointer {
                batch,
                source: viewport,
                ordinal: ordinal as u32,
            };
            let delivery = PointerDelivery::Unrouted(operation_id);
            let fact = match event {
                egui::Event::PointerButton {
                    button, pressed, ..
                } => {
                    let slot = &mut presses.buttons[*button as usize];
                    if *pressed {
                        *slot = delivery;
                        Some((delivery, OperationPhase::Press, "pointer"))
                    } else {
                        // Click navigation occurs on release; it inherits the
                        // exact press delivery and cannot cancel as a new edge.
                        let original = *slot;
                        *slot = PointerDelivery::Empty;
                        original
                            .id()
                            .map(|_| (original, OperationPhase::Press, "pointer"))
                    }
                }
                egui::Event::MouseWheel { .. } => Some((delivery, OperationPhase::Press, "wheel")),
                egui::Event::WindowFocused(false) => {
                    Some((delivery, OperationPhase::Continuous, "focus_lost"))
                }
                egui::Event::PointerMoved(_) if i.pointer.any_down() => {
                    Some((delivery, OperationPhase::Continuous, "drag"))
                }
                egui::Event::Touch {
                    device_id,
                    id,
                    phase: egui::TouchPhase::Start,
                    ..
                } => {
                    presses
                        .touches
                        .retain(|(contact, _)| *contact != (*device_id, *id));
                    presses.touches.push(((*device_id, *id), delivery));
                    Some((delivery, OperationPhase::Press, "touch"))
                }
                egui::Event::Touch {
                    phase: egui::TouchPhase::Move,
                    ..
                } => Some((delivery, OperationPhase::Continuous, "drag")),
                egui::Event::Touch {
                    device_id,
                    id,
                    phase: egui::TouchPhase::End | egui::TouchPhase::Cancel,
                    ..
                } => {
                    let contact = presses
                        .touches
                        .iter()
                        .position(|(contact, _)| *contact == (*device_id, *id));
                    contact.and_then(|index| {
                        let (_, original) = presses.touches.remove(index);
                        matches!(
                            event,
                            egui::Event::Touch {
                                phase: egui::TouchPhase::End,
                                ..
                            }
                        )
                        .then_some((
                            original,
                            OperationPhase::Press,
                            "touch",
                        ))
                    })
                }
                _ => None,
            };
            if let Some(fact) = fact {
                if fact.2 != "focus_lost" && fact.0.id() == Some(operation_id) {
                    crate::key_input::trace_page_turn_operation_ingress(
                        || format!("{operation_id:?}"),
                        viewport,
                        None,
                        match fact.1 {
                            OperationPhase::Press => "Press",
                            OperationPhase::Repeat => "Repeat",
                            OperationPhase::Continuous => "Continuous",
                        },
                        "pointer_raw_transaction",
                    );
                }
                if !facts
                    .iter()
                    .any(|(previous, _, _)| PointerDelivery::id(*previous) == fact.0.id())
                {
                    facts.push(fact);
                }
            }
        }
    });
    ctx.data_mut(|d| {
        d.insert_temp(id, PointerTransaction { frame, facts });
        d.insert_temp(press_id, presses);
    });
}

pub(crate) fn install(ctx: &egui::Context) {
    let id = egui::Id::new("page_turn_pointer_tracking_installed");
    if ctx.data(|d| d.get_temp::<bool>(id)).unwrap_or(false) {
        return;
    }
    ctx.data_mut(|d| d.insert_temp(id, true));
    ctx.add_plugin(SubmissionPlugin::default());
    ctx.on_begin_pass(
        "page_turn_pointer_tracking",
        Arc::new(|ctx| {
            if ctx
                .data(|d| d.get_temp::<bool>(egui::Id::new("page_turn_input_enabled")))
                .unwrap_or(false)
            {
                observe_pointer_transaction(ctx);
            }
        }),
    );
}

#[derive(Clone)]
pub(super) struct PreparedPagedPage {
    pub(super) occurrence: SpreadPageOccurrence,
    pub(super) transform: DisplayedImageTransform,
    pub(super) resource: FullscreenPaintResource,
    pub(super) painter: egui::Painter,
    pub(super) underlay: FsBgStyle,
    pub(super) live: bool,
    /// Normal spread layout's resolved spine and gutter, before texture paint rounding.
    pub(super) spine: Option<(f32, f32)>,
}

#[derive(Clone)]
pub(super) struct SubmittedUnitReceipt {
    destination: PresentationDestination,
    frame: u64,
    pass: u64,
    pages: ReceiptPages,
    accepted: AcceptanceEvidence,
    pub(super) curl: bool,
}

type ReceiptPage = (SpreadPageOccurrence, egui::TextureId, egui::TextureId, f32);

#[derive(Clone, Copy)]
enum ReceiptPages {
    Single(ReceiptPage),
    Spread([ReceiptPage; 2]),
}

impl ReceiptPages {
    fn as_slice(&self) -> &[ReceiptPage] {
        match self {
            Self::Single(p) => std::slice::from_ref(p),
            Self::Spread(p) => p,
        }
    }
    fn as_mut_slice(&mut self) -> &mut [ReceiptPage] {
        match self {
            Self::Single(p) => std::slice::from_mut(p),
            Self::Spread(p) => p,
        }
    }
}

#[derive(Clone)]
enum AcceptanceEvidence {
    Immediate,
    Deferred(Arc<AtomicBool>),
}

impl AcceptanceEvidence {
    fn accept(&self) {
        if let Self::Deferred(accepted) = self {
            accepted.store(true, Ordering::Release);
        }
    }
    fn was_accepted(&self) -> bool {
        match self {
            Self::Immediate => true,
            Self::Deferred(accepted) => accepted.load(Ordering::Acquire),
        }
    }
}

impl SubmittedUnitReceipt {
    pub(super) fn matches(
        &self,
        destination: PresentationDestination,
        frame: u64,
        sources: &FsNavigatorTextureSources,
    ) -> bool {
        self.frame == frame && self.matches_sources(destination, sources)
    }
    pub(super) fn matches_sources(
        &self,
        destination: PresentationDestination,
        sources: &FsNavigatorTextureSources,
    ) -> bool {
        !self.curl
            && self.destination == destination
            && self.pages.as_slice().len() == sources.pages.len()
            && self
                .pages
                .as_slice()
                .iter()
                .all(|(occurrence, source, native, area)| {
                    *area > 0.0
                        && sources.pages.iter().any(|p| {
                            p.occurrence == *occurrence
                                && p.resource.source_texture_id() == *source
                                && p.resource.paint_texture_id() == *native
                        })
                })
    }
    pub(super) fn was_accepted(&self) -> bool {
        self.accepted.was_accepted()
    }
    pub(super) fn frame(&self) -> u64 {
        self.frame
    }
    pub(super) fn pass(&self) -> u64 {
        self.pass
    }
    pub(super) fn destination(&self) -> PresentationDestination {
        self.destination
    }
    pub(super) fn pages(&self) -> &[(SpreadPageOccurrence, egui::TextureId, egui::TextureId, f32)] {
        self.pages.as_slice()
    }
    pub(super) fn accepted_ledger(&self) -> Option<Arc<AtomicBool>> {
        match &self.accepted {
            AcceptanceEvidence::Immediate => None,
            AcceptanceEvidence::Deferred(accepted) => Some(accepted.clone()),
        }
    }
    pub(super) fn rebind(&mut self, occurrences: &[SpreadPageOccurrence]) {
        for (page, _, _, _) in self.pages.as_mut_slice() {
            if let Some(occurrence) = occurrences
                .iter()
                .find(|o| o.idx == page.idx && o.role == page.role)
            {
                *page = *occurrence;
            }
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SubmissionStatus {
    Offered,
    Started,
    Finished,
    Cancelled,
}

struct SubmissionTracker {
    ledger: Mutex<SubmissionLedger>,
}

struct SubmissionLedger {
    status: SubmissionStatus,
    visible_pages: u8,
    canonical_visible: bool,
}

impl SubmissionTracker {
    fn new() -> Self {
        Self {
            ledger: Mutex::new(SubmissionLedger {
                status: SubmissionStatus::Offered,
                visible_pages: 0,
                canonical_visible: false,
            }),
        }
    }
    fn status(&self) -> SubmissionStatus {
        self.ledger.lock().unwrap_or_else(|p| p.into_inner()).status
    }
    fn end(&self) -> SubmissionStatus {
        let mut state = self.ledger.lock().unwrap_or_else(|p| p.into_inner());
        let previous = state.status;
        state.status = SubmissionStatus::Cancelled;
        previous
    }
}

#[derive(Clone)]
struct PendingSubmission {
    receipt: SubmittedUnitReceipt,
    event: Option<SubmissionEvent>,
}

#[derive(Clone)]
struct SubmissionEvent {
    tracker: Arc<SubmissionTracker>,
    fact: ViewerOperationFact,
    kind: SubmissionKind,
    elapsed_ms: f64,
    retained_bytes: u64,
    mesh_ms: f64,
    silhouette_visibility_ms: f64,
    tessellation_ms: f64,
    outer_mask_ms: f64,
    visible_pages: u8,
    proof: Option<serde_json::Value>,
}

#[derive(Clone, Copy)]
enum SubmissionKind {
    Curl,
    Canonical,
}

/// Finalizes after all UI widgets and egui end-pass work. Discarded projection
/// passes have leases but never become first-visible/start/finish evidence.
#[derive(Default)]
struct SubmissionPlugin {
    pending: std::collections::HashMap<egui::ViewportId, PendingSubmission>,
    output: Option<(egui::ViewportId, Option<PendingSubmission>, usize)>,
    // A deadline drops effect resources immediately. Only its diagnostic
    // completion receipt survives a discarded pass, until the next output.
    canonical: std::collections::HashMap<egui::ViewportId, PendingSubmission>,
}

impl egui::Plugin for SubmissionPlugin {
    fn debug_name(&self) -> &'static str {
        "page_turn_submitted_output"
    }
    fn on_begin_pass(&mut self, ctx: &egui::Context) {
        self.pending.remove(&ctx.viewport_id());
    }
    fn on_end_pass(&mut self, ctx: &egui::Context) {
        let viewport = ctx.viewport_id();
        self.output = Some((
            viewport,
            self.pending.remove(&viewport),
            ctx.options(|o| o.max_passes.get()),
        ));
    }
    fn output_hook(&mut self, output: &mut egui::FullOutput) {
        let Some((viewport, pending, max_passes)) = self.output.take() else {
            return;
        };
        if output.platform_output.requested_discard()
            && output.platform_output.num_completed_passes < max_passes
        {
            if let Some(pending) = pending
                && matches!(
                    pending.event.as_ref().map(|e| e.kind),
                    Some(SubmissionKind::Canonical)
                )
            {
                self.canonical.insert(viewport, pending);
            }
            return;
        }
        let Some(mut pending) = pending else {
            if let Some(completion) = self.canonical.remove(&viewport) {
                cancel_pending_submission(completion, "canonical_submission_missing");
            }
            return;
        };
        if let Some(completion) = self.canonical.remove(&viewport) {
            if pending.event.is_none()
                && same_submitted_pages(&completion.receipt, &pending.receipt)
            {
                pending.event = completion.event;
            } else if !same_submitted_pages(&completion.receipt, &pending.receipt) {
                cancel_pending_submission(completion, "canonical_source_changed");
            }
        }
        pending.receipt.accepted.accept();
        let Some(event) = pending.event else {
            return;
        };
        let mut ledger = event
            .tracker
            .ledger
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        let mut fields = Vec::new();
        if crate::perf::is_enabled() {
            fields.extend([
                ("elapsed_ms", event.elapsed_ms.into()),
                ("retain_bytes", event.retained_bytes.into()),
                ("mesh_cpu_ms", event.mesh_ms.into()),
                (
                    "silhouette_visibility_cpu_ms",
                    event.silhouette_visibility_ms.into(),
                ),
                ("tessellation_cpu_ms", event.tessellation_ms.into()),
                ("outer_mask_cpu_ms", event.outer_mask_ms.into()),
                ("frame", pending.receipt.frame.into()),
                ("pass", pending.receipt.pass.into()),
                ("clock_origin", "accepted_navigation".into()),
            ]);
            if let Some(proof) = event.proof {
                fields.push(("unit", proof));
            }
            fields.push(("submitted_pages",serde_json::Value::Array(pending.receipt.pages().iter().map(|(occurrence,source,native,area)|serde_json::json!({"idx":occurrence.idx,"role":format!("{:?}",occurrence.role),"navigation_anchor_idx":occurrence.navigation_anchor_idx,"source_texture_id":format!("{source:?}"),"native_texture_id":format!("{native:?}"),"visible_area":area})).collect())));
        }
        match (event.kind, ledger.status) {
            (SubmissionKind::Curl, SubmissionStatus::Offered) => {
                ledger.status = SubmissionStatus::Started;
                emit(&event.fact, "start", "curl_submitted", &fields);
                emit(&event.fact, "frame", "curl_submitted", &fields);
            }
            (SubmissionKind::Curl, SubmissionStatus::Started) => {
                emit(&event.fact, "frame", "curl_submitted", &fields);
            }
            (SubmissionKind::Canonical, SubmissionStatus::Started) => {
                ledger.status = SubmissionStatus::Finished;
                ledger.canonical_visible = true;
                emit(&event.fact, "finish", "canonical_submitted", &fields);
                emit(&event.fact, "fully_visible", "canonical_submitted", &fields);
            }
            (SubmissionKind::Canonical, SubmissionStatus::Cancelled)
                if !ledger.canonical_visible =>
            {
                ledger.canonical_visible = true;
                emit(
                    &event.fact,
                    "fully_visible",
                    "canonical_after_cancel",
                    &fields,
                );
            }
            (SubmissionKind::Canonical, SubmissionStatus::Offered) => {
                ledger.status = SubmissionStatus::Cancelled;
                emit(&event.fact, "skip", "expired_before_accepted_curl", &[]);
            }
            _ => {}
        }
        if matches!(
            ledger.status,
            SubmissionStatus::Started | SubmissionStatus::Finished
        ) || matches!(event.kind, SubmissionKind::Canonical) && ledger.canonical_visible
        {
            for (i, (occurrence, _, _, area)) in pending.receipt.pages().iter().enumerate() {
                let bit = 1 << i;
                if event.visible_pages & bit != 0 && ledger.visible_pages & bit == 0 {
                    ledger.visible_pages |= bit;
                    let mut page_fields = fields.clone();
                    page_fields.extend([
                        ("idx", occurrence.idx.into()),
                        ("role", format!("{:?}", occurrence.role).into()),
                        ("visible_area", (*area).into()),
                    ]);
                    emit(
                        &event.fact,
                        "first_visible",
                        if pending.receipt.curl {
                            "curl_page_submitted"
                        } else {
                            "canonical_page_submitted"
                        },
                        &page_fields,
                    );
                }
            }
        }
    }
}

fn same_submitted_pages(a: &SubmittedUnitReceipt, b: &SubmittedUnitReceipt) -> bool {
    a.destination == b.destination
        && a.pages.as_slice().len() == b.pages.as_slice().len()
        && a.pages.as_slice().iter().zip(b.pages.as_slice()).all(
            |((oa, sa, na, _), (ob, sb, nb, area))| oa == ob && sa == sb && na == nb && *area > 0.0,
        )
}

fn cancel_pending_submission(pending: PendingSubmission, reason: &'static str) {
    if let Some(event) = pending.event {
        let status = event.tracker.end();
        if status == SubmissionStatus::Started {
            emit(&event.fact, "cancel", reason, &[]);
        } else if status == SubmissionStatus::Offered {
            emit(&event.fact, "skip", reason, &[]);
        }
    }
}

#[derive(Clone)]
struct PaintedUnit {
    destination: PresentationDestination,
    items: u64,
    layout: Arc<Vec<SpreadDisplayUnit>>,
    mode: SpreadMode,
    reading_rtl: bool,
    slice: crate::page_split::PageSlice,
    pages: Vec<PreparedPagedPage>,
    white: Option<(egui::Rect, egui::Rect)>,
    reserved_bytes: Arc<AtomicU64>,
    evidence: AcceptanceEvidence,
    content: Vec<PageTurnContent>,
}

/// Quality publication changes source_gen, not the document edits or viewing policy.
#[derive(Clone, PartialEq)]
struct PageTurnContent {
    edit: crate::app::EditResultKey,
    params: crate::adjustment::AdjustParams,
    comic_generation: u64,
    gap: u32,
}

#[derive(Clone)]
struct AcceptedSource {
    occurrence: SpreadPageOccurrence,
    resource: crate::gpu_lanczos::FullscreenPaintResource,
    quality: &'static str,
    content: PageTurnContent,
}

struct AcceptedSources(Vec<AcceptedSource>);

impl AcceptedSources {
    fn account(&self, accountant: &mut crate::vram_budget::VramAccountant) {
        for source in &self.0 {
            accountant.add_texture(
                crate::vram_budget::VramSubsystem::PageTurnPresentation,
                source.resource.source_texture(),
                true,
            );
        }
    }
}

impl PaintedUnit {
    fn select_flat(previous: Arc<Self>, current: &Arc<Self>) -> Arc<Self> {
        if previous.evidence.was_accepted() && previous.same_material_and_geometry(current) {
            previous
        } else {
            current.clone()
        }
    }
    fn same_material_and_geometry(&self, other: &Self) -> bool {
        self.destination == other.destination
            && self.items == other.items
            && Arc::ptr_eq(&self.layout, &other.layout)
            && self.mode == other.mode
            && self.reading_rtl == other.reading_rtl
            && self.slice == other.slice
            && self.white == other.white
            && self.pages.len() == other.pages.len()
            && self.pages.iter().zip(&other.pages).all(|(a, b)| {
                a.occurrence == b.occurrence
                    && a.resource.source_texture_id() == b.resource.source_texture_id()
                    && a.resource.paint_texture_id() == b.resource.paint_texture_id()
                    && a.resource.source_generation() == b.resource.source_generation()
                    && a.resource.visible_source_uv_rect() == b.resource.visible_source_uv_rect()
                    && a.transform.paint_quad() == b.transform.paint_quad()
                    && a.transform.uv_rect == b.transform.uv_rect
                    && a.transform.fit_input == b.transform.fit_input
                    && a.transform.fit_scale_limits == b.transform.fit_scale_limits
                    && a.transform.pixels_per_point == b.transform.pixels_per_point
                    && a.spine == b.spine
                    && a.painter.clip_rect() == b.painter.clip_rect()
                    && same_underlay(&a.underlay, &b.underlay)
            })
    }
    fn retained_bytes(&self) -> u64 {
        let mut accountant = crate::vram_budget::VramAccountant::default();
        self.account(&mut accountant);
        accountant.finish().total_bytes()
    }
    fn account(&self, accountant: &mut crate::vram_budget::VramAccountant) {
        for page in &self.pages {
            accountant.add_texture(
                crate::vram_budget::VramSubsystem::PageTurnPresentation,
                page.resource.source_texture(),
                true,
            );
            if let Some(output) = page.resource.lanczos_output() {
                accountant.add_texture_id(
                    crate::vram_budget::VramSubsystem::PageTurnPresentation,
                    output.texture_id(),
                    output.size(),
                    false,
                );
            }
            if let FsBgStyle::Checker(texture) = &page.underlay {
                accountant.add_texture(
                    crate::vram_budget::VramSubsystem::PageTurnPresentation,
                    texture,
                    false,
                );
            }
        }
    }
}

fn same_fit_policy(a: FullscreenFitInput, b: FullscreenFitInput) -> bool {
    match (a, b) {
        (
            FullscreenFitInput::SpecifiedScale {
                size: a,
                max_tenths: am,
                ..
            },
            FullscreenFitInput::SpecifiedScale {
                size: b,
                max_tenths: bm,
                ..
            },
        ) => a == b && am == bm,
        _ => a.mode() == b.mode(),
    }
}

// Reservations are globally deduplicated and include both normal sources and
// prepared ROI outputs. The old 128 MiB ceiling rejected resident 8K comics even
// with ample configured VRAM (2026-10-11: 737 MiB measured, 756 MiB with
// the incoming ROI estimate from previous ordinary prepares).
fn page_turn_pin_limit(budget: crate::vram_budget::VramPoolBudget) -> u64 {
    budget
        .fullscreen_bytes
        .map_or(1024 * 1024 * 1024, |bytes| bytes / 10)
}

fn same_underlay(a: &FsBgStyle, b: &FsBgStyle) -> bool {
    match (a, b) {
        (FsBgStyle::Solid(a), FsBgStyle::Solid(b)) => a == b,
        (FsBgStyle::Checker(a), FsBgStyle::Checker(b)) => a.id() == b.id(),
        _ => false,
    }
}

/// Weak inventory includes mounted, parked and in-flight batches without owning texels.
#[derive(Default)]
pub(crate) struct PageTurnLeaseInventory {
    units: Vec<Weak<PaintedUnit>>,
    sources: Vec<Weak<AcceptedSources>>,
}

impl PageTurnLeaseInventory {
    fn register(&mut self, unit: &Arc<PaintedUnit>) {
        self.units.retain(|u| u.strong_count() != 0);
        let weak = Arc::downgrade(unit);
        if !self.units.iter().any(|u| Weak::ptr_eq(u, &weak)) {
            self.units.push(weak);
        }
    }
    pub(crate) fn account(&self, accountant: &mut crate::vram_budget::VramAccountant) {
        for unit in self.units.iter().filter_map(Weak::upgrade) {
            unit.account(accountant);
        }
        for sources in self.sources.iter().filter_map(Weak::upgrade) {
            sources.account(accountant);
        }
    }
    fn reserved_bytes(&self) -> u64 {
        let mut accountant = crate::vram_budget::VramAccountant::default();
        self.account_reserved(&mut accountant);
        accountant.finish().total_bytes()
    }
    fn account_reserved(&self, accountant: &mut crate::vram_budget::VramAccountant) {
        for unit in self.units.iter().filter_map(Weak::upgrade) {
            if unit.reserved_bytes.load(Ordering::Relaxed) != 0 {
                unit.account(accountant);
            }
        }
        for sources in self.sources.iter().filter_map(Weak::upgrade) {
            sources.account(accountant);
        }
    }
    fn reservation_bytes_with(&self, unit: &PaintedUnit) -> u64 {
        let mut accountant = crate::vram_budget::VramAccountant::default();
        self.account_reserved(&mut accountant);
        unit.account(&mut accountant);
        accountant.finish().total_bytes()
    }
}

struct AcceptedOffer {
    fact: ViewerOperationFact,
    ready_sources: Arc<AcceptedSources>,
    resident_bytes: u64,
    pin_bytes: u64,
    accepted_at: Instant,
    duration: Duration,
    forward: bool,
    submission: Arc<SubmissionTracker>,
}

struct PreparedLeaf {
    outgoing: crate::page_turn_animation::CanonicalUnit,
    incoming: crate::page_turn_animation::CanonicalUnit,
    direction: crate::page_turn_animation::TurnDirection,
    recipe: crate::page_turn_animation::TurnRecipe,
    prepare_cpu_ms: f64,
}

fn prepare_leaf(
    outgoing: &PaintedUnit,
    incoming: &PaintedUnit,
    forward: bool,
) -> Result<PreparedLeaf, &'static str> {
    let start = crate::perf::is_enabled().then(Instant::now);
    let outgoing = canonical_unit(outgoing)?;
    let input = canonical_unit(incoming)?;
    let direction = crate::page_turn_animation::TurnDirection {
        reading_rtl: incoming.reading_rtl,
        forward,
    };
    let recipe = crate::page_turn_animation::prepare_turn(&outgoing, &input, direction)
        .map_err(|e| e.sub_check())?;
    Ok(PreparedLeaf {
        outgoing,
        incoming: input,
        direction,
        recipe,
        prepare_cpu_ms: start.map_or(0.0, |s| s.elapsed().as_secs_f64() * 1000.0),
    })
}

enum PresentationState {
    Inactive,
    Flat(Arc<PaintedUnit>),
    Offered {
        outgoing: Arc<PaintedUnit>,
        offer: AcceptedOffer,
    },
    Turning {
        outgoing: Arc<PaintedUnit>,
        incoming: Arc<PaintedUnit>,
        offer: AcceptedOffer,
        projection: PreparedLeaf,
    },
    Completing {
        incoming: Arc<PaintedUnit>,
        completion: FlatCompletion,
        continuation: Option<ResolvedPageTurnNavigation>,
        diagnostics: CompletionDiagnostics,
    },
}

/// Correlates the mandatory flat with the original turn without keeping its
/// outgoing resources or an effect clock alive.
struct CompletionDiagnostics {
    fact: ViewerOperationFact,
    tracker: Arc<SubmissionTracker>,
    accepted_at: Instant,
}

impl CompletionDiagnostics {
    fn from_offer(offer: &AcceptedOffer) -> Self {
        Self {
            fact: offer.fact,
            tracker: offer.submission.clone(),
            accepted_at: offer.accepted_at,
        }
    }
}

/// Input observation is a transaction projection, never a navigation request or queue.
#[derive(Default)]
struct OperationTransaction {
    facts: Vec<ViewerOperationFact>,
    disposition: OperationDisposition,
    observed_frame: Option<(egui::ViewportId, u64)>,
}

#[derive(Default, PartialEq, Eq)]
enum OperationDisposition {
    #[default]
    Available,
    Claimed,
    Cancelled,
    Excluded,
}

fn same_operation(a: &ViewerOperationFact, b: &ViewerOperationFact) -> bool {
    a.destination == b.destination
        && a.operation_id == b.operation_id
        && a.source == b.source
        && a.phase == b.phase
}

pub(crate) struct PageTurnPresentation {
    state: PresentationState,
    transaction: OperationTransaction,
}

impl Default for PageTurnPresentation {
    fn default() -> Self {
        Self {
            state: PresentationState::Inactive,
            transaction: OperationTransaction::default(),
        }
    }
}

impl PageTurnPresentation {
    pub(crate) fn has_flat_completion(&self) -> bool {
        matches!(self.state, PresentationState::Completing { .. })
    }
    pub(crate) fn cancel_for_settings(&mut self, leases: &mut PageTurnLeaseInventory) {
        self.cancel("settings_changed");
        match &mut self.state {
            PresentationState::Flat(_) => self.state = PresentationState::Inactive,
            PresentationState::Completing {
                incoming,
                completion,
                ..
            } => {
                // The old output's hook can still accept its receipt later.
                // Give the next canonical geometry its own acceptance evidence.
                *incoming = Arc::new(PaintedUnit {
                    evidence: AcceptanceEvidence::Deferred(Arc::new(AtomicBool::new(false))),
                    ..(**incoming).clone()
                });
                leases.register(incoming);
                *completion = FlatCompletion::NeedFlat;
            }
            _ => {}
        }
        // A continuation has its own immutable fact. Old raw input is not
        // eligible for a new offer when the setting is turned on again.
        self.transaction = OperationTransaction::default();
    }
    pub(crate) fn cancel(&mut self, reason: &'static str) {
        let old = std::mem::replace(&mut self.state, PresentationState::Inactive);
        match old {
            PresentationState::Turning {
                incoming, offer, ..
            } => {
                let previous = offer.submission.end();
                if matches!(
                    previous,
                    SubmissionStatus::Offered | SubmissionStatus::Started
                ) {
                    emit(
                        &offer.fact,
                        if previous == SubmissionStatus::Started {
                            "cancel"
                        } else {
                            "skip"
                        },
                        reason,
                        &[("retain_bytes", incoming.retained_bytes().into())],
                    );
                }
                self.state = PresentationState::Completing {
                    incoming,
                    completion: FlatCompletion::NeedFlat,
                    continuation: None,
                    diagnostics: CompletionDiagnostics::from_offer(&offer),
                };
                self.transaction.disposition = OperationDisposition::Cancelled;
            }
            PresentationState::Offered { outgoing, offer } => {
                emit(&offer.fact, "skip", reason, &[]);
                offer.submission.end();
                // No projected batch existed; ordinary flat leases remain accounted.
                outgoing.reserved_bytes.store(0, Ordering::Relaxed);
                self.state = PresentationState::Flat(outgoing);
                self.transaction.disposition = OperationDisposition::Cancelled;
            }
            other => self.state = other,
        }
    }
    pub(crate) fn deactivate(&mut self, reason: &'static str) {
        self.cancel(reason);
        self.state = PresentationState::Inactive;
        self.transaction = OperationTransaction::default();
    }
    pub(crate) fn observe(&mut self, facts: Vec<ViewerOperationFact>) {
        // Empty/poll observations cannot erase a claimed transaction and make
        // the same immutable batch eligible again during a replay pass.
        if facts.is_empty() {
            return;
        }
        if self.transaction.facts.len() == facts.len()
            && self
                .transaction
                .facts
                .iter()
                .all(|a| facts.iter().any(|b| same_operation(a, b)))
        {
            return;
        }
        let observed_frame = self.transaction.observed_frame;
        self.transaction = OperationTransaction {
            facts,
            disposition: OperationDisposition::Available,
            observed_frame,
        };
        let started = match &self.state {
            PresentationState::Offered { offer, .. } | PresentationState::Turning { offer, .. } => {
                Some(offer.fact)
            }
            _ => None,
        };
        if let Some(started) = started
            && self.transaction.facts.iter().any(|f| {
                f.destination == started.destination
                    && (f.operation_id != started.operation_id
                        || f.phase == OperationPhase::Continuous)
            })
        {
            self.cancel("new_operation");
        }
    }
    fn observe_for_frame(
        &mut self,
        frame: (egui::ViewportId, u64),
        mut facts: Vec<ViewerOperationFact>,
    ) {
        if facts.is_empty() {
            if self.transaction.observed_frame != Some(frame)
                && self.transaction.disposition == OperationDisposition::Available
            {
                self.transaction.facts.clear();
                self.transaction.observed_frame = Some(frame);
            }
            return;
        }
        if self.transaction.observed_frame == Some(frame) {
            for previous in &self.transaction.facts {
                if !facts.iter().any(|f| same_operation(f, previous)) {
                    facts.push(*previous);
                }
            }
        }
        self.observe(facts);
        self.transaction.observed_frame = Some(frame);
    }
    fn take_candidate(&mut self) -> Option<ViewerOperationFact> {
        let fact = (self.transaction.disposition == OperationDisposition::Available
            && self.transaction.facts.len() == 1)
            .then(|| self.transaction.facts[0])
            .filter(|f| f.phase == OperationPhase::Press)?;
        self.transaction.disposition = OperationDisposition::Claimed;
        Some(fact)
    }

    #[cfg(test)]
    pub(crate) fn continuation_operation_for_test(&self) -> Option<(ViewerOperationId, usize)> {
        match &self.state {
            PresentationState::Completing {
                continuation: Some(command),
                ..
            } => Some((command.fact.operation_id, command.target_idx)),
            _ => None,
        }
    }

    #[cfg(test)]
    pub(crate) fn seed_operation_for_test(
        &mut self,
        destination: PresentationDestination,
        batch: u64,
    ) {
        self.observe(vec![ViewerOperationFact {
            destination,
            operation_id: ViewerOperationId::Pad { batch, ordinal: 0 },
            source: destination.viewport,
            phase: OperationPhase::Press,
            cause: "test",
            diagnostic_input_seq: 0,
        }]);
    }

    #[cfg(test)]
    pub(crate) fn operation_for_test(&self) -> Option<(PresentationDestination, u64)> {
        self.transaction.facts.first().and_then(|f| {
            if let ViewerOperationId::Pad { batch, .. } = f.operation_id {
                Some((f.destination, batch))
            } else {
                None
            }
        })
    }
}

fn emit(
    fact: &ViewerOperationFact,
    kind: &'static str,
    reason: &'static str,
    extra: &[(&str, serde_json::Value)],
) {
    #[cfg(test)]
    TEST_EVENTS.with(|events| events.borrow_mut().push((*fact, kind, reason)));
    // One line per lifecycle transition, never per projected frame. A normal
    // log must distinguish an unoffered turn from a submitted curl cancelled
    // before the observer saw it; perf logging is optional in real reports.
    if matches!(kind, "skip" | "start" | "cancel" | "finish") {
        let sub_check = extra
            .iter()
            .find(|(key, _)| *key == "sub_check")
            .and_then(|(_, value)| value.as_str())
            .unwrap_or(reason);
        crate::logger::log(format!(
            "[page-turn] {kind} reason={reason} sub_check={sub_check} operation={:?} context={:?}",
            fact.operation_id, fact.destination.context
        ));
    }
    if !crate::perf::is_enabled() {
        return;
    }
    let mut fields = vec![
        ("context", format!("{:?}", fact.destination.context).into()),
        (
            "window",
            fact.destination
                .window
                .map_or_else(|| "main".into(), |id| id.to_string())
                .into(),
        ),
        (
            "viewport",
            format!("{:?}", fact.destination.viewport).into(),
        ),
        ("source_viewport", format!("{:?}", fact.source).into()),
        ("operation_id", format!("{:?}", fact.operation_id).into()),
        ("phase", format!("{:?}", fact.phase).into()),
        ("cause", fact.cause.into()),
        ("reason", reason.into()),
    ];
    fields.extend_from_slice(extra);
    crate::perf::event(
        "page_turn_animation",
        kind,
        None,
        fact.diagnostic_input_seq,
        &fields,
    );
}

#[cfg(test)]
thread_local! {
    static TEST_EVENTS: std::cell::RefCell<Vec<(ViewerOperationFact, &'static str, &'static str)>> = const { std::cell::RefCell::new(Vec::new()) };
}

#[cfg(test)]
pub(crate) fn take_test_events() -> Vec<(ViewerOperationFact, &'static str, &'static str)> {
    TEST_EVENTS.with(|events| std::mem::take(&mut *events.borrow_mut()))
}

impl App {
    /// Preferences and HUD adopt the same global setting before any next turn.
    pub(crate) fn adopt_page_turn_animation_settings(&mut self, enabled: bool, duration_ms: u32) {
        if self.settings.page_turn_animation_enabled != enabled
            || (enabled && self.settings.page_turn_animation_duration_ms != duration_ms)
        {
            self.cancel_all_page_turn_effects_for_settings();
        }
        crate::key_input::set_page_turn_ingress_instrumentation_enabled(enabled);
        self.settings.page_turn_animation_enabled = enabled;
        self.settings.page_turn_animation_duration_ms = duration_ms;
    }

    pub(crate) fn page_turn_pre_draw(&mut self, ctx: &egui::Context) {
        self.flush_submitted_page_turn_ready(ctx);
        if !self.settings.page_turn_animation_enabled && !self.page_turn_completion_active() {
            let enabled_id = egui::Id::new("page_turn_input_enabled");
            if ctx.data(|d| d.get_temp::<bool>(enabled_id)) == Some(true) {
                ctx.data_mut(|d| d.insert_temp(enabled_id, false));
            }
            self.page_turn_presentation.deactivate("disabled");
            return;
        }
        ctx.data_mut(|d| d.insert_temp(egui::Id::new("page_turn_input_enabled"), true));
        // The ordinary key handler resolves this same pass owner. Resolve it
        // before observing its immutable edges too: an absent cached owner is
        // deliberately fail-closed, not evidence that this pass blocks keys.
        let keyboard_owner = self.keyboard_owner_for_pass(ctx);
        observe_pointer_transaction(ctx);
        if ctx.viewport_id() == self.page_turn_destination().viewport
            && ctx.input(|i| i.viewport().minimized.unwrap_or(false))
        {
            self.page_turn_presentation.cancel("hidden");
            return;
        }
        if self.fs_loupe_locked || self.page_turn_loupe_hold(ctx) {
            self.page_turn_presentation.cancel("loupe");
            self.page_turn_presentation.transaction.disposition = OperationDisposition::Cancelled;
            return;
        }
        let destination = self.page_turn_destination();
        let source = ctx.viewport_id();
        let mut facts = crate::key_input::operation_key_edges(ctx, source)
            .into_iter()
            .filter(|e| e.pressed())
            .map(|e| ViewerOperationFact {
                destination,
                operation_id: ViewerOperationId::Key(e.operation_id()),
                source,
                phase: if e.repeat() {
                    OperationPhase::Repeat
                } else {
                    OperationPhase::Press
                },
                cause: "key",
                diagnostic_input_seq: self.input_seq,
            })
            .collect::<Vec<_>>();
        let key_admission_blocked =
            !facts.is_empty() && keyboard_owner.blocks_legacy_keymap_shortcuts();
        if key_admission_blocked {
            facts.clear();
        }
        let id = egui::Id::new(("page_turn_pointer_transaction", source));
        if let Some(mut transaction) = ctx.data(|d| d.get_temp::<PointerTransaction>(id)) {
            let press_id = egui::Id::new(("page_turn_pointer_presses", source));
            let mut presses = ctx
                .data(|d| d.get_temp::<PointerPresses>(press_id))
                .unwrap_or_default();
            for (delivery, phase, cause) in &mut transaction.facts {
                if *cause == "focus_lost" {
                    // Lifecycle belongs to the receiving window, not a forwarded
                    // key destination. Mark the real delivery once so a replay
                    // cannot cancel a new wheel accepted while still unfocused.
                    if source == destination.viewport {
                        let fresh = matches!(delivery, PointerDelivery::Unrouted(_));
                        if let Some(routed) = delivery.route(destination) {
                            *delivery = routed;
                            if fresh {
                                self.page_turn_presentation.cancel("focus_lost");
                            }
                        }
                    }
                    continue;
                }
                let Some(routed) = delivery.route(destination) else {
                    continue;
                };
                *delivery = routed;
                let operation_id = routed.id().expect("routed pointer delivery has identity");
                for active in &mut presses.buttons {
                    if active.id() == Some(operation_id) {
                        *active = routed;
                    }
                }
                for (_, active) in &mut presses.touches {
                    if active.id() == Some(operation_id) {
                        *active = routed;
                    }
                }
                facts.push(ViewerOperationFact {
                    destination,
                    operation_id,
                    source,
                    phase: *phase,
                    cause,
                    diagnostic_input_seq: self.input_seq,
                });
            }
            ctx.data_mut(|d| {
                d.insert_temp(id, transaction);
                d.insert_temp(press_id, presses);
            });
        }
        self.page_turn_presentation
            .observe_for_frame((ctx.viewport_id(), ctx.cumulative_frame_nr()), facts);
    }

    pub(crate) fn page_turn_external_operation(
        &mut self,
        ctx: &egui::Context,
        operation_id: ViewerOperationId,
        phase: OperationPhase,
        cause: &'static str,
    ) {
        self.page_turn_external_operations(ctx, &[(operation_id, phase, cause)]);
    }

    pub(crate) fn page_turn_external_operations(
        &mut self,
        ctx: &egui::Context,
        operations: &[(ViewerOperationId, OperationPhase, &'static str)],
    ) {
        if !self.settings.page_turn_animation_enabled && !self.page_turn_barrier_active() {
            return;
        }
        let destination = self.page_turn_destination();
        let mut facts = Vec::with_capacity(operations.len());
        for &(operation_id, phase, cause) in operations {
            if let ViewerOperationId::Pad { batch, .. } = operation_id {
                let id = egui::Id::new("page_turn_pad_destination");
                let original = ctx.data(|d| d.get_temp::<(u64, PresentationDestination)>(id));
                if original.is_some_and(|(old_batch, d)| old_batch == batch && d != destination) {
                    continue;
                }
                if original.is_none_or(|(old_batch, _)| old_batch != batch) {
                    ctx.data_mut(|d| d.insert_temp(id, (batch, destination)));
                }
            }
            if let ViewerOperationId::Slideshow(batch) = operation_id {
                let id = egui::Id::new("page_turn_slideshow_ingress");
                if ctx.data(|d| d.get_temp::<u64>(id)) != Some(batch) {
                    crate::key_input::trace_page_turn_operation_ingress(
                        || format!("{operation_id:?}"),
                        ctx.viewport_id(),
                        None,
                        "Press",
                        "slideshow_activation",
                    );
                    ctx.data_mut(|d| d.insert_temp(id, batch));
                }
            }
            facts.push(ViewerOperationFact {
                destination,
                operation_id,
                source: ctx.viewport_id(),
                phase,
                cause,
                diagnostic_input_seq: self.input_seq,
            });
        }
        self.page_turn_presentation
            .observe_for_frame((ctx.viewport_id(), ctx.cumulative_frame_nr()), facts);
    }

    pub(super) fn page_turn_view_eligible(&self, page_idx: usize) -> bool {
        self.settings.page_turn_animation_enabled
            && self.reading_flow.is_paged()
            && !self.fs_entry_is_animated(page_idx)
            && !self.is_panorama_mode_active(page_idx)
            && !self.is_overlay_edit_mode_active()
            && !self.analysis_mode
            && !self.adjustment_mode.is_open()
            && !self.fs_zoom_mode_engaged()
            && self.fs_zoom_pan().is_none()
            && self.fs_free_rotation.abs() <= TRANSFORM_EPSILON
            && self.sns_split.is_none()
            && self.capture_region_selection.is_none()
            && matches!(self.compare_view_mode, crate::app::CompareViewMode::Off)
            && !self.fs_loupe_locked
            && self
                .items
                .get(page_idx)
                .is_some_and(GridItem::has_page_data)
    }

    fn page_turn_loupe_hold(&self, ctx: &egui::Context) -> bool {
        crate::keyboard_input::focused_key_state_permit(ctx).is_some_and(|permit| {
            self.keymap
                .modifier_held_action(ctx, permit, KeyAction::FsLoupeHold)
        })
    }

    /// Called only after ordinary navigation accepted this operation.
    pub(super) fn page_turn_excluded_navigation(
        &mut self,
        ctx: &egui::Context,
        reason: &'static str,
    ) {
        self.page_turn_excluded_navigation_with_operation(ctx, reason, None);
    }

    pub(super) fn page_turn_excluded_navigation_with_operation(
        &mut self,
        ctx: &egui::Context,
        reason: &'static str,
        operation: Option<ViewerOperationId>,
    ) {
        if !self.settings.page_turn_animation_enabled {
            return;
        }
        let destination = self.page_turn_destination();
        let transaction = &mut self.page_turn_presentation.transaction;
        // A resolved handler is the authority that navigation was accepted.
        // Its diagnostic must survive missing/ambiguous ingress, rather than
        // silently returning because an animation candidate could not be made.
        // The child callback hands its resolved identity to ROOT after returning.
        // Their counters are unrelated: an explicit matching destination/identity
        // is the authority, not the receiver's ambient frame number.
        let matched_handoff = operation.is_some_and(|id| {
            transaction
                .facts
                .iter()
                .any(|f| f.operation_id == id && f.destination == destination)
        });
        let frame = (ctx.viewport_id(), ctx.cumulative_frame_nr());
        if !matched_handoff && transaction.observed_frame != Some(frame) {
            *transaction = OperationTransaction {
                observed_frame: Some(frame),
                ..Default::default()
            };
        }
        if !matches!(
            transaction.disposition,
            OperationDisposition::Available | OperationDisposition::Cancelled
        ) {
            return;
        }
        let observed = transaction
            .facts
            .iter()
            .find(|f| {
                f.destination == destination
                    && operation.map_or(transaction.facts.len() == 1, |id| f.operation_id == id)
            })
            .copied();
        let fact = observed.unwrap_or_else(|| ViewerOperationFact {
            destination,
            operation_id: operation.unwrap_or_else(|| {
                ViewerOperationId::UnattributedNavigation(next_operation_batch())
            }),
            source: ctx.viewport_id(),
            phase: OperationPhase::Press,
            cause: match operation {
                Some(ViewerOperationId::Key(_)) => "key",
                Some(ViewerOperationId::Pointer { .. }) => "pointer",
                Some(ViewerOperationId::Pad { .. }) => "pad",
                Some(ViewerOperationId::Slideshow(_)) => "slideshow",
                None | Some(ViewerOperationId::UnattributedNavigation(_)) => {
                    "unidentified_navigation"
                }
            },
            diagnostic_input_seq: self.input_seq,
        });
        let sub_check = if transaction.facts.len() > 1 {
            "multiple_operation_edges"
        } else if observed.is_none() {
            "missing_operation_fact"
        } else {
            "accepted_navigation"
        };
        if observed.is_none() {
            // Retain the accepted diagnostic identity for replay dedup. The
            // Excluded disposition prevents it from becoming an input offer.
            transaction.facts.push(fact);
        }
        self.page_turn_presentation.transaction.disposition = OperationDisposition::Excluded;
        emit(&fact, "skip", reason, &[("sub_check", sub_check.into())]);
    }

    fn page_turn_content(&self, idx: usize) -> PageTurnContent {
        let mut edit = self.current_edit_result_key(idx);
        // PDF/RAW/AI quality publication may replace the source raster. Edits,
        // item identity, effective adjustments and comic content stay binding.
        edit.source_gen = 0;
        PageTurnContent {
            edit,
            params: self.effective_params(idx).clone(),
            comic_generation: self.comic_generation,
            gap: self.settings.spread_page_gap_px,
        }
    }

    /// Resident-only counterpart of ordinary processed + fallback selection.
    /// Incomplete final output is displayable; this never invokes a producer.
    fn page_turn_resident_source(&self, idx: usize) -> Option<(egui::TextureHandle, &'static str)> {
        let edit_key = self.current_edit_result_key(idx);
        let final_entry = self
            .final_composite_cache
            .iter()
            .find(|(key, entry)| {
                key.edit_key == edit_key
                    && **key
                        == self.final_composite_key_for_pixels(
                            edit_key,
                            entry.source_size,
                            self.effective_params(idx),
                        )
            })
            .map(|(_, entry)| entry);
        let selected = if self.raw_development_blocked(idx) {
            self.resolve_original_preview_tex(idx)
                .map(|t| (t, "preview"))
        } else if let Some(entry) = final_entry {
            let comic = self.comic_cache.get(&idx).filter(|comic| {
                comic.comic_gen == self.comic_generation
                    && comic.preview_scale == 1
                    && comic.dims == entry.pixels.size
                    && Arc::ptr_eq(&comic.base, &entry.pixels)
            });
            if let Some(comic) = comic {
                Some((comic.texture.clone(), "comic"))
            } else {
                Some((
                    entry.texture.clone(),
                    if entry.complete {
                        "final"
                    } else {
                        "provisional"
                    },
                ))
            }
        } else if self.colorize_display_requires_final_effect(idx) {
            // Ordinary waiting display also uses a faithful catalog rendition.
            // Never create the rendition at acceptance.
            self.cached_passthrough_rendition(idx)
                .map(|t| (t, "faithful_preview"))
        } else {
            self.resolve_original_preview_tex(idx)
                .map(|t| {
                    let quality = if matches!(
                        self.fs_cache.get(&idx),
                        Some(FsCacheEntry::RawPreview { .. })
                    ) {
                        "preview"
                    } else {
                        "normal"
                    };
                    (t, quality)
                })
                .or_else(|| {
                    self.fs_thumbnail_texture_for_display(idx)
                        .map(|t| (t, "thumbnail"))
                })
        }?;
        self.display_texture_matches_page(&selected.0, idx)
            .then_some(selected)
    }

    #[cfg(test)]
    fn page_turn_normal_ready_texture(&self, idx: usize) -> Option<egui::TextureHandle> {
        self.page_turn_resident_source(idx)
            .map(|(texture, _)| texture)
    }

    /// The immutable accepted source owns the turn until its canonical flat is
    /// adopted. Ordinary producers still run; their upgrades become visible next.
    pub(super) fn page_turn_held_resource(
        &self,
        idx: usize,
    ) -> Option<crate::gpu_lanczos::FullscreenPaintResource> {
        if self.settings.page_turn_animation_enabled && !self.page_turn_view_eligible(idx) {
            return None;
        }
        if !self.settings.page_turn_animation_enabled
            && !matches!(
                self.page_turn_presentation.state,
                PresentationState::Completing { .. }
            )
        {
            return None;
        }
        match &self.page_turn_presentation.state {
            PresentationState::Offered { offer, .. }
                if offer.fact.destination == self.page_turn_destination() =>
            {
                offer
                    .ready_sources
                    .0
                    .iter()
                    .find(|source| {
                        source.occurrence.idx == idx
                            && source.content == self.page_turn_content(idx)
                    })
                    .map(|source| source.resource.clone())
            }
            PresentationState::Turning { incoming, .. }
            | PresentationState::Completing { incoming, .. }
                if incoming.destination == self.page_turn_destination()
                    && incoming.items == self.items_generation
                    && incoming.mode == self.spread_mode
                    && incoming.slice == self.fullscreen_page_slice
                    && Arc::ptr_eq(&incoming.layout, &self.spread_display_units_cache.units) =>
            {
                incoming
                    .pages
                    .iter()
                    .zip(&incoming.content)
                    .find(|(page, content)| {
                        page.occurrence.idx == idx && **content == self.page_turn_content(idx)
                    })
                    .map(|(page, _)| page.resource.clone())
            }
            _ => None,
        }
    }

    pub(super) fn page_turn_bind_prepared(&self, page: PreparedPagedPage) -> PreparedPagedPage {
        if self.settings.page_turn_animation_enabled && !page.live {
            return page;
        }
        let incoming = match &self.page_turn_presentation.state {
            PresentationState::Turning { incoming, .. }
            | PresentationState::Completing { incoming, .. } => incoming,
            _ => return page,
        };
        if self.page_turn_held_resource(page.occurrence.idx).is_none() {
            return page;
        }
        let Some(old) = incoming
            .pages
            .iter()
            .find(|old| old.occurrence == page.occurrence)
        else {
            return page;
        };
        let a = &old.transform;
        let b = &page.transform;
        // Only raster quality/layout metadata may change during the frozen turn.
        // A real viewport/view change remains ordinary cancellation/rebinding.
        if a.viewport_rect != b.viewport_rect
            || a.placement != b.placement
            || a.pixels_per_point != b.pixels_per_point
            || a.rotation != b.rotation
            || a.free_rotation_rad != b.free_rotation_rad
            || a.content_bbox != b.content_bbox
            || a.fit_mode != b.fit_mode
            || a.fit_scale_limits != b.fit_scale_limits
            || old.painter.clip_rect() != page.painter.clip_rect()
            || !same_underlay(&old.underlay, &page.underlay)
            || !same_fit_policy(a.fit_input, b.fit_input)
        {
            return page;
        }
        PreparedPagedPage {
            painter: page.painter,
            ..old.clone()
        }
    }

    /// Called only at accepted Sequential navigation, before opening/mutating the target.
    pub(super) fn offer_page_turn(&mut self, ctx: &egui::Context, target: usize, forward: bool) {
        self.offer_page_turn_with_operation(ctx, target, forward, None);
    }

    pub(super) fn offer_page_turn_with_operation(
        &mut self,
        ctx: &egui::Context,
        target: usize,
        forward: bool,
        operation: Option<ViewerOperationId>,
    ) {
        if !self.settings.page_turn_animation_enabled {
            return;
        }
        if operation.is_some_and(|id| {
            !self
                .page_turn_presentation
                .transaction
                .facts
                .iter()
                .any(|f| f.operation_id == id)
        }) {
            self.page_turn_excluded_navigation_with_operation(
                ctx,
                "missing_operation_fact",
                operation,
            );
            return;
        }
        let Some(fact) = self.page_turn_presentation.take_candidate() else {
            let reason = self
                .page_turn_presentation
                .transaction
                .facts
                .first()
                .map_or("missing_operation_fact", |fact| match fact.phase {
                    OperationPhase::Press => "superseded_animation",
                    OperationPhase::Repeat => "repeat_operation",
                    OperationPhase::Continuous => "continuous_operation",
                });
            self.page_turn_excluded_navigation_with_operation(ctx, reason, operation);
            return;
        };
        if self
            .items
            .get(target)
            .is_some_and(|item| matches!(item, GridItem::Video { .. } | GridItem::Audio { .. }))
        {
            emit(&fact, "skip", "stage1_media_target", &[]);
            return;
        }
        if self.spread_mode.is_split()
            || self.fullscreen_page_slice != crate::page_split::PageSlice::Full
        {
            emit(&fact, "skip", "stage1_split_step", &[]);
            return;
        }
        if !self.page_turn_view_eligible(target) || self.page_turn_loupe_hold(ctx) {
            emit(&fact, "skip", "ineligible_view", &[]);
            return;
        }
        let duration = Duration::from_millis(
            self.settings
                .page_turn_animation_duration_ms
                .clamp(100, 600) as u64,
        );
        if matches!(fact.operation_id, ViewerOperationId::Slideshow(_))
            && !slideshow_interval_allows_turn(duration, self.slideshow_interval_duration())
        {
            emit(
                &fact,
                "skip",
                "slideshow_interval_shorter_than_duration",
                &[],
            );
            return;
        }
        let composition = self.spread_display_composition_for_anchor(target);
        let occurrences = composition.pages_in_screen_order(self.spread_mode);
        // Composition permission belongs to the ordinary presentation owner.
        // A turn cannot select or pin a cached Hidden/Checking companion, or
        // reinterpret a partially permitted unit as the full incoming paper.
        let demand =
            composition.presentation_demand(self.spread_mode, crate::page_split::PageSlice::Full);
        if !matches!(
            self.book_presentation_allowed(&demand),
            crate::book_composition::PresentationDecision::Allowed(allowed)
                if allowed.len() == demand.len() && allowed.iter().all(|allowed| *allowed)
        ) {
            emit(&fact, "skip", "book_presentation_not_fully_allowed", &[]);
            return;
        }
        if let Some(reason) = stage_one_shape_reason(
            self.spread_mode,
            occurrences.iter().copied(),
            composition.singleton_placement.has_white_companion(),
        ) {
            emit(&fact, "skip", reason, &[]);
            return;
        }
        let mut ready_sources = Vec::with_capacity(occurrences.len());
        for occurrence in occurrences {
            if !self.page_turn_view_eligible(occurrence.idx) {
                emit(&fact, "skip", "ineligible_companion", &[]);
                return;
            }
            let Some((texture, quality)) = self.page_turn_resident_source(occurrence.idx) else {
                emit(
                    &fact,
                    "skip",
                    "no_displayable_source_at_accept",
                    &[
                        ("idx", occurrence.idx.into()),
                        (
                            "sub_check",
                            if self.colorize_display_requires_final_effect(occurrence.idx) {
                                "faithful_effect_not_resident"
                            } else {
                                "no_resident_texture"
                            }
                            .into(),
                        ),
                    ],
                );
                return;
            };
            let resource = self.fullscreen_paint_resource_for_texture(occurrence.idx, texture);
            ready_sources.push(AcceptedSource {
                occurrence,
                resource,
                quality,
                content: self.page_turn_content(occurrence.idx),
            });
        }
        let ready_sources = Arc::new(AcceptedSources(ready_sources));
        emit(
            &fact,
            "opportunity",
            "ready_at_accept",
            &[("target", target.into())],
        );
        let old = std::mem::replace(
            &mut self.page_turn_presentation.state,
            PresentationState::Inactive,
        );
        let PresentationState::Flat(outgoing) = old else {
            self.page_turn_presentation.state = old;
            emit(&fact, "skip", "no_outgoing_paint", &[]);
            return;
        };
        // Geometry may legitimately have different gutters (Pair <-> landscape).
        // Its saved content must still belong to the current settings at acceptance,
        // before any pin reservation; a settings change can precede the next paint.
        if outgoing.content.len() != outgoing.pages.len()
            || outgoing
                .content
                .iter()
                .zip(&outgoing.pages)
                .any(|(saved, page)| *saved != self.page_turn_content(page.occurrence.idx))
        {
            emit(
                &fact,
                "skip",
                "outgoing_content_changed",
                &[(
                    "sub_check",
                    if outgoing
                        .content
                        .iter()
                        .any(|saved| saved.gap != self.settings.spread_page_gap_px)
                    {
                        "gap_setting_changed"
                    } else {
                        "page_content_changed"
                    }
                    .into(),
                )],
            );
            self.page_turn_presentation.state = PresentationState::Flat(outgoing);
            return;
        }
        if let Some(reason) = stage_one_shape_reason(
            outgoing.mode,
            outgoing.pages.iter().map(|p| p.occurrence),
            outgoing.white.is_some(),
        ) {
            emit(&fact, "skip", reason, &[]);
            self.page_turn_presentation.state = PresentationState::Flat(outgoing);
            return;
        }
        let bytes = outgoing.retained_bytes();
        let budget = self.vram_pool_budget();
        let resident_bytes = self.vram_accounting_snapshot().total_bytes();
        let pin_bytes = {
            let mut accountant = crate::vram_budget::VramAccountant::default();
            self.page_turn_leases.account_reserved(&mut accountant);
            outgoing.account(&mut accountant);
            ready_sources.account(&mut accountant);
            accountant.finish().total_bytes()
        };
        let limit = page_turn_pin_limit(budget);
        if budget.pool_bytes.is_some_and(|cap| resident_bytes > cap) {
            emit(
                &fact,
                "skip",
                "memory_pressure",
                &[
                    ("resident_bytes", resident_bytes.into()),
                    ("pin_bytes", pin_bytes.into()),
                ],
            );
            self.page_turn_presentation.state = PresentationState::Flat(outgoing);
            return;
        }
        if pin_bytes > limit {
            emit(
                &fact,
                "skip",
                "pin_budget",
                &[
                    ("retain_bytes", bytes.into()),
                    ("resident_bytes", resident_bytes.into()),
                    ("pin_bytes", pin_bytes.into()),
                    ("pin_limit_bytes", limit.into()),
                    (
                        "fullscreen_budget_bytes",
                        serde_json::json!(budget.fullscreen_bytes),
                    ),
                    ("pool_budget_bytes", serde_json::json!(budget.pool_bytes)),
                    (
                        "pin_limit_policy",
                        "fullscreen_tenth_or_unlimited_1gib".into(),
                    ),
                ],
            );
            self.page_turn_presentation.state = PresentationState::Flat(outgoing);
            return;
        }
        outgoing.reserved_bytes.store(bytes, Ordering::Relaxed);
        self.page_turn_leases
            .sources
            .retain(|sources| sources.strong_count() != 0);
        self.page_turn_leases
            .sources
            .push(Arc::downgrade(&ready_sources));
        emit(
            &fact,
            "offered",
            "ready_at_accept",
            &[
                ("retain_bytes", bytes.into()),
                ("resident_bytes", resident_bytes.into()),
                ("pin_bytes", pin_bytes.into()),
                ("pin_limit_bytes", limit.into()),
                ("fullscreen_budget_bytes", serde_json::json!(budget.fullscreen_bytes)),
                ("pool_budget_bytes", serde_json::json!(budget.pool_bytes)),
                ("pin_limit_policy", "fullscreen_tenth_or_unlimited_1gib".into()),
                ("accepted_quality", serde_json::json!(ready_sources.0.iter().map(|s| serde_json::json!({"idx":s.occurrence.idx,"quality":s.quality,"size":s.resource.size()})).collect::<Vec<_>>())),
            ],
        );
        self.page_turn_presentation.state = PresentationState::Offered {
            outgoing,
            offer: AcceptedOffer {
                fact,
                ready_sources,
                resident_bytes,
                pin_bytes,
                accepted_at: Instant::now(),
                duration,
                forward,
                submission: Arc::new(SubmissionTracker::new()),
            },
        };
        let _ = ctx;
    }

    pub(super) fn submit_paged_display_unit(
        &mut self,
        ctx: &egui::Context,
        pages: &[PreparedPagedPage],
        white: Option<(egui::Rect, egui::Rect)>,
    ) -> Option<SubmittedUnitReceipt> {
        if pages.is_empty() {
            self.page_turn_presentation.deactivate("no_page");
            return None;
        }
        assert!(
            pages.len() <= 2,
            "normal paged units have at most two real occurrences"
        );
        self.refresh_page_turn_completion_pages(pages, white);
        // OFF does no effect clock, snapshot/inventory work, geometry projection or input diagnostics.
        if !self.settings.page_turn_animation_enabled {
            for page in pages {
                paint_flat_page(page);
            }
            if let Some((rect, clip)) = white {
                paint_white_companion(&pages[0].painter, rect, clip);
            }
            let completing = self.page_turn_completion_active();
            let receipt = flat_receipt(self.page_turn_destination(), ctx, pages, !completing);
            let event = self.page_turn_completion_event(&receipt);
            self.record_page_turn_completion_flat(&receipt);
            if completing {
                queue_submission(ctx, receipt.clone(), event);
            }
            return Some(receipt);
        }
        // `live` comes from the normal unit/source selector. Do not re-resolve,
        // prepare or mutate holdover readiness while deciding presentation.
        let eligible = pages
            .iter()
            .all(|p| p.live && self.page_turn_view_eligible(p.occurrence.idx))
            && !self.page_turn_loupe_hold(ctx);
        if !eligible {
            self.page_turn_presentation.cancel("ineligible_view");
            for page in pages {
                paint_flat_page(page);
            }
            if let Some((rect, clip)) = white {
                paint_white_companion(&pages[0].painter, rect, clip);
            }
            let receipt = flat_receipt(self.page_turn_destination(), ctx, pages, false);
            let event = self.page_turn_completion_event(&receipt);
            self.record_page_turn_completion_flat(&receipt);
            queue_submission(ctx, receipt.clone(), event);
            return Some(receipt);
        }
        let mut receipt = flat_receipt(self.page_turn_destination(), ctx, pages, false);
        let unit = Arc::new(PaintedUnit {
            destination: self.page_turn_destination(),
            items: self.items_generation,
            layout: Arc::clone(&self.spread_display_units_cache.units),
            mode: self.spread_mode,
            reading_rtl: self
                .spread_mode
                .canonical_reading_direction()
                .unwrap_or(self.reading_direction)
                == ReadingDirection::Rtl,
            slice: self.fullscreen_page_slice,
            pages: pages.to_vec(),
            white,
            reserved_bytes: Arc::new(AtomicU64::new(0)),
            evidence: receipt.accepted.clone(),
            content: pages
                .iter()
                .map(|p| self.page_turn_content(p.occurrence.idx))
                .collect(),
        });
        let old = std::mem::replace(
            &mut self.page_turn_presentation.state,
            PresentationState::Inactive,
        );
        let now = Instant::now();
        let selected = match old {
            PresentationState::Offered { outgoing, offer } => {
                let matching = outgoing.evidence.was_accepted()
                    && offer.fact.destination == unit.destination
                    && outgoing.items == unit.items
                    && outgoing.mode == unit.mode
                    && outgoing.reading_rtl == unit.reading_rtl
                    && outgoing.slice == unit.slice
                    && Arc::ptr_eq(&outgoing.layout, &unit.layout)
                    && stage_one_shape_reason(
                        outgoing.mode,
                        outgoing.pages.iter().map(|page| page.occurrence),
                        outgoing.white.is_some(),
                    )
                    .is_none()
                    && stage_one_shape_reason(
                        unit.mode,
                        unit.pages.iter().map(|page| page.occurrence),
                        unit.white.is_some(),
                    )
                    .is_none()
                    && offer.ready_sources.0.len() == unit.pages.len()
                    && offer
                        .ready_sources
                        .0
                        .iter()
                        .zip(&unit.pages)
                        .all(|(source, p)| {
                            source.occurrence == p.occurrence
                                && source.resource.source_texture_id()
                                    == p.resource.source_texture_id()
                                && source.resource.source_generation()
                                    == p.resource.source_generation()
                                && source.content == self.page_turn_content(p.occurrence.idx)
                        });
                if matching && now.saturating_duration_since(offer.accepted_at) < offer.duration {
                    match prepare_leaf(&outgoing, &unit, offer.forward) {
                        Ok(projection) => {
                            // Include the ordinary prepared native ROI as well as
                            // the accepted source, globally deduplicated.
                            let pin_bytes = self.page_turn_leases.reservation_bytes_with(&unit);
                            let budget = self.vram_pool_budget();
                            let limit = page_turn_pin_limit(budget);
                            if pin_bytes > limit {
                                offer.submission.end();
                                emit(
                                    &offer.fact,
                                    "skip",
                                    "pin_budget",
                                    &[
                                        ("sub_check", "prepared_incoming_roi".into()),
                                        ("pin_bytes", pin_bytes.into()),
                                        ("pin_limit_bytes", limit.into()),
                                        (
                                            "fullscreen_budget_bytes",
                                            serde_json::json!(budget.fullscreen_bytes),
                                        ),
                                        ("pool_budget_bytes", serde_json::json!(budget.pool_bytes)),
                                        (
                                            "pin_limit_policy",
                                            "fullscreen_tenth_or_unlimited_1gib".into(),
                                        ),
                                    ],
                                );
                                PresentationState::Flat(unit.clone())
                            } else {
                                unit.reserved_bytes
                                    .store(unit.retained_bytes(), Ordering::Relaxed);
                                self.page_turn_leases.register(&unit);
                                PresentationState::Turning {
                                    outgoing,
                                    incoming: unit.clone(),
                                    offer,
                                    projection,
                                }
                            }
                        }
                        Err(reason) => {
                            offer.submission.end();
                            emit_projection_failure(&offer, &outgoing, &unit, "skip", reason);
                            PresentationState::Flat(unit.clone())
                        }
                    }
                } else {
                    offer.submission.end();
                    emit(&offer.fact, "skip", "offer_mismatch_or_expired", &[]);
                    PresentationState::Flat(unit.clone())
                }
            }
            PresentationState::Turning {
                outgoing,
                incoming,
                offer,
                projection,
            } => {
                if incoming.content == unit.content && incoming.same_material_and_geometry(&unit) {
                    PresentationState::Turning {
                        outgoing,
                        incoming,
                        offer,
                        projection,
                    }
                } else {
                    let previous = offer.submission.end();
                    if matches!(
                        previous,
                        SubmissionStatus::Offered | SubmissionStatus::Started
                    ) {
                        emit(
                            &offer.fact,
                            if previous == SubmissionStatus::Started {
                                "cancel"
                            } else {
                                "skip"
                            },
                            "source_geometry_or_binding_changed",
                            &[],
                        );
                    }
                    PresentationState::Flat(unit.clone())
                }
            }
            PresentationState::Flat(previous) => {
                PresentationState::Flat(PaintedUnit::select_flat(previous, &unit))
            }
            PresentationState::Completing {
                incoming,
                completion,
                continuation,
                diagnostics,
            } => PresentationState::Completing {
                incoming,
                completion,
                continuation,
                diagnostics,
            },
            _ => PresentationState::Flat(unit.clone()),
        };
        let mut final_state = selected;
        let mut curl = false;
        let mut submission_event = None;
        if let PresentationState::Turning {
            outgoing,
            incoming,
            offer,
            projection: prepared,
        } = &final_state
        {
            let progress = now
                .saturating_duration_since(offer.accepted_at)
                .as_secs_f32()
                / offer.duration.as_secs_f32();
            if progress < 1.0 {
                let t0 = crate::perf::is_enabled().then(Instant::now);
                let projection = crate::page_turn_animation::project_prepared(
                    &prepared.outgoing,
                    &prepared.incoming,
                    prepared.direction,
                    prepared.recipe,
                    progress,
                    crate::perf::is_enabled(),
                )
                .map_err(|failure| failure.sub_check());
                match projection {
                    Ok(projection) => {
                        let mesh_stats = crate::perf::is_enabled().then(|| {
                            let vertices: usize = projection
                                .meshes
                                .iter()
                                .map(|draw| draw.mesh.vertices.len())
                                .sum();
                            let indices: usize = projection
                                .meshes
                                .iter()
                                .map(|draw| draw.mesh.indices.len())
                                .sum();
                            serde_json::json!({
                                "vertices": vertices,
                                "triangles": indices / 3,
                                "draws": projection.meshes.len(),
                                "vertex_bytes": vertices * std::mem::size_of::<egui::epaint::Vertex>(),
                                "index_bytes": indices * std::mem::size_of::<u32>()
                            })
                        });
                        for draw in projection.meshes {
                            let painter = unit.pages[0].painter.with_clip_rect(draw.clip_rect);
                            #[cfg(any(test, all(windows, feature = "test-script")))]
                            let page =
                                incoming
                                    .pages
                                    .iter()
                                    .chain(outgoing.pages.iter())
                                    .find(|page| {
                                        page.resource.paint_texture_id() == draw.mesh.texture_id
                                    });
                            #[cfg(test)]
                            if let Some(page) = page {
                                crate::app::paint_record_test_support::record_selected_paint_resource(&painter,&page.resource);
                            }
                            painter.add(egui::Shape::mesh(draw.mesh));
                            #[cfg(all(windows, feature = "test-script"))]
                            if let Some(page) = page {
                                crate::test_script::record_drawn_paint(
                                    &painter,
                                    &page.resource,
                                    format!("{:?}", page.transform.placement),
                                );
                            }
                        }
                        crate::gpu_lanczos::retain_owner_for_fullscreen_paint_batch(
                            &unit.pages[0].painter,
                            Arc::clone(outgoing),
                        );
                        crate::gpu_lanczos::retain_owner_for_fullscreen_paint_batch(
                            &unit.pages[0].painter,
                            Arc::clone(incoming),
                        );
                        self.page_turn_leases.register(outgoing);
                        self.page_turn_leases.register(incoming);
                        receipt.curl = true;
                        // A landscape's two projection faces share one actual
                        // source occurrence. Vacant never enters the receipt.
                        let actual_visibility = actual_page_visibility(
                            &projection.visibility,
                            receipt.pages.as_slice().len(),
                        );
                        for (i, (_, _, _, area)) in
                            receipt.pages.as_mut_slice().iter_mut().enumerate()
                        {
                            *area = actual_visibility
                                .iter()
                                .find(|v| v.occurrence == i as u64)
                                .map_or(0.0, |v| v.visible_area);
                        }
                        submission_event = Some(SubmissionEvent {
                            tracker: offer.submission.clone(),
                            fact: offer.fact,
                            kind: SubmissionKind::Curl,
                            elapsed_ms: now
                                .saturating_duration_since(offer.accepted_at)
                                .as_secs_f64()
                                * 1000.0,
                            retained_bytes: outgoing.reserved_bytes.load(Ordering::Relaxed),
                            mesh_ms: t0.map_or(0.0, |t| t.elapsed().as_secs_f64() * 1000.0),
                            silhouette_visibility_ms: projection.cost.silhouette_visibility_ms,
                            tessellation_ms: projection.cost.tessellation_ms,
                            outer_mask_ms: projection.cost.outer_mask_cpu_ms,
                            visible_pages: actual_visibility.iter().fold(0, |mask, v| if v.fraction() >= 0.05 && v.short_edge >= 8.0 { mask | (1 << v.occurrence) } else { mask }),
                            proof: crate::perf::is_enabled()
                                .then(||serde_json::json!({"recipe":if unit.mode.is_spread() { "BookLeaf" } else if offer.forward { "SingleDeparture" } else { "SingleArrival" },"angle_degrees":projection.angle_degrees,"hinge_x":projection.hinge_x,"face":if projection.showing_back {"back"} else {"front"},"recipe_prepare_cpu_ms":prepared.prepare_cpu_ms,"mesh":mesh_stats,"static_clip_loss_fraction":projection.cost.static_clip_loss_fraction,"static_support_loss_fraction":projection.cost.static_support_loss_fraction,"visible":actual_visibility.iter().map(|v|serde_json::json!({"occurrence":v.occurrence,"area":v.visible_area,"fraction":v.fraction(),"short_edge":v.short_edge})).collect::<Vec<_>>(),"projection_faces":projection.visibility.incoming.iter().map(|v|serde_json::json!({"face":v.occurrence,"area":v.visible_area,"fraction":v.fraction(),"short_edge":v.short_edge})).collect::<Vec<_>>(),"incoming":unit_diagnostics(incoming,offer),"outgoing":unit_diagnostics(outgoing,offer)})),
                        });
                        curl = true;
                        ctx.request_repaint();
                    }
                    Err(reason) => {
                        let previous = offer.submission.end();
                        if matches!(
                            previous,
                            SubmissionStatus::Offered | SubmissionStatus::Started
                        ) {
                            emit_projection_failure(
                                offer,
                                outgoing,
                                incoming,
                                if previous == SubmissionStatus::Started {
                                    "cancel"
                                } else {
                                    "skip"
                                },
                                reason,
                            );
                        }
                        final_state = PresentationState::Flat(unit.clone());
                    }
                }
            } else {
                // The completion owner retains the incoming pin until its flat
                // receipt is adopted, including when another context offers.
                unit.reserved_bytes
                    .store(unit.retained_bytes(), Ordering::Relaxed);
                submission_event = Some(SubmissionEvent {
                    tracker: offer.submission.clone(),
                    fact: offer.fact,
                    kind: SubmissionKind::Canonical,
                    elapsed_ms: now
                        .saturating_duration_since(offer.accepted_at)
                        .as_secs_f64()
                        * 1000.0,
                    retained_bytes: 0,
                    mesh_ms: 0.0,
                    silhouette_visibility_ms: 0.0,
                    tessellation_ms: 0.0,
                    outer_mask_ms: 0.0,
                    visible_pages: (1 << unit.pages.len()) - 1,
                    proof: crate::perf::is_enabled().then(|| unit_diagnostics(incoming, offer)),
                });
                final_state = PresentationState::Completing {
                    incoming: unit.clone(),
                    completion: FlatCompletion::NeedFlat,
                    continuation: None,
                    diagnostics: CompletionDiagnostics::from_offer(offer),
                };
                // The next owning frame adopts this flat and releases the
                // presentation freeze, even if the upgrade already arrived.
                ctx.request_repaint();
            }
        }
        if !curl {
            for page in &unit.pages {
                paint_flat_page(page);
            }
            if let Some((rect, clip)) = white {
                paint_white_companion(&unit.pages[0].painter, rect, clip);
            }
        }
        if !curl {
            let retained = match &final_state {
                PresentationState::Flat(unit) => unit.clone(),
                _ => unit.clone(),
            };
            self.page_turn_leases.register(&retained);
            crate::gpu_lanczos::retain_owner_for_fullscreen_paint_batch(
                &unit.pages[0].painter,
                retained,
            );
        }
        self.page_turn_presentation.state = final_state;
        if !curl {
            if submission_event.is_none() {
                submission_event = self.page_turn_completion_event(&receipt);
            }
            self.record_page_turn_completion_flat(&receipt);
        }
        queue_submission(ctx, receipt.clone(), submission_event);
        Some(receipt)
    }

    fn page_turn_completion_event(
        &self,
        receipt: &SubmittedUnitReceipt,
    ) -> Option<SubmissionEvent> {
        let PresentationState::Completing {
            incoming,
            diagnostics,
            ..
        } = &self.page_turn_presentation.state
        else {
            return None;
        };
        if receipt.curl
            || receipt.destination != incoming.destination
            || receipt.pages().len() != incoming.pages.len()
            || !incoming.pages.iter().all(|page| {
                receipt.pages().iter().any(|(o, source, paint, area)| {
                    *o == page.occurrence
                        && *source == page.resource.source_texture_id()
                        && *paint == page.resource.paint_texture_id()
                        && *area > 0.0
                })
            })
        {
            return None;
        }
        Some(SubmissionEvent {
            tracker: diagnostics.tracker.clone(),
            fact: diagnostics.fact,
            kind: SubmissionKind::Canonical,
            elapsed_ms: if crate::perf::is_enabled() {
                diagnostics.accepted_at.elapsed().as_secs_f64() * 1000.0
            } else {
                0.0
            },
            retained_bytes: 0,
            mesh_ms: 0.0,
            silhouette_visibility_ms: 0.0,
            tessellation_ms: 0.0,
            outer_mask_ms: 0.0,
            visible_pages: (1 << incoming.pages.len()) - 1,
            proof: None,
        })
    }

    /// The receipt binds the actual normal output, not a stale native ROI from
    /// the canceled pose. A resize/fit change can replace that output while the
    /// logical target and immutable source are unchanged.
    fn refresh_page_turn_completion_pages(
        &mut self,
        pages: &[PreparedPagedPage],
        white: Option<(egui::Rect, egui::Rect)>,
    ) {
        let PresentationState::Completing {
            incoming,
            completion,
            ..
        } = &self.page_turn_presentation.state
        else {
            return;
        };
        if matches!(completion, FlatCompletion::Submitted(receipt) if receipt.was_accepted()) {
            return;
        }
        if incoming.destination != self.page_turn_destination()
            || incoming.items != self.items_generation
            || incoming.mode != self.spread_mode
            || incoming.slice != self.fullscreen_page_slice
            || !Arc::ptr_eq(&incoming.layout, &self.spread_display_units_cache.units)
            || incoming.pages.len() != pages.len()
            || !incoming.pages.iter().zip(pages).all(|(old, new)| {
                old.occurrence == new.occurrence
                    && old.resource.source_texture_id() == new.resource.source_texture_id()
                    && old.resource.source_generation() == new.resource.source_generation()
            })
        {
            self.page_turn_presentation
                .deactivate("completion_source_or_configuration_changed");
            return;
        }
        let fresh = Arc::new(PaintedUnit {
            pages: pages.to_vec(),
            white,
            ..(**incoming).clone()
        });
        if incoming.same_material_and_geometry(&fresh) {
            return;
        }
        // OFF also retains the source through canonical adoption. Geometry
        // rebinding replaces the Arc, so keep its weak global inventory binding.
        self.page_turn_leases.register(&fresh);
        if let PresentationState::Completing {
            incoming,
            completion,
            ..
        } = &mut self.page_turn_presentation.state
        {
            *incoming = fresh;
            *completion = FlatCompletion::NeedFlat;
        }
    }
}

fn paint_white_companion(painter: &egui::Painter, rect: egui::Rect, clip: egui::Rect) {
    let painter = painter.with_clip_rect(clip);
    #[cfg(test)]
    crate::app::paint_record_test_support::record_white_companion_paint(&painter, rect);
    painter.rect_filled(rect, 0.0, egui::Color32::WHITE);
}

pub(super) fn paint_flat_page(page: &PreparedPagedPage) {
    paint_image_underlay(&page.painter, page.transform.paint_quad(), &page.underlay);
    #[cfg(test)]
    crate::app::paint_record_test_support::record_selected_paint_resource(
        &page.painter,
        &page.resource,
    );
    if let Some(roi) = page.resource.visible_source_uv_rect() {
        page.transform.paint_texture_source_region(
            &page.painter,
            page.resource.paint_texture_id(),
            roi,
            egui::Color32::WHITE,
        );
    } else {
        page.transform.paint_texture(
            &page.painter,
            page.resource.paint_texture_id(),
            egui::Color32::WHITE,
        );
    }
    #[cfg(all(windows, feature = "test-script"))]
    crate::test_script::record_drawn_paint(
        &page.painter,
        &page.resource,
        format!("{:?}", page.transform.placement),
    );
    page.resource.retain_native_output_for_paint(&page.painter);
}

fn paint_projection_meshes(
    painter: &egui::Painter,
    meshes: Vec<crate::page_turn_animation::ProjectedMesh>,
) {
    for draw in meshes {
        painter
            .with_clip_rect(draw.clip_rect)
            .add(egui::Shape::mesh(draw.mesh));
    }
}

#[cfg(test)]
pub(super) fn paint_projection_for_test(
    painter: &egui::Painter,
    outgoing: &crate::page_turn_animation::CanonicalUnit,
    incoming: &crate::page_turn_animation::CanonicalUnit,
    direction: crate::page_turn_animation::TurnDirection,
    progress: f32,
) -> Result<
    crate::page_turn_animation::VisibilityReceipt,
    crate::page_turn_animation::ProjectionFailure,
> {
    let projection =
        crate::page_turn_animation::project_turn(outgoing, incoming, direction, progress)?;
    paint_projection_meshes(painter, projection.meshes);
    Ok(projection.visibility)
}

fn flat_receipt(
    destination: PresentationDestination,
    ctx: &egui::Context,
    pages: &[PreparedPagedPage],
    accepted: bool,
) -> SubmittedUnitReceipt {
    SubmittedUnitReceipt {
        destination,
        frame: ctx.cumulative_frame_nr(),
        pass: ctx.cumulative_pass_nr(),
        accepted: if accepted {
            AcceptanceEvidence::Immediate
        } else {
            AcceptanceEvidence::Deferred(Arc::new(AtomicBool::new(false)))
        },
        curl: false,
        pages: {
            let entry = |p: &PreparedPagedPage| {
                (
                    p.occurrence,
                    p.resource.source_texture_id(),
                    p.resource.paint_texture_id(),
                    visible_rect_area(p.transform.paint_rect.intersect(p.painter.clip_rect())),
                )
            };
            match pages {
                [page] => ReceiptPages::Single(entry(page)),
                [left, right] => ReceiptPages::Spread([entry(left), entry(right)]),
                _ => unreachable!("normal paged units have one or two actual occurrences"),
            }
        },
    }
}

fn visible_rect_area(rect: egui::Rect) -> f32 {
    if rect.width() > 0.0 && rect.height() > 0.0 && rect.is_finite() {
        rect.area()
    } else {
        0.0
    }
}

fn slideshow_interval_allows_turn(duration: Duration, interval: Duration) -> bool {
    duration <= interval
}

/// Ordinary full-page windows, real pairs and normalized spread singletons.
/// This boundary is about composition, never a guess from fitted aspect ratios.
fn stage_one_shape_reason(
    mode: SpreadMode,
    occurrences: impl IntoIterator<Item = SpreadPageOccurrence>,
    white: bool,
) -> Option<&'static str> {
    let mut real_pages = 0;
    let mut navigation = None;
    let mut cover = None;
    let mut navigation_count = 0;
    let mut cover_count = 0;
    for page in occurrences {
        real_pages += 1;
        match page.role {
            SpreadPageRole::Navigation => {
                navigation_count += 1;
                navigation = Some(page);
            }
            SpreadPageRole::FrontCoverSupplement => {
                cover_count += 1;
                cover = Some(page);
            }
        }
    }
    // A supplemented terminal unit keeps its single navigation anchor, but its
    // two real presentation occurrences are an ordinary pair. Prove the roles
    // and anchor here for both incoming composition and retained outgoing paint.
    let cover_pair = mode.has_cover()
        && real_pages == 2
        && navigation_count == 1
        && cover_count == 1
        && navigation.zip(cover).is_some_and(|(navigation, cover)| {
            navigation.idx == navigation.navigation_anchor_idx
                && cover.navigation_anchor_idx == navigation.navigation_anchor_idx
                && cover.idx != navigation.idx
        });
    if mode.is_split() {
        Some("stage1_split_step")
    } else if (white
        && !(mode.is_spread() && real_pages == 1 && navigation_count == 1 && cover_count == 0))
        || (cover_count != 0 && !cover_pair)
    {
        Some("stage1_cover_or_single_companion")
    } else if !(1..=2).contains(&real_pages) {
        Some("invalid_actual_page_count")
    } else if !mode.is_spread() && mode != SpreadMode::Single {
        Some("stage1_unsupported_flow")
    } else {
        None
    }
}
fn rect_diagnostics(rect: egui::Rect) -> [f32; 4] {
    [rect.min.x, rect.min.y, rect.max.x, rect.max.y]
}

fn unit_diagnostics(unit: &PaintedUnit, offer: &AcceptedOffer) -> serde_json::Value {
    serde_json::json!({
        "items_generation":unit.items,
        "resident_bytes_at_accept":offer.resident_bytes,
        "global_pin_bytes_at_accept":offer.pin_bytes,
        "expected_sources":offer.ready_sources.0.iter().map(|s|serde_json::json!({"idx":s.occurrence.idx,"source_texture_id":format!("{:?}",s.resource.source_texture_id()),"quality":s.quality,"source_generation":s.resource.source_generation().map(|g|serde_json::json!({"items":g.items,"input":g.input}))})).collect::<Vec<_>>(),
        "reading_rtl":unit.reading_rtl,
        "forward":offer.forward,
        "duration_ms":offer.duration.as_millis() as u64,
        "mode":format!("{:?}",unit.mode),
        "slice":format!("{:?}",unit.slice),
        "projection_shape":if unit.mode.is_spread() && unit.pages.len()==1 { if displayed_landscape(&unit.pages[0].transform) { "landscape_halves" } else { "real_and_vacant" } } else { "ordinary" },
        "pages":unit.pages.iter().map(|page|{
            let resource=&page.resource;
            let uv=canonical_source_uv(&page.transform);
            serde_json::json!({
                "idx":page.occurrence.idx,"role":format!("{:?}",page.occurrence.role),"navigation_anchor_idx":page.occurrence.navigation_anchor_idx,
                "source_texture_id":format!("{:?}",resource.source_texture_id()),"native_texture_id":format!("{:?}",resource.paint_texture_id()),
                "source_size":resource.size(),"native_size":resource.lanczos_output().map_or(resource.size(),|o|o.size()),
                "source_generation":resource.source_generation().map(|g|serde_json::json!({"items":g.items,"input":g.input})),
                "visible_source_roi":resource.visible_source_uv_rect().map(rect_diagnostics),
                "canonical_rect":rect_diagnostics(page.transform.paint_rect),"clip_rect":rect_diagnostics(page.painter.clip_rect()),
                "effective_projection_clip":rect_diagnostics(page.painter.clip_rect().intersect(page.transform.viewport_rect)),
                "normal_layout_spine":page.spine.map(|(h, g)|serde_json::json!({"hinge_x":h,"gutter_width":g})),
                "viewport_rect":rect_diagnostics(page.transform.viewport_rect),"pixels_per_point":page.transform.pixels_per_point,
                "uv":uv.map(|p|[p.x,p.y]),"quality":"normal_selected_processed_resource"
            })
        }).collect::<Vec<_>>()
    })
}

fn emit_projection_failure(
    offer: &AcceptedOffer,
    outgoing: &PaintedUnit,
    incoming: &PaintedUnit,
    kind: &'static str,
    reason: &'static str,
) {
    if !crate::perf::is_enabled() {
        // Keep the light event observable by handler tests without constructing
        // geometry diagnostics or enabling the process-wide perf logger.
        emit(
            &offer.fact,
            kind,
            "geometry_visibility_or_roi",
            &[("sub_check", reason.into())],
        );
        return;
    }
    let loss = canonical_unit(outgoing)
        .ok()
        .zip(canonical_unit(incoming).ok())
        .map(|(old, new)| {
            crate::page_turn_animation::static_surface_loss(
                &old,
                &new,
                crate::page_turn_animation::TurnDirection {
                    reading_rtl: incoming.reading_rtl,
                    forward: offer.forward,
                },
            )
        });
    emit(
        &offer.fact,
        kind,
        "geometry_visibility_or_roi",
        &[
            ("sub_check", reason.into()),
            (
                "geometry",
                serde_json::json!({"outgoing":unit_diagnostics(outgoing, offer),"incoming":unit_diagnostics(incoming, offer),"max_static_clip_loss_fraction":loss.map(|l|l.static_clip_loss_fraction),"max_static_support_loss_fraction":loss.map(|l|l.static_support_loss_fraction),"static_clip_loss_surface":loss.map(|l|l.static_clip_loss_surface),"static_clip_loss_hinge":loss.map(|l|l.static_clip_loss_hinge),"static_support_loss_surface":loss.map(|l|l.static_support_loss_surface),"static_support_loss_hinge":loss.map(|l|l.static_support_loss_hinge)}),
            ),
        ],
    );
}

fn queue_submission(
    ctx: &egui::Context,
    receipt: SubmittedUnitReceipt,
    event: Option<SubmissionEvent>,
) {
    ctx.with_plugin(|plugin: &mut SubmissionPlugin| {
        plugin
            .pending
            .insert(ctx.viewport_id(), PendingSubmission { receipt, event });
    });
}

fn canonical_source_uv(transform: &DisplayedImageTransform) -> [egui::Pos2; 4] {
    [(0.0, 0.0), (1.0, 0.0), (1.0, 1.0), (0.0, 1.0)].map(|(u, v)| {
        let (s, t) = crate::displayed_image_transform::inverse_uv(transform.rotation, u, v);
        egui::pos2(
            transform.uv_rect.left() + s * transform.uv_rect.width(),
            transform.uv_rect.top() + t * transform.uv_rect.height(),
        )
    })
}

fn canonical_unit(
    unit: &PaintedUnit,
) -> Result<crate::page_turn_animation::CanonicalUnit, &'static str> {
    use crate::page_turn_animation::*;
    let mut pages = Vec::new();
    for (i, page) in unit.pages.iter().enumerate() {
        if page.spine != unit.pages[0].spine {
            return Err("mixed_normal_layout_spine");
        }
        if page.transform.pixels_per_point != unit.pages[0].transform.pixels_per_point {
            return Err("mixed_destination_pixel_scale");
        }
        if page.transform.free_rotation_rad.abs() > TRANSFORM_EPSILON {
            return Err("free_rotation");
        }
        let quad = page.transform.paint_quad();
        let uv = canonical_source_uv(&page.transform);
        let material = if let Some(roi) = page.resource.visible_source_uv_rect() {
            PageMaterial::Texture {
                texture_id: page.resource.paint_texture_id(),
                uv: canonical_roi_uv(uv, roi, page.resource.size())?,
                available_uv: egui::Rect::from_min_max(egui::Pos2::ZERO, egui::pos2(1.0, 1.0)),
            }
        } else {
            PageMaterial::Texture {
                texture_id: page.resource.paint_texture_id(),
                uv,
                available_uv: egui::Rect::from_min_max(egui::Pos2::ZERO, egui::pos2(1.0, 1.0)),
            }
        };
        let underlay = match &page.underlay {
            FsBgStyle::Solid(color) => PageMaterial::Blank { color: *color },
            FsBgStyle::Checker(tex) => PageMaterial::Texture {
                texture_id: tex.id(),
                uv: checker_quad_uvs(quad.local_size),
                available_uv: egui::Rect::from_min_max(
                    egui::Pos2::ZERO,
                    egui::pos2(
                        quad.local_size.x / CHECKER_TILE_PX,
                        quad.local_size.y / CHECKER_TILE_PX,
                    ),
                ),
            },
        };
        pages.push(CanonicalPage {
            occurrence: Some(i as u64),
            rect: page.transform.paint_rect,
            // Painter scissor depends on whether this page has trim. The actual
            // image destination remains the transform viewport for both pages.
            // Intersect them before comparing/projecting destinations, rather
            // than mistaking optional trim clipping for a viewport change.
            clip_rect: page
                .painter
                .clip_rect()
                .intersect(page.transform.viewport_rect),
            material,
            underlay: Some(underlay),
        });
    }
    if let Some((rect, clip_rect)) = unit.white {
        pages.push(CanonicalPage {
            occurrence: None,
            rect,
            // Match the ordinary white paint's effective scissor. The stored
            // companion clip alone need not include the UI painter's clip.
            clip_rect: clip_rect
                .intersect(unit.pages[0].painter.clip_rect())
                .intersect(unit.pages[0].transform.viewport_rect),
            material: PageMaterial::Blank {
                color: egui::Color32::WHITE,
            },
            underlay: None,
        });
        pages.sort_by(|a, b| a.rect.left().total_cmp(&b.rect.left()));
    }
    let canonical = CanonicalUnit {
        spine_x: unit.pages[0].spine.map(|(spine, _)| spine),
        gutter_width: unit.pages[0].spine.map_or(0.0, |(_, gap)| gap),
        pixels_per_point: unit.pages[0].transform.pixels_per_point,
        layout: if pages.len() == 2 {
            UnitLayout::Spread
        } else {
            UnitLayout::Single
        },
        pages,
    };
    if unit.mode.is_spread() && unit.pages.len() == 1 && unit.white.is_none() {
        let page = &unit.pages[0];
        let window = if displayed_landscape(&page.transform) {
            SpreadWindow::Landscape
        } else {
            let (side, gap) = match page.transform.placement {
                ResolvedDisplayPlacement::SingletonSpread { side, gap, .. } => (side, gap),
                ResolvedDisplayPlacement::Z {
                    singleton_side,
                    singleton_gap,
                    ..
                } => (singleton_side, singleton_gap),
                ResolvedDisplayPlacement::Normal { .. } => (SingletonSpreadPlacement::Center, 0.0),
            };
            if unit.mode.has_cover()
                && unit
                    .layout
                    .first()
                    .is_some_and(|first| first.pages.first() == Some(&page.occurrence.idx))
                && unit.layout[0].formation
                    != SpreadDisplayUnitFormation::Singleton(
                        SpreadDisplaySingletonCause::ExplicitAlone,
                    )
            {
                let first = &unit.layout[0];
                let cover_side = if unit.mode.is_rtl() {
                    SingletonSpreadPlacement::Left
                } else {
                    SingletonSpreadPlacement::Right
                };
                // The common builder owns formation and the normal resolver owns
                // placement. Explicit Alone has its own first-endpoint policy;
                // only an automatically formed cover needs this side proof.
                // A raw cover index can also be a supplement or a shifted unit.
                // Only the builder's real first unpaired Navigation leaf on its
                // resolved cover side establishes a physical front cover.
                if first.pages != [page.occurrence.idx]
                    || !matches!(
                        first.formation,
                        SpreadDisplayUnitFormation::Singleton(
                            SpreadDisplaySingletonCause::UnpairedSlot
                        )
                    )
                    || page.occurrence.role != SpreadPageRole::Navigation
                    || page.occurrence.navigation_anchor_idx != page.occurrence.idx
                    || side != cover_side
                {
                    return Err("cover_side_not_proven");
                }
            }
            SpreadWindow::Side {
                right: match side.side() {
                    SingletonSpreadPlacement::Right => true,
                    SingletonSpreadPlacement::Left => false,
                    _ => unit.reading_rtl,
                },
                gap: if side == SingletonSpreadPlacement::Center {
                    0.0
                } else {
                    gap
                },
            }
        };
        normalize_spread_window(canonical, window).map_err(|e| e.sub_check())
    } else {
        Ok(canonical)
    }
}

fn canonical_roi_uv(
    uv: [egui::Pos2; 4],
    roi: egui::Rect,
    source_size: [usize; 2],
) -> Result<[egui::Pos2; 4], &'static str> {
    // Visible ROI travels through screen -> source UV and physical-texel
    // rounding before the native output retains its f32 edges. A nominally
    // full edge can therefore be one ULP inside the canonical UV (the measured
    // right edge was 0.99999994). Admit only TRANSFORM_EPSILON of one SOURCE
    // texel, independently per axis; this is not a normalized-UV/whole-pixel
    // allowance and never grows with the native output or screen dimensions.
    let tolerance = egui::vec2(
        TRANSFORM_EPSILON / source_size[0].max(1) as f32,
        TRANSFORM_EPSILON / source_size[1].max(1) as f32,
    );
    if !roi.is_finite()
        || roi.width() <= 0.0
        || roi.height() <= 0.0
        || uv.iter().any(|p| {
            !p.is_finite()
                || roi.min.x - p.x > tolerance.x
                || p.x - roi.max.x > tolerance.x
                || roi.min.y - p.y > tolerance.y
                || p.y - roi.max.y > tolerance.y
        })
    {
        return Err("partial_roi");
    }
    // Clamp to the admitted actual resource before renormalizing. Expanding
    // coverage alone would let UV1 become >1 on the native ROI texture.
    Ok(uv.map(|p| {
        let p = roi.clamp(p);
        egui::pos2(
            ((p.x - roi.min.x) / roi.width()).clamp(0.0, 1.0),
            ((p.y - roi.min.y) / roi.height()).clamp(0.0, 1.0),
        )
    }))
}

fn displayed_landscape(transform: &DisplayedImageTransform) -> bool {
    let size = transform.source_size;
    match transform.rotation {
        crate::rotation_db::Rotation::Cw90 | crate::rotation_db::Rotation::Cw270 => size.y > size.x,
        _ => size.x > size.y,
    }
}

fn actual_page_visibility(
    receipt: &crate::page_turn_animation::VisibilityReceipt,
    actual_count: usize,
) -> Vec<crate::page_turn_animation::PageVisibility> {
    let mut result: Vec<crate::page_turn_animation::PageVisibility> =
        Vec::with_capacity(actual_count);
    for face in &receipt.incoming {
        let actual = if actual_count == 1 {
            0
        } else {
            face.occurrence
        };
        if let Some(page) = result.iter_mut().find(|p| p.occurrence == actual) {
            page.visible_area += face.visible_area;
            page.canonical_visible_area += face.canonical_visible_area;
            page.short_edge = page.short_edge.max(face.short_edge);
        } else {
            result.push(crate::page_turn_animation::PageVisibility {
                occurrence: actual,
                ..face.clone()
            });
        }
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    fn destination(serial: u64) -> PresentationDestination {
        PresentationDestination {
            context: crate::app::ViewerContextId::for_test(serial),
            window: None,
            viewport: egui::ViewportId::ROOT,
        }
    }
    fn fact(batch: u64) -> ViewerOperationFact {
        ViewerOperationFact {
            destination: destination(1),
            operation_id: ViewerOperationId::Pad { batch, ordinal: 0 },
            source: egui::ViewportId::ROOT,
            phase: OperationPhase::Press,
            cause: "test",
            diagnostic_input_seq: 1,
        }
    }
    fn receipt(ctx: &egui::Context, idx: usize) -> SubmittedUnitReceipt {
        SubmittedUnitReceipt {
            destination: destination(1),
            frame: ctx.cumulative_frame_nr(),
            pass: ctx.cumulative_pass_nr(),
            pages: ReceiptPages::Single((
                SpreadPageOccurrence::navigation(idx, idx),
                egui::TextureId::Managed(idx as u64),
                egui::TextureId::User(idx as u64),
                100.0,
            )),
            accepted: AcceptanceEvidence::Deferred(Arc::new(AtomicBool::new(false))),
            curl: false,
        }
    }
    fn event(tracker: &Arc<SubmissionTracker>, kind: SubmissionKind) -> SubmissionEvent {
        SubmissionEvent {
            tracker: tracker.clone(),
            fact: fact(1),
            kind,
            elapsed_ms: 0.0,
            retained_bytes: 0,
            mesh_ms: 0.0,
            silhouette_visibility_ms: 0.0,
            tessellation_ms: 0.0,
            outer_mask_ms: 0.0,
            visible_pages: 1,
            proof: None,
        }
    }
    fn unit(ctx: &egui::Context, texture: egui::TextureHandle, serial: u64) -> Arc<PaintedUnit> {
        use crate::displayed_image_transform::*;
        let rect = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(32.0, 32.0));
        let transform = DisplayedImageTransform::from_resolved_rect(
            DisplayedImageTransformInput {
                page_idx: 0,
                viewport_rect: rect,
                source_size: rect.size(),
                texture_size: rect.size(),
                rotation: crate::rotation_db::Rotation::None,
                free_rotation_rad: 0.0,
                content_bbox: None,
                fit_mode: FullscreenFitInput::legacy(FullscreenFitMode::Page),
                fit_scale_limits: FullscreenFitScaleLimits::default(),
                pixels_per_point: 1.0,
                placement: ResolvedDisplayPlacement::Normal { zoom_pan: None },
                pixel_fit: RectPixelFit::Texels,
            },
            rect,
        )
        .unwrap();
        Arc::new(PaintedUnit {
            destination: destination(serial),
            items: 1,
            layout: Arc::new(Vec::new()),
            mode: SpreadMode::Single,
            reading_rtl: false,
            slice: crate::page_split::PageSlice::Full,
            pages: vec![PreparedPagedPage {
                occurrence: SpreadPageOccurrence::navigation(0, 0),
                transform,
                resource: FullscreenPaintResource::resampleable(
                    0,
                    texture,
                    crate::gpu_lanczos::FullscreenPaintSourceGeneration { items: 1, input: 0 },
                ),
                painter: ctx.debug_painter().with_clip_rect(rect),
                underlay: FsBgStyle::Solid(egui::Color32::BLACK),
                live: true,
                spine: None,
            }],
            white: None,
            reserved_bytes: Arc::new(AtomicU64::new(0)),
            evidence: AcceptanceEvidence::Immediate,
            content: Vec::new(),
        })
    }

    #[test]
    fn page_turn_white_canonical_uses_normal_trim_scissor_without_a_resource_or_receipt() {
        use crate::displayed_image_transform::*;
        let ctx = egui::Context::default();
        let texture = ctx.load_texture(
            "white-trim-resource",
            egui::ColorImage::filled([32, 32], egui::Color32::WHITE),
            egui::TextureOptions::LINEAR,
        );
        let mut painted = unit(&ctx, texture, 1);
        let bytes = painted.retained_bytes();
        let viewport = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(80.0, 32.0));
        let trim_clip = egui::Rect::from_min_max(egui::pos2(0.0, 2.0), egui::pos2(80.0, 30.0));
        let white_rect = egui::Rect::from_min_size(egui::pos2(40.0, 0.0), egui::vec2(32.0, 32.0));
        let actual = Arc::get_mut(&mut painted).unwrap();
        actual.mode = SpreadMode::Ltr;
        let real_rect = actual.pages[0].transform.paint_rect;
        actual.pages[0].transform = DisplayedImageTransform::from_resolved_rect(
            DisplayedImageTransformInput {
                page_idx: 0,
                viewport_rect: viewport,
                source_size: real_rect.size(),
                texture_size: real_rect.size(),
                rotation: crate::rotation_db::Rotation::None,
                free_rotation_rad: 0.0,
                content_bbox: None,
                fit_mode: FullscreenFitInput::legacy(FullscreenFitMode::Page),
                fit_scale_limits: FullscreenFitScaleLimits::default(),
                pixels_per_point: 1.0,
                placement: ResolvedDisplayPlacement::SingletonSpread {
                    side: SingletonSpreadPlacement::LeftWhite,
                    gap: 8.0,
                    zoom_pan: None,
                },
                pixel_fit: RectPixelFit::Texels,
            },
            real_rect,
        )
        .unwrap();
        actual.pages[0].painter = ctx.debug_painter().with_clip_rect(trim_clip);
        actual.pages[0].spine = Some((36.0, 8.0));
        actual.white = Some((white_rect, viewport.expand(10.0)));
        let canonical = canonical_unit(&painted).unwrap();
        assert_eq!(painted.retained_bytes(), bytes, "white holds no resource");
        assert_eq!(canonical.pages.len(), 2);
        assert_eq!(canonical.pages[0].rect, real_rect);
        assert_eq!(canonical.spine_x, Some(36.0));
        assert!(
            canonical
                .pages
                .iter()
                .all(|page| page.clip_rect == trim_clip)
        );
        assert_eq!(canonical.pages[1].rect, white_rect);
        assert!(canonical.pages[1].occurrence.is_none());
        let proof = flat_receipt(destination(1), &ctx, &painted.pages, false);
        assert_eq!(proof.pages().len(), 1, "white has no presentation receipt");
        for forward in [false, true] {
            let direction = crate::page_turn_animation::TurnDirection {
                reading_rtl: false,
                forward,
            };
            let recipe =
                crate::page_turn_animation::prepare_turn(&canonical, &canonical, direction)
                    .unwrap();
            let projected = crate::page_turn_animation::project_prepared(
                &canonical, &canonical, direction, recipe, 0.5, false,
            )
            .unwrap();
            assert_eq!(projected.visibility.incoming.len(), 1);
            assert!(
                projected
                    .meshes
                    .iter()
                    .all(|draw| draw.clip_rect == trim_clip)
            );
        }
    }

    #[test]
    fn page_turn_destination_clip_is_the_canvas_not_the_optional_trim_scissor() {
        let ctx = egui::Context::default();
        let texture = ctx.load_texture(
            "clip-policy",
            egui::ColorImage::filled([32, 32], egui::Color32::WHITE),
            egui::TextureOptions::LINEAR,
        );
        let mut outgoing = unit(&ctx, texture.clone(), 1);
        let mut incoming = unit(&ctx, texture, 1);
        let viewport = outgoing.pages[0].transform.viewport_rect;
        Arc::get_mut(&mut outgoing).unwrap().pages[0].painter =
            ctx.debug_painter().with_clip_rect(viewport.expand(20.0));
        Arc::get_mut(&mut incoming).unwrap().pages[0].painter =
            ctx.debug_painter().with_clip_rect(viewport);
        let old = canonical_unit(&outgoing).unwrap();
        let new = canonical_unit(&incoming).unwrap();
        assert_eq!(old.pages[0].clip_rect, viewport);
        assert_eq!(old.pages[0].clip_rect, new.pages[0].clip_rect);
        let direction = crate::page_turn_animation::TurnDirection {
            reading_rtl: false,
            forward: true,
        };
        assert!(crate::page_turn_animation::prepare_turn(&old, &new, direction).is_ok());
        // An actually narrower destination is still not the same canvas.
        Arc::get_mut(&mut incoming).unwrap().pages[0].painter = ctx.debug_painter().with_clip_rect(
            egui::Rect::from_min_max(viewport.min, viewport.max - egui::vec2(1.0, 0.0)),
        );
        let different = canonical_unit(&incoming).unwrap();
        assert_eq!(
            crate::page_turn_animation::prepare_turn(&old, &different, direction)
                .unwrap_err()
                .sub_check(),
            "destination_clip_changed"
        );
    }

    #[cfg(windows)]
    #[test]
    fn page_turn_settings_retire_receipts_and_ready_in_mounted_and_parked_owners() {
        let ctx = egui::Context::default();
        let mut app = crate::app::setup_app_for_test();
        let texture = ctx.load_texture(
            "settings-owner",
            egui::ColorImage::filled([4, 4], egui::Color32::WHITE),
            egui::TextureOptions::LINEAR,
        );
        let expected_bytes = crate::vram_budget::texture_size_texels([4, 4], true) as u64 * 4;
        let seed = |owner: &mut App, batch: u64| {
            let destination = owner.page_turn_destination();
            let accepted = Arc::new(AtomicBool::new(true));
            let incoming = Arc::new(PaintedUnit {
                destination,
                evidence: AcceptanceEvidence::Deferred(accepted.clone()),
                reserved_bytes: Arc::new(AtomicU64::new(expected_bytes)),
                ..(*unit(&ctx, texture.clone(), batch)).clone()
            });
            owner.page_turn_leases.register(&incoming);
            let fact = ViewerOperationFact {
                destination,
                source: destination.viewport,
                ..fact(batch)
            };
            owner.fullscreen_idx = Some(0);
            owner.page_turn_presentation.state = PresentationState::Completing {
                incoming: incoming.clone(),
                completion: FlatCompletion::NeedFlat,
                continuation: None,
                diagnostics: CompletionDiagnostics {
                    fact,
                    tracker: Arc::new(SubmissionTracker::new()),
                    accepted_at: Instant::now(),
                },
            };
            owner.page_turn_external_operation(
                &ctx,
                fact.operation_id,
                OperationPhase::Press,
                "test",
            );
            assert!(owner.page_turn_defer_navigation(
                &ctx,
                1,
                None,
                crate::fs_page_load_scheduler::FsPageLoadContract::Sequential,
                Some(fact.operation_id),
            ));
            let mut receipt = flat_receipt(destination, &ctx, &incoming.pages, true);
            receipt.accepted = incoming.evidence.clone();
            if let PresentationState::Completing { completion, .. } =
                &mut owner.page_turn_presentation.state
            {
                *completion = FlatCompletion::Submitted(receipt.clone());
            }
            owner.pending_fs_page_turn_ready = Some(PendingFsPageTurnReady {
                items_generation: owner.items_generation,
                units: owner.spread_display_units_cache.units.clone(),
                mode: owner.spread_mode,
                slice: owner.fullscreen_page_slice,
                composition: SpreadDisplayComposition::from_navigation_pages(0, &[0]).unwrap(),
                layout: owner.fullscreen_page_layout.clone(),
                decision: FsPageTurnDecision::normal(),
                source_policy: FsSubmittedSourcePolicy::Normal,
                sources: FsNavigatorTextureSources {
                    pages: Vec::new(),
                    submission: FsDisplayUnitSubmission::Paged(receipt),
                },
            });
            accepted
        };
        let main_acceptance = seed(&mut app, 871);
        let mut parked_acceptance = None;
        let sibling = app.build_window_context_for_test(91345, |owner| {
            parked_acceptance = Some(seed(owner, 872));
        });
        assert_eq!(app.page_turn_leases.reserved_bytes(), expected_bytes);
        app.cancel_all_page_turn_effects_for_settings();
        assert_eq!(app.page_turn_leases.reserved_bytes(), expected_bytes);
        let mut accountant = crate::vram_budget::VramAccountant::default();
        app.page_turn_leases.account(&mut accountant);
        assert_eq!(accountant.finish().total_bytes(), expected_bytes);
        let check = |owner: &App, batch: u64, old: &AtomicBool| {
            assert!(owner.pending_fs_page_turn_ready.is_none());
            let PresentationState::Completing {
                incoming,
                completion: FlatCompletion::NeedFlat,
                ..
            } = &owner.page_turn_presentation.state
            else {
                panic!("settings must require a new canonical flat");
            };
            // A late old hook cannot adopt the new geometry's evidence.
            old.store(true, Ordering::Relaxed);
            assert!(!incoming.evidence.was_accepted());
            assert_eq!(
                owner
                    .page_turn_presentation
                    .continuation_operation_for_test(),
                Some((ViewerOperationId::Pad { batch, ordinal: 0 }, 1))
            );
        };
        check(&app, 871, &main_acceptance);
        assert_eq!(app.page_turn_leases.reserved_bytes(), expected_bytes);
        app.with_window_context_for_test(sibling, |owner| {
            check(owner, 872, parked_acceptance.as_ref().unwrap());
        });
        check(&app, 871, &main_acceptance);
    }

    #[test]
    fn immutable_operation_replay_ignores_diagnostic_sequence_and_claims_once() {
        let mut presentation = PageTurnPresentation::default();
        presentation.observe_for_frame((egui::ViewportId::ROOT, 1), vec![fact(1)]);
        assert!(presentation.take_candidate().is_some());
        let mut replay = fact(1);
        replay.diagnostic_input_seq = 999;
        replay.cause = "replay_metadata";
        presentation.observe_for_frame((egui::ViewportId::ROOT, 1), vec![replay]);
        assert!(presentation.take_candidate().is_none());
        presentation.observe_for_frame((egui::ViewportId::ROOT, 1), vec![]);
        assert!(presentation.take_candidate().is_none());
    }
    #[test]
    fn batch_merge_suppresses_multiple_edges_and_unclaimed_facts_expire() {
        let mut presentation = PageTurnPresentation::default();
        presentation.observe_for_frame((egui::ViewportId::ROOT, 1), vec![fact(1)]);
        presentation.observe_for_frame((egui::ViewportId::ROOT, 1), vec![fact(2)]);
        assert_eq!(presentation.transaction.facts.len(), 2);
        assert!(presentation.take_candidate().is_none());
        presentation.observe_for_frame((egui::ViewportId::ROOT, 2), vec![]);
        assert!(presentation.transaction.facts.is_empty());
        let mut continuous = fact(3);
        continuous.phase = OperationPhase::Continuous;
        presentation.observe_for_frame((egui::ViewportId::ROOT, 3), vec![continuous]);
        assert!(presentation.take_candidate().is_none());
        presentation.deactivate("disabled");
        assert!(presentation.transaction.facts.is_empty());
    }
    #[test]
    fn touch_end_inherits_start_identity_and_original_destination() {
        let ctx = egui::Context::default();
        install(&ctx);
        let mut app = crate::app::setup_app_for_test();
        app.settings.page_turn_animation_enabled = true;
        let touch = |phase| egui::Event::Touch {
            device_id: egui::TouchDeviceId(1),
            id: egui::TouchId(9),
            phase,
            pos: egui::pos2(30.0, 40.0),
            force: None,
        };
        let _ = ctx.run(
            egui::RawInput {
                events: vec![touch(egui::TouchPhase::Start)],
                ..Default::default()
            },
            |ctx| app.page_turn_pre_draw(ctx),
        );
        let start = app
            .page_turn_presentation
            .transaction
            .facts
            .iter()
            .find(|f| f.cause == "touch")
            .copied()
            .unwrap();
        let _ = ctx.run(
            egui::RawInput {
                events: vec![touch(egui::TouchPhase::End)],
                ..Default::default()
            },
            |ctx| app.page_turn_pre_draw(ctx),
        );
        let end = app
            .page_turn_presentation
            .transaction
            .facts
            .iter()
            .find(|f| f.cause == "touch")
            .copied()
            .unwrap();
        assert!(same_operation(&start, &end));
        assert_eq!(start.destination, end.destination);
        let _ = ctx.run(
            egui::RawInput {
                events: vec![touch(egui::TouchPhase::End)],
                ..Default::default()
            },
            |ctx| app.page_turn_pre_draw(ctx),
        );
        assert!(
            !app.page_turn_presentation
                .transaction
                .facts
                .iter()
                .any(|f| f.cause == "touch")
        );
    }

    #[test]
    fn pointer_delivery_preserves_original_binding_on_release() {
        let id = ViewerOperationId::Pointer {
            batch: 2,
            source: egui::ViewportId::ROOT,
            ordinal: 0,
        };
        let routed = PointerDelivery::Unrouted(id).route(destination(1)).unwrap();
        assert_eq!(routed.id(), Some(id));
        assert!(routed.route(destination(2)).is_none());
        assert_eq!(routed.route(destination(1)).unwrap().id(), Some(id));
    }
    #[test]
    fn output_plugin_accepts_only_last_actual_submission() {
        let ctx = egui::Context::default();
        install(&ctx);
        let tracker = Arc::new(SubmissionTracker::new());
        let mut receipts = Vec::new();
        let _ = ctx.run(egui::RawInput::default(), |ctx| {
            let proof = receipt(ctx, 1);
            queue_submission(
                ctx,
                proof.clone(),
                Some(event(&tracker, SubmissionKind::Curl)),
            );
            receipts.push(proof);
            if ctx.current_pass_index() == 0 {
                ctx.request_discard("test replay");
            }
        });
        assert_eq!(receipts.len(), 2);
        assert!(!receipts[0].was_accepted());
        assert!(receipts[1].was_accepted());
        assert_eq!(tracker.status(), SubmissionStatus::Started);
        let _ = ctx.run(egui::RawInput::default(), |ctx| {
            queue_submission(
                ctx,
                receipt(ctx, 1),
                Some(event(&tracker, SubmissionKind::Canonical)),
            )
        });
        assert_eq!(tracker.status(), SubmissionStatus::Finished);
        let _ = ctx.run(egui::RawInput::default(), |ctx| {
            queue_submission(
                ctx,
                receipt(ctx, 1),
                Some(event(&tracker, SubmissionKind::Curl)),
            )
        });
        assert_eq!(tracker.status(), SubmissionStatus::Finished);
    }
    #[test]
    fn discarded_canonical_completion_requires_same_actual_next_flat_output() {
        for matching in [true, false] {
            let ctx = egui::Context::default();
            install(&ctx);
            let tracker = Arc::new(SubmissionTracker::new());
            tracker.ledger.lock().unwrap().status = SubmissionStatus::Started;
            let _ = ctx.run(egui::RawInput::default(), |ctx| {
                if ctx.current_pass_index() == 0 {
                    queue_submission(
                        ctx,
                        receipt(ctx, 1),
                        Some(event(&tracker, SubmissionKind::Canonical)),
                    );
                    ctx.request_discard("canonical replay");
                } else {
                    queue_submission(ctx, receipt(ctx, if matching { 1 } else { 2 }), None);
                }
            });
            assert_eq!(
                tracker.status(),
                if matching {
                    SubmissionStatus::Finished
                } else {
                    SubmissionStatus::Cancelled
                }
            );
        }
    }
    #[test]
    fn canceled_leaf_credits_unseen_pages_only_after_canonical_output_is_adopted() {
        let ctx = egui::Context::default();
        install(&ctx);
        let tracker = Arc::new(SubmissionTracker::new());
        tracker.ledger.lock().unwrap().status = SubmissionStatus::Started;
        tracker.end();
        let _ = ctx.run(egui::RawInput::default(), |ctx| {
            if ctx.current_pass_index() == 0 {
                queue_submission(
                    ctx,
                    receipt(ctx, 1),
                    Some(event(&tracker, SubmissionKind::Canonical)),
                );
                ctx.request_discard("cancel flat replay");
            } else {
                let ledger = tracker.ledger.lock().unwrap();
                assert!(!ledger.canonical_visible);
                assert_eq!(ledger.visible_pages, 0);
                drop(ledger);
                queue_submission(ctx, receipt(ctx, 1), None);
            }
        });
        let ledger = tracker.ledger.lock().unwrap();
        assert!(ledger.canonical_visible);
        assert_eq!(ledger.visible_pages, 1);
        assert_eq!(ledger.status, SubmissionStatus::Cancelled);
    }

    #[test]
    fn weak_inventory_deduplicates_shared_sources_and_releases_after_last_real_batch() {
        let ctx = egui::Context::default();
        let texture = ctx.load_texture(
            "lease",
            egui::ColorImage::filled([4, 4], egui::Color32::WHITE),
            egui::TextureOptions::LINEAR,
        );
        let old = unit(&ctx, texture.clone(), 1);
        let sibling = unit(&ctx, texture, 2);
        let mut inventory = PageTurnLeaseInventory::default();
        inventory.register(&old);
        inventory.register(&old);
        inventory.register(&sibling);
        assert_eq!(inventory.units.len(), 2);
        let bytes = old.retained_bytes();
        old.reserved_bytes.store(bytes, Ordering::Relaxed);
        sibling.reserved_bytes.store(bytes, Ordering::Relaxed);
        assert_eq!(inventory.reserved_bytes(), bytes);
        assert_eq!(inventory.reservation_bytes_with(&sibling), bytes);
        let mut accountant = crate::vram_budget::VramAccountant::default();
        accountant.add_texture(
            crate::vram_budget::VramSubsystem::FsCache,
            old.pages[0].resource.source_texture(),
            true,
        );
        inventory.account(&mut accountant);
        assert_eq!(accountant.finish().total_bytes(), bytes);
        let weak = Arc::downgrade(&old);
        let output = ctx.run(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(64.0, 64.0),
                )),
                ..Default::default()
            },
            |ctx| {
                let painter = ctx.debug_painter();
                paint_flat_page(&old.pages[0]);
                crate::gpu_lanczos::retain_owner_for_fullscreen_paint_batch(&painter, old.clone());
            },
        );
        drop(old);
        assert!(weak.upgrade().is_some());
        let primitives = ctx.tessellate(output.shapes, output.pixels_per_point);
        assert!(weak.upgrade().is_some());
        drop(primitives);
        assert!(weak.upgrade().is_none());
        assert_eq!(inventory.reserved_bytes(), bytes); // sibling reservation remains
        drop(sibling);
        assert_eq!(inventory.reserved_bytes(), 0);
    }

    #[test]
    fn discarded_flat_unit_is_unadopted_until_its_actual_receipt_is_accepted() {
        let ctx = egui::Context::default();
        install(&ctx);
        let texture = ctx.load_texture(
            "discarded-flat",
            egui::ColorImage::filled([4, 4], egui::Color32::WHITE),
            egui::TextureOptions::LINEAR,
        );
        let mut outgoing = unit(&ctx, texture, 1);
        let proof = receipt(&ctx, 0);
        Arc::make_mut(&mut outgoing).evidence = proof.accepted.clone();
        let _ = ctx.run(egui::RawInput::default(), |ctx| {
            if ctx.current_pass_index() == 0 {
                queue_submission(ctx, proof.clone(), None);
                ctx.request_discard("never shown outgoing");
            }
        });
        assert!(!outgoing.evidence.was_accepted());
        // The runtime Offered -> Turning guard reads this exact shared proof.
        let _ = ctx.run(egui::RawInput::default(), |ctx| {
            queue_submission(ctx, proof.clone(), None)
        });
        assert!(outgoing.evidence.was_accepted());
    }

    #[test]
    fn same_material_flat_replay_replaces_unaccepted_outgoing_proof() {
        let ctx = egui::Context::default();
        install(&ctx);
        let texture = ctx.load_texture(
            "replay-adoption",
            egui::ColorImage::filled([4, 4], egui::Color32::WHITE),
            egui::TextureOptions::LINEAR,
        );
        let mut previous = unit(&ctx, texture, 1);
        let discarded = receipt(&ctx, 0);
        Arc::make_mut(&mut previous).evidence = discarded.accepted.clone();
        let mut next = Arc::new((*previous).clone());
        let adopted = receipt(&ctx, 0);
        Arc::make_mut(&mut next).evidence = adopted.accepted.clone();
        assert!(previous.same_material_and_geometry(&next));
        let selected = PaintedUnit::select_flat(previous, &next);
        assert!(Arc::ptr_eq(&selected, &next));
        let _ = ctx.run(egui::RawInput::default(), |ctx| {
            queue_submission(ctx, adopted.clone(), None)
        });
        assert!(selected.evidence.was_accepted());
        assert!(!discarded.was_accepted());
        assert!(Arc::ptr_eq(
            &PaintedUnit::select_flat(selected.clone(), &next),
            &selected
        ));
    }

    #[test]
    fn offered_cancel_releases_reservation_but_preserves_flat_and_sibling_leases() {
        let ctx = egui::Context::default();
        let texture = ctx.load_texture(
            "cancel",
            egui::ColorImage::filled([4, 4], egui::Color32::WHITE),
            egui::TextureOptions::LINEAR,
        );
        let outgoing = unit(&ctx, texture.clone(), 1);
        let sibling = unit(&ctx, texture, 2);
        let bytes = outgoing.retained_bytes();
        outgoing.reserved_bytes.store(bytes, Ordering::Relaxed);
        sibling.reserved_bytes.store(bytes, Ordering::Relaxed);
        let mut inventory = PageTurnLeaseInventory::default();
        inventory.register(&outgoing);
        inventory.register(&sibling);
        let mut presentation = PageTurnPresentation::default();
        presentation.state = PresentationState::Offered {
            outgoing: outgoing.clone(),
            offer: AcceptedOffer {
                fact: fact(1),
                ready_sources: Arc::new(AcceptedSources(Vec::new())),
                resident_bytes: bytes,
                pin_bytes: bytes,
                accepted_at: Instant::now(),
                duration: Duration::from_millis(200),
                forward: true,
                submission: Arc::new(SubmissionTracker::new()),
            },
        };
        presentation.cancel("new_operation");
        assert!(matches!(presentation.state, PresentationState::Flat(_)));
        assert_eq!(outgoing.reserved_bytes.load(Ordering::Relaxed), 0);
        assert_eq!(inventory.reserved_bytes(), bytes);
        drop(sibling);
        assert_eq!(inventory.reserved_bytes(), 0);
        let mut accountant = crate::vram_budget::VramAccountant::default();
        inventory.account(&mut accountant);
        assert_eq!(accountant.finish().total_bytes(), bytes);
    }

    #[test]
    fn canonical_uv_matches_normal_paint_for_rounded_trimmed_and_rotated_rects() {
        use crate::displayed_image_transform::*;
        let ctx = egui::Context::default();
        for rotation in [
            crate::rotation_db::Rotation::None,
            crate::rotation_db::Rotation::Cw90,
            crate::rotation_db::Rotation::Cw180,
            crate::rotation_db::Rotation::Cw270,
        ] {
            let rect = egui::Rect::from_min_size(
                egui::pos2(27.333334, 11.125),
                egui::vec2(266.66666, 400.0),
            );
            let transform = DisplayedImageTransform::from_resolved_rect(
                DisplayedImageTransformInput {
                    page_idx: 0,
                    viewport_rect: egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(600.0, 600.0),
                    ),
                    source_size: egui::vec2(80.0, 120.0),
                    texture_size: egui::vec2(80.0, 120.0),
                    rotation,
                    free_rotation_rad: 0.0,
                    content_bbox: Some(egui::Rect::from_min_max(
                        egui::pos2(0.1, 0.2),
                        egui::pos2(0.9, 0.8),
                    )),
                    fit_mode: FullscreenFitInput::legacy(FullscreenFitMode::Page),
                    fit_scale_limits: FullscreenFitScaleLimits::default(),
                    pixels_per_point: 1.0,
                    placement: ResolvedDisplayPlacement::Normal { zoom_pan: None },
                    pixel_fit: RectPixelFit::Texels,
                },
                rect,
            )
            .unwrap();
            let output = ctx.run(egui::RawInput::default(), |ctx| {
                transform.paint_texture(
                    &ctx.debug_painter(),
                    egui::TextureId::User(19),
                    egui::Color32::WHITE,
                )
            });
            let mesh = output
                .shapes
                .iter()
                .find_map(|shape| match &shape.shape {
                    egui::Shape::Mesh(mesh) if mesh.texture_id == egui::TextureId::User(19) => {
                        Some(mesh)
                    }
                    _ => None,
                })
                .unwrap();
            assert_eq!(
                canonical_source_uv(&transform),
                mesh.vertices
                    .iter()
                    .map(|v| v.uv)
                    .collect::<Vec<_>>()
                    .as_slice()
            );
            assert!(canonical_source_uv(&transform).iter().all(|uv| {
                egui::Rect::from_min_max(egui::Pos2::ZERO, egui::pos2(1.0, 1.0)).contains(*uv)
            }));
        }
    }
    #[test]
    fn raw_cache_is_displayable_and_offer_never_starts_processing() {
        let ctx = egui::Context::default();
        let mut app = crate::app::setup_app_for_test();
        app.items = vec![GridItem::Image(std::path::PathBuf::from(
            "c:/page-turn/cold.png",
        ))];
        app.items_generation = 41;
        app.fullscreen_idx = Some(0);
        app.reading_flow = crate::settings::ReadingFlow::Paged;
        app.settings.page_turn_animation_enabled = true;
        let image = egui::ColorImage::filled([4, 4], egui::Color32::WHITE);
        let tex = ctx.load_texture("cold", image.clone(), egui::TextureOptions::LINEAR);
        app.fs_cache.insert(
            0,
            FsCacheEntry::Static {
                tex,
                pixels: Arc::new(image),
                source_dims: Some([4, 4]),
                load_seq: 1,
                animation: crate::fs_animation::StaticAnimationState::Still,
            },
        );
        assert!(app.resolve_fs_display_tex(0, false).is_some());
        assert!(app.page_turn_normal_ready_texture(0).is_some());
        let _ = ctx.run(egui::RawInput::default(), |ctx| {
            app.page_turn_external_operation(
                ctx,
                ViewerOperationId::Pad {
                    batch: 73,
                    ordinal: 0,
                },
                OperationPhase::Press,
                "test",
            );
            app.offer_page_turn(ctx, 0, true);
        });
        assert!(matches!(
            app.page_turn_presentation.state,
            PresentationState::Inactive
        ));
        assert!(app.final_composite_cache.is_empty());
        assert!(app.edit_result_cache.is_empty());
        assert!(app.comic_docs.is_empty());
    }
    #[test]
    fn accepted_exclusion_claims_single_repeat_once_and_does_not_attribute_multi_edges() {
        let ctx = egui::Context::default();
        let mut app = crate::app::setup_app_for_test();
        app.settings.page_turn_animation_enabled = true;
        let _ = ctx.run(egui::RawInput::default(), |ctx| {
            app.page_turn_external_operation(
                ctx,
                ViewerOperationId::Pad {
                    batch: 91,
                    ordinal: 0,
                },
                OperationPhase::Repeat,
                "test",
            );
            app.page_turn_excluded_navigation(ctx, "repeat_operation");
            assert!(
                app.page_turn_presentation.transaction.disposition
                    == OperationDisposition::Excluded
            );
            app.page_turn_external_operation(
                ctx,
                ViewerOperationId::Pad {
                    batch: 91,
                    ordinal: 0,
                },
                OperationPhase::Repeat,
                "replay_metadata",
            );
            app.page_turn_excluded_navigation(ctx, "repeat_operation");
            assert!(
                app.page_turn_presentation.transaction.disposition
                    == OperationDisposition::Excluded
            );
        });
        let _ = ctx.run(egui::RawInput::default(), |ctx| {
            app.page_turn_external_operations(
                ctx,
                &[
                    (
                        ViewerOperationId::Pad {
                            batch: 92,
                            ordinal: 0,
                        },
                        OperationPhase::Press,
                        "test",
                    ),
                    (
                        ViewerOperationId::Pad {
                            batch: 92,
                            ordinal: 1,
                        },
                        OperationPhase::Press,
                        "test",
                    ),
                ],
            );
            app.page_turn_excluded_navigation(ctx, "ambiguous");
            assert!(
                app.page_turn_presentation.transaction.disposition
                    == OperationDisposition::Excluded
            );
        });
    }

    #[test]
    fn accepted_new_operation_that_cancels_turn_is_excluded_once_without_readmission() {
        let ctx = egui::Context::default();
        let mut app = crate::app::setup_app_for_test();
        app.settings.page_turn_animation_enabled = true;
        let texture = ctx.load_texture(
            "supersession",
            egui::ColorImage::filled([4, 4], egui::Color32::WHITE),
            egui::TextureOptions::LINEAR,
        );
        let outgoing = unit(&ctx, texture.clone(), 1);
        let incoming = unit(&ctx, texture, 1);
        let tracker = Arc::new(SubmissionTracker::new());
        tracker.ledger.lock().unwrap().status = SubmissionStatus::Started;
        let _ = ctx.run(egui::RawInput::default(), |ctx| {
            app.page_turn_external_operation(
                ctx,
                ViewerOperationId::Pad {
                    batch: 110,
                    ordinal: 0,
                },
                OperationPhase::Press,
                "test",
            );
            let old_fact = app.page_turn_presentation.take_candidate().unwrap();
            app.page_turn_presentation.state = PresentationState::Turning {
                projection: prepare_leaf(&outgoing, &incoming, true).unwrap(),
                outgoing: outgoing.clone(),
                incoming: incoming.clone(),
                offer: AcceptedOffer {
                    fact: old_fact,
                    ready_sources: Arc::new(AcceptedSources(Vec::new())),
                    resident_bytes: 0,
                    pin_bytes: 0,
                    accepted_at: Instant::now(),
                    duration: Duration::from_millis(200),
                    forward: true,
                    submission: tracker.clone(),
                },
            };
        });
        // The active turn originated in a previous frame. Distinct operations
        // in one physical frame are intentionally an ambiguous transaction.
        let _ = ctx.run(egui::RawInput::default(), |ctx| {
            app.page_turn_external_operation(
                ctx,
                ViewerOperationId::Pad {
                    batch: 111,
                    ordinal: 0,
                },
                OperationPhase::Repeat,
                "test",
            );
            assert!(matches!(
                app.page_turn_presentation.state,
                PresentationState::Completing { .. }
            ));
            assert!(
                app.page_turn_presentation.transaction.disposition
                    == OperationDisposition::Cancelled
            );
            assert_eq!(tracker.status(), SubmissionStatus::Cancelled);
            app.page_turn_excluded_navigation(ctx, "repeat_operation");
            assert!(
                app.page_turn_presentation.transaction.disposition
                    == OperationDisposition::Excluded
            );
            assert!(app.page_turn_presentation.take_candidate().is_none());
            app.page_turn_external_operation(
                ctx,
                ViewerOperationId::Pad {
                    batch: 111,
                    ordinal: 0,
                },
                OperationPhase::Repeat,
                "replay_metadata",
            );
            app.page_turn_excluded_navigation(ctx, "repeat_operation");
            assert!(
                app.page_turn_presentation.transaction.disposition
                    == OperationDisposition::Excluded
            );
            assert!(app.page_turn_presentation.take_candidate().is_none());
        });
        assert!(app.fullscreen_idx.is_none()); // Observer never navigates or queues a target.
    }

    #[test]
    fn off_receipt_has_stack_pages_and_immediate_evidence() {
        let ctx = egui::Context::default();
        let texture = ctx.load_texture(
            "off",
            egui::ColorImage::filled([4, 4], egui::Color32::WHITE),
            egui::TextureOptions::LINEAR,
        );
        let unit = unit(&ctx, texture, 1);
        let proof = flat_receipt(destination(1), &ctx, &unit.pages, true);
        assert!(proof.was_accepted());
        assert!(proof.accepted_ledger().is_none());
        assert!(matches!(proof.pages, ReceiptPages::Single(_)));
    }
    #[test]
    fn slideshow_policy_uses_adopted_interval() {
        let duration = Duration::from_millis(200);
        assert!(!slideshow_interval_allows_turn(
            duration,
            Duration::from_millis(199)
        ));
        assert!(slideshow_interval_allows_turn(duration, duration));
        assert!(slideshow_interval_allows_turn(
            duration,
            Duration::from_secs(3)
        ));
    }

    #[test]
    fn stage_one_shape_scope_keeps_pairs_in_cover_books_and_logs_other_shapes() {
        let navigation = |idx| SpreadPageOccurrence {
            idx,
            role: SpreadPageRole::Navigation,
            navigation_anchor_idx: 1,
        };
        let cover = SpreadPageOccurrence {
            idx: 0,
            role: SpreadPageRole::FrontCoverSupplement,
            navigation_anchor_idx: 1,
        };
        for mode in [
            SpreadMode::Ltr,
            SpreadMode::Rtl,
            SpreadMode::LtrCover,
            SpreadMode::RtlCover,
        ] {
            assert_eq!(
                stage_one_shape_reason(mode, [navigation(1), navigation(2)], false),
                None
            );
            assert_eq!(stage_one_shape_reason(mode, [navigation(1)], false), None);
            assert_eq!(
                stage_one_shape_reason(mode, [navigation(1), cover], false),
                (!mode.has_cover()).then_some("stage1_cover_or_single_companion")
            );
            for invalid in [
                vec![cover],
                vec![cover, cover],
                vec![navigation(1), navigation(2), cover],
                vec![navigation(2), cover],
                vec![navigation(1), SpreadPageOccurrence { idx: 1, ..cover }],
                vec![
                    navigation(1),
                    SpreadPageOccurrence {
                        navigation_anchor_idx: 2,
                        ..cover
                    },
                ],
            ] {
                assert_eq!(
                    stage_one_shape_reason(mode, invalid, false),
                    Some("stage1_cover_or_single_companion")
                );
            }
            assert_eq!(
                stage_one_shape_reason(mode, [navigation(1), cover], true),
                Some("stage1_cover_or_single_companion")
            );
        }
        assert_eq!(
            stage_one_shape_reason(SpreadMode::Single, [navigation(1)], false),
            None
        );
        assert_eq!(
            stage_one_shape_reason(SpreadMode::SplitRtl, [navigation(1)], false),
            Some("stage1_split_step")
        );
        assert_eq!(
            stage_one_shape_reason(SpreadMode::Ltr, [navigation(1)], true),
            None
        );
        assert_eq!(
            stage_one_shape_reason(SpreadMode::Single, [navigation(1)], true),
            Some("stage1_cover_or_single_companion")
        );
        assert_eq!(
            stage_one_shape_reason(SpreadMode::Ltr, [navigation(1), navigation(2)], true),
            Some("stage1_cover_or_single_companion")
        );
        assert_eq!(
            stage_one_shape_reason(SpreadMode::SplitRtl, [navigation(1)], true),
            Some("stage1_split_step")
        );
    }

    #[test]
    fn partial_leaf_receipt_cannot_complete_navigation_even_with_every_source_resident() {
        let ctx = egui::Context::default();
        let tex = ctx.load_texture(
            "partial-navigation-proof",
            egui::ColorImage::filled([4, 4], egui::Color32::WHITE),
            egui::TextureOptions::LINEAR,
        );
        let painted = unit(&ctx, tex, 1);
        let page = &painted.pages[0];
        let sources = FsNavigatorTextureSources {
            pages: vec![FsNavigatorTextureSource {
                occurrence: page.occurrence,
                resource: page.resource.clone(),
                provenance: FsDisplayUnitPageProvenance::Live,
            }],
            submission: FsDisplayUnitSubmission::Unsubmitted,
        };
        let mut proof = flat_receipt(destination(1), &ctx, &painted.pages, true);
        assert!(proof.matches_sources(destination(1), &sources));
        proof.curl = true;
        assert!(!proof.matches_sources(destination(1), &sources));
    }
}
