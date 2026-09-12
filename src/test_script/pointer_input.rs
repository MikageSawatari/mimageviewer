//! Test-script-only egui pointer transactions and fullscreen named-region evidence.
//!
//! The synthetic timeline remains the transaction owner. This module joins its one-shot child
//! viewport delivery to the real widget handler, callback tail, and following painted geometry.

use std::cell::RefCell;
use std::marker::PhantomData;
use std::rc::Rc;
use std::sync::{Arc, RwLock};
use std::time::{Duration, Instant};

use rhai::{Dynamic, Engine, EvalAltResult, ImmutableString, Map};

use super::{RunnerBridge, TestScriptWindowIdentity, TestScriptWindowSnapshot};
use crate::key_input::{
    PreparedSyntheticPointerStep, SyntheticPointerDownRequest, SyntheticPointerHandlerEffect,
    SyntheticPointerHandlerProof, SyntheticPointerModeSignature, SyntheticPointerOwner,
    SyntheticPointerRegion, SyntheticPointerShowTail, SyntheticPointerStepKind,
};

pub(crate) const STRIP_REGION: &str = "fullscreen_still_seek_strip_row";
pub(crate) const TRACK_REGION: &str = "fullscreen_seek_track";

#[derive(Clone, Copy, Debug, Hash, PartialEq, Eq)]
pub(crate) enum RegionId {
    StillSeekStripRow,
    StillSeekTrack,
}

impl RegionId {
    fn parse(value: &str) -> Result<Self, String> {
        match value {
            STRIP_REGION => Ok(Self::StillSeekStripRow),
            TRACK_REGION => Ok(Self::StillSeekTrack),
            _ => Err(format!("unsupported egui pointer region: {value}")),
        }
    }

    fn as_str(self) -> &'static str {
        match self {
            Self::StillSeekStripRow => STRIP_REGION,
            Self::StillSeekTrack => TRACK_REGION,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct NamedRegion {
    pub(crate) id: RegionId,
    pub(crate) widget_id: egui::Id,
    pub(crate) rect: egui::Rect,
    /// Stable logical coordinate frame used by the production gesture math. The strip response
    /// rect may move as its visible cells move, while this frame must remain fixed.
    pub(crate) coordinate_frame: egui::Rect,
    pub(crate) geometry_token: u64,
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct PaintedStripLayout {
    pub(crate) center_pos: usize,
    pub(crate) cell_indices: Vec<usize>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct FullscreenModeProof {
    pub(crate) spread_mode: String,
    pub(crate) reading_flow: String,
    pub(crate) strip_rtl: bool,
    pub(crate) seek_bar_rtl: bool,
    pub(crate) strip_visible: bool,
    pub(crate) strip_locked: bool,
    pub(crate) bar_locked: bool,
}

impl FullscreenModeProof {
    fn key_signature(&self) -> SyntheticPointerModeSignature {
        SyntheticPointerModeSignature {
            spread_mode: self.spread_mode.clone(),
            reading_flow: self.reading_flow.clone(),
            strip_rtl: self.strip_rtl,
            seek_bar_rtl: self.seek_bar_rtl,
            strip_visible: self.strip_visible,
            strip_locked: self.strip_locked,
            bar_locked: self.bar_locked,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct RegionFrame {
    pub(crate) owner: TestScriptWindowIdentity,
    pub(crate) items_generation: u64,
    /// Per-frame content observation. This is deliberately not gesture-owner identity.
    pub(crate) page_index: usize,
    pub(crate) item_identity: String,
    pub(crate) mode: FullscreenModeProof,
    pub(crate) viewport_frame: u64,
    pub(crate) raw_input_time_bits: u64,
    pub(crate) pixels_per_point: f32,
    pub(crate) revision: u64,
    pub(crate) regions: Vec<NamedRegion>,
    pub(crate) strip_layout: Option<PaintedStripLayout>,
}

#[derive(Default)]
pub(crate) struct RegionCatalog {
    next_revision: u64,
    next_geometry_token: u64,
    frames: Vec<RegionFrame>,
}

impl RegionCatalog {
    pub(crate) fn retain_authoritative(&mut self, windows: &[TestScriptWindowSnapshot]) {
        self.frames.retain(|frame| {
            windows.iter().any(|window| {
                window.identity.as_ref() == Some(&frame.owner)
                    && window.items_generation == frame.items_generation
            })
        });
    }

    pub(crate) fn publish(&mut self, mut frame: RegionFrame) -> u64 {
        self.next_revision = self.next_revision.wrapping_add(1).max(1);
        frame.revision = self.next_revision;
        for named in &mut frame.regions {
            let previous_token = self.frames.iter().find_map(|previous| {
                (previous.owner == frame.owner
                    && previous.items_generation == frame.items_generation
                    && previous.page_index == frame.page_index
                    && previous.item_identity == frame.item_identity
                    && previous.mode == frame.mode
                    && previous.pixels_per_point.to_bits() == frame.pixels_per_point.to_bits())
                .then(|| {
                    previous.regions.iter().find(|candidate| {
                        candidate.id == named.id
                            && candidate.widget_id == named.widget_id
                            && candidate.rect == named.rect
                            && candidate.coordinate_frame == named.coordinate_frame
                    })
                })
                .flatten()
                .map(|previous| previous.geometry_token)
            });
            named.geometry_token = previous_token.unwrap_or_else(|| {
                self.next_geometry_token = self
                    .next_geometry_token
                    .checked_add(1)
                    .expect("test-script pointer geometry token space exhausted");
                self.next_geometry_token
            });
        }
        self.frames.retain(|old| old.owner != frame.owner);
        self.frames.push(frame);
        self.next_revision
    }

    pub(crate) fn invalidate(&mut self, owner: Option<&TestScriptWindowIdentity>) {
        if let Some(owner) = owner {
            self.frames.retain(|frame| frame.owner != *owner);
        }
    }

    fn current_region<'a>(
        &'a self,
        owner: &TestScriptWindowIdentity,
        current: &TestScriptWindowSnapshot,
        region: RegionId,
        after_revision: u64,
    ) -> Option<(&'a RegionFrame, &'a NamedRegion)> {
        let frame = self.frames.iter().find(|frame| {
            frame.owner == *owner
                && frame.items_generation == current.items_generation
                && current.page_index == Some(frame.page_index)
                && current.item_identity == frame.item_identity
                && frame.revision > after_revision
        })?;
        let named = frame.regions.iter().find(|named| named.id == region)?;
        Some((frame, named))
    }

    /// Return the latest geometry for an already-held gesture. Page and item identity are
    /// deliberately absent: track dragging may navigate during Move, while the immutable gesture
    /// owner is the exact host plus catalog generation, mode, widget, ppp, and coordinate frame.
    fn held_region<'a>(
        &'a self,
        owner: &TestScriptWindowIdentity,
        items_generation: u64,
        region: RegionId,
    ) -> Option<(&'a RegionFrame, &'a NamedRegion)> {
        let frame = self
            .frames
            .iter()
            .find(|frame| frame.owner == *owner && frame.items_generation == items_generation)?;
        let named = frame.regions.iter().find(|named| named.id == region)?;
        Some((frame, named))
    }
}

pub(crate) type SharedRegionCatalog = Arc<RwLock<RegionCatalog>>;

#[cfg(test)]
pub(crate) fn new_shared_catalog() -> SharedRegionCatalog {
    Arc::new(RwLock::new(RegionCatalog::default()))
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ShowOwner {
    pub(crate) identity: TestScriptWindowIdentity,
    pub(crate) items_generation: u64,
}

#[derive(Clone, Debug)]
pub(crate) struct PassGeometry {
    /// Captured inside the actual child callback. A show-pre owner claim is not geometry proof.
    callback_viewport: egui::ViewportId,
    callback_witness: eframe::miv_test_script_window_witness::WindowWitness,
    items_generation: u64,
    page_index: usize,
    item_identity: String,
    mode: FullscreenModeProof,
    viewport_frame: u64,
    raw_input_time_bits: u64,
    pixels_per_point: f32,
    regions: Vec<NamedRegion>,
    strip_layout: Option<PaintedStripLayout>,
}

#[derive(Clone, Debug)]
enum GeometryAccumulator {
    Empty,
    Valid(PassGeometry),
    Contradiction(String),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct CallbackTail {
    /// Captured at the end of the actual child callback pass, after widget handling.
    callback_viewport: egui::ViewportId,
    callback_witness: eframe::miv_test_script_window_witness::WindowWitness,
    primary_down: bool,
}

#[derive(Clone, Debug)]
enum TailAccumulator {
    Empty,
    Valid(CallbackTail),
    Contradiction(String),
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct HandlerReceipt {
    /// The exact immutable transport proof supplied by `key_input` in this lexical show.
    pub(crate) prepared: PreparedSyntheticPointerStep,
    pub(crate) observed_point: egui::Pos2,
    pub(crate) effect_after: HandlerEffect,
    pub(crate) child_witness: eframe::miv_test_script_window_witness::WindowWitness,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum HandlerEffect {
    Pressed,
    StripCenter(usize),
    TrackTarget(Option<usize>),
    ReleasedStripCenter(usize),
    ReleasedTrackTarget(Option<usize>),
}

#[derive(Clone, Debug)]
enum ReceiptAccumulator {
    Empty,
    Valid(HandlerReceipt),
    Contradiction(String),
}

#[derive(Clone, Debug)]
enum DeliveryAccumulator {
    Empty,
    /// Immutable proof recorded by the input hook in the lexical show that received the event.
    Exact(InputDeliveryProof),
    Contradiction(String),
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct InputDeliveryProof {
    pub(crate) prepared: PreparedSyntheticPointerStep,
    pub(crate) callback_witness: eframe::miv_test_script_window_witness::WindowWitness,
    pub(crate) child_viewport: egui::ViewportId,
    /// Time assigned by the actual child RawInput. ROOT preparation has a different backend
    /// clock sample and remains only transport provenance on `prepared`.
    pub(crate) child_raw_time_bits: u64,
}

impl DeliveryAccumulator {
    fn observe(&mut self, delivered: InputDeliveryProof) {
        match self {
            Self::Empty => *self = Self::Exact(delivered),
            Self::Exact(existing) => {
                *self = Self::Contradiction(format!(
                    "pointer step was delivered more than once in one show: first={existing:?} next={delivered:?}"
                ));
            }
            Self::Contradiction(_) => {}
        }
    }
}

fn show_owner_from_pointer(owner: &SyntheticPointerOwner) -> ShowOwner {
    ShowOwner {
        identity: owner.identity.clone(),
        items_generation: owner.items_generation,
    }
}

fn prepared_show_owner(prepared: &PreparedSyntheticPointerStep) -> ShowOwner {
    show_owner_from_pointer(&prepared.step.latch.owner)
}

fn prepared_is_down(prepared: &PreparedSyntheticPointerStep) -> bool {
    matches!(prepared.step.kind, SyntheticPointerStepKind::Down { .. })
}

impl ReceiptAccumulator {
    fn observe(&mut self, receipt: HandlerReceipt) {
        match self {
            Self::Empty => *self = Self::Valid(receipt),
            Self::Valid(existing)
                if existing.prepared == receipt.prepared
                    && existing.observed_point == receipt.observed_point
                    && existing.effect_after == receipt.effect_after
                    && existing.child_witness == receipt.child_witness
                    && !prepared_is_down(&receipt.prepared) =>
            {
                // A move may be observed again in a later pass. It is the same step and effect,
                // not another move to accumulate. A repeated Down is never idempotent.
            }
            Self::Valid(existing) => {
                *self = Self::Contradiction(format!(
                    "conflicting pointer handler receipts: first={existing:?} next={receipt:?}"
                ));
            }
            Self::Contradiction(_) => {}
        }
    }
}

#[derive(Clone, Debug)]
struct ActiveShow {
    /// Expected logical owner fixed before `show_viewport_immediate`. It is routing input, not
    /// proof that any callback or painted geometry belonged to that owner.
    owner: Option<ShowOwner>,
    geometry: GeometryAccumulator,
    tail: TailAccumulator,
    delivery: DeliveryAccumulator,
    receipt: ReceiptAccumulator,
    ownership_error: Option<String>,
}

thread_local! {
    static ACTIVE_SHOW: RefCell<Option<ActiveShow>> = const { RefCell::new(None) };
}

#[must_use]
pub(crate) struct ShowGuard {
    previous: Option<ActiveShow>,
    active: bool,
    _not_send_or_sync: PhantomData<Rc<()>>,
}

impl ShowGuard {
    pub(crate) fn finish(mut self) -> ShowOutput {
        let current = ACTIVE_SHOW.with(|slot| slot.borrow_mut().take());
        ACTIVE_SHOW.with(|slot| *slot.borrow_mut() = self.previous.take());
        self.active = false;
        match current {
            Some(ActiveShow {
                owner,
                geometry,
                tail,
                delivery,
                receipt,
                ownership_error,
            }) => ShowOutput {
                owner,
                geometry,
                tail,
                delivery,
                receipt,
                ownership_error,
            },
            None => ShowOutput {
                owner: None,
                geometry: GeometryAccumulator::Contradiction(
                    "pointer show scope disappeared before finish".to_string(),
                ),
                tail: TailAccumulator::Contradiction(
                    "pointer show scope disappeared before finish".to_string(),
                ),
                delivery: DeliveryAccumulator::Contradiction(
                    "pointer show scope disappeared before finish".to_string(),
                ),
                receipt: ReceiptAccumulator::Contradiction(
                    "pointer show scope disappeared before finish".to_string(),
                ),
                ownership_error: Some("pointer show scope disappeared before finish".to_string()),
            },
        }
    }
}

impl Drop for ShowGuard {
    fn drop(&mut self) {
        if self.active {
            ACTIVE_SHOW.with(|slot| *slot.borrow_mut() = self.previous.take());
        }
    }
}

pub(crate) fn enter_show(owner: Option<ShowOwner>) -> ShowGuard {
    let current = ActiveShow {
        owner,
        geometry: GeometryAccumulator::Empty,
        tail: TailAccumulator::Empty,
        delivery: DeliveryAccumulator::Empty,
        receipt: ReceiptAccumulator::Empty,
        ownership_error: None,
    };
    let previous = ACTIVE_SHOW.with(|slot| slot.borrow_mut().replace(current));
    ShowGuard {
        previous,
        active: true,
        _not_send_or_sync: PhantomData,
    }
}

/// Called at the beginning of every child callback pass. Geometry is final-pass-only; the receipt
/// accumulator is deliberately retained across passes in this lexical show.
pub(crate) fn begin_pass(
    ctx: &egui::Context,
    items_generation: u64,
    page_index: usize,
    item_identity: String,
    mode: FullscreenModeProof,
) {
    let callback_viewport = ctx.viewport_id();
    let callback_witness = eframe::miv_test_script_window_witness::active();
    let next = match callback_witness {
        Some(witness) if witness.viewport_id() == callback_viewport => {
            GeometryAccumulator::Valid(PassGeometry {
                callback_viewport,
                callback_witness: witness,
                items_generation,
                page_index,
                item_identity,
                mode,
                viewport_frame: ctx.cumulative_frame_nr(),
                raw_input_time_bits: ctx.input(|input| input.time.to_bits()),
                pixels_per_point: ctx.pixels_per_point(),
                regions: Vec::new(),
                strip_layout: None,
            })
        }
        Some(witness) => GeometryAccumulator::Contradiction(format!(
            "pointer geometry callback viewport mismatch: ctx={callback_viewport:?} backend={:?}",
            witness.viewport_id()
        )),
        None => GeometryAccumulator::Contradiction(format!(
            "pointer geometry callback {callback_viewport:?} has no active backend witness"
        )),
    };
    ACTIVE_SHOW.with(|slot| {
        let mut slot = slot.borrow_mut();
        let Some(show) = slot.as_mut() else {
            return;
        };
        // Geometry and primary level are one pass-local product. Starting a later valid pass
        // invalidates the earlier tail until this pass calls `finish_pass`. Handler proof alone is
        // run-local and deliberately remains across passes.
        if !matches!(&show.geometry, GeometryAccumulator::Contradiction(_))
            && !matches!(&show.tail, TailAccumulator::Contradiction(_))
        {
            match next {
                GeometryAccumulator::Valid(geometry) => {
                    show.geometry = GeometryAccumulator::Valid(geometry);
                    show.tail = TailAccumulator::Empty;
                }
                GeometryAccumulator::Contradiction(reason) => {
                    show.geometry = GeometryAccumulator::Contradiction(reason.clone());
                    show.tail = TailAccumulator::Contradiction(reason);
                }
                GeometryAccumulator::Empty => unreachable!("begin_pass always observes a pass"),
            }
        }
    });
}

pub(crate) fn record_region(
    id: RegionId,
    response: &egui::Response,
    coordinate_frame: egui::Rect,
    strip_layout: Option<PaintedStripLayout>,
) {
    ACTIVE_SHOW.with(|slot| {
        let mut slot = slot.borrow_mut();
        let Some(GeometryAccumulator::Valid(geometry)) =
            slot.as_mut().map(|show| &mut show.geometry)
        else {
            return;
        };
        geometry.regions.retain(|named| named.id != id);
        geometry.regions.push(NamedRegion {
            id,
            widget_id: response.id,
            rect: response.rect,
            coordinate_frame,
            geometry_token: 0,
        });
        if strip_layout.is_some() {
            geometry.strip_layout = strip_layout;
        }
    });
}

/// Called at the lexical end of every child callback pass, after the real handlers. The final
/// pass replaces primary-level evidence. The witness must still be the one captured by
/// `begin_pass`; a nested/embedded scope mismatch makes the whole show unpublishable.
pub(crate) fn finish_pass(ctx: &egui::Context) {
    let callback_viewport = ctx.viewport_id();
    let callback_witness = eframe::miv_test_script_window_witness::active();
    let primary_down = ctx.input(|input| input.pointer.primary_down());
    ACTIVE_SHOW.with(|slot| {
        let mut slot = slot.borrow_mut();
        let Some(show) = slot.as_mut() else {
            return;
        };
        if matches!(&show.tail, TailAccumulator::Contradiction(_)) {
            return;
        }
        let geometry_witness = match &show.geometry {
            GeometryAccumulator::Valid(geometry)
                if geometry.callback_viewport == callback_viewport =>
            {
                Some(geometry.callback_witness)
            }
            GeometryAccumulator::Valid(_) | GeometryAccumulator::Empty => None,
            GeometryAccumulator::Contradiction(reason) => {
                show.tail = TailAccumulator::Contradiction(reason.clone());
                return;
            }
        };
        show.tail = match (geometry_witness, callback_witness) {
            (Some(begin), Some(end)) if begin == end => TailAccumulator::Valid(CallbackTail {
                callback_viewport,
                callback_witness: end,
                primary_down,
            }),
            _ => TailAccumulator::Contradiction(format!(
                "pointer callback backend scope changed before tail: viewport={callback_viewport:?}"
            )),
        };
    });
}

pub(crate) fn record_handler_receipt(receipt: HandlerReceipt) {
    ACTIVE_SHOW.with(|slot| {
        let mut slot = slot.borrow_mut();
        let Some(show) = slot.as_mut() else {
            return;
        };
        let callback_matches = matches!(
            &show.geometry,
            GeometryAccumulator::Valid(geometry)
                if geometry.callback_witness == receipt.child_witness
                    && geometry.callback_viewport == receipt.child_witness.viewport_id()
        );
        let expected_matches = show.owner.as_ref() == Some(&prepared_show_owner(&receipt.prepared))
            && receipt
                .prepared
                .step
                .latch
                .owner
                .identity
                .matches_backend_witness(receipt.child_witness);
        let delivery_matches = matches!(
            &show.delivery,
            DeliveryAccumulator::Exact(delivered)
                if delivered.prepared == receipt.prepared
                    && delivered.callback_witness == receipt.child_witness
        );
        if callback_matches && expected_matches && delivery_matches {
            show.receipt.observe(receipt);
        } else {
            show.ownership_error = Some(
                "pointer handler receipt did not belong to the actual callback owner".to_string(),
            );
        }
    });
}

fn key_region(region: RegionId) -> SyntheticPointerRegion {
    match region {
        RegionId::StillSeekStripRow => SyntheticPointerRegion::StillSeekStripRow,
        RegionId::StillSeekTrack => SyntheticPointerRegion::StillSeekTrack,
    }
}

fn region_from_key(region: SyntheticPointerRegion) -> RegionId {
    match region {
        SyntheticPointerRegion::StillSeekStripRow => RegionId::StillSeekStripRow,
        SyntheticPointerRegion::StillSeekTrack => RegionId::StillSeekTrack,
    }
}

fn pointer_step_point(kind: SyntheticPointerStepKind) -> egui::Pos2 {
    match kind {
        SyntheticPointerStepKind::Down { point } | SyntheticPointerStepKind::Move { point, .. } => {
            point
        }
        SyntheticPointerStepKind::Up { final_point, .. }
        | SyntheticPointerStepKind::CleanupUp { final_point } => final_point,
    }
}

fn current_input_proves_step(ctx: &egui::Context, prepared: &PreparedSyntheticPointerStep) -> bool {
    let expected = pointer_step_point(prepared.step.kind);
    ctx.input(|input| match prepared.step.kind {
        SyntheticPointerStepKind::Down { .. } => input.events.iter().any(|event| {
            matches!(event, egui::Event::PointerButton {
                pos,
                button: egui::PointerButton::Primary,
                pressed: true,
                ..
            } if *pos == expected)
        }),
        SyntheticPointerStepKind::Move { .. } => input
            .events
            .iter()
            .any(|event| matches!(event, egui::Event::PointerMoved(pos) if *pos == expected)),
        SyntheticPointerStepKind::Up { .. } | SyntheticPointerStepKind::CleanupUp { .. } => {
            let moved = input.events.iter().position(
                |event| matches!(event, egui::Event::PointerMoved(pos) if *pos == expected),
            );
            let released = input.events.iter().position(|event| {
                matches!(event, egui::Event::PointerButton {
                    pos,
                    button: egui::PointerButton::Primary,
                    pressed: false,
                    ..
                } if *pos == expected)
            });
            moved
                .zip(released)
                .is_some_and(|(moved, released)| moved < released)
        }
    })
}

/// Observe the real supported widget after its production handler has run. The immutable delivery
/// proof must be in this same lexical show and the exact raw edge must still belong to this pass.
/// Later passes may repeat a Move response, but cannot create a second effect without the edge.
pub(crate) fn observe_region_handler(
    response: &egui::Response,
    region: RegionId,
    effect_after: Option<HandlerEffect>,
) {
    let child_witness = eframe::miv_test_script_window_witness::active();
    ACTIVE_SHOW.with(|slot| {
        let slot = slot.borrow();
        let Some(show) = slot.as_ref() else {
            return;
        };
        let DeliveryAccumulator::Exact(delivery) = &show.delivery else {
            return;
        };
        let latch = &delivery.prepared.step.latch;
        let callback_geometry_matches = matches!(
            &show.geometry,
            GeometryAccumulator::Valid(geometry)
                if geometry.items_generation == latch.owner.items_generation
                    && geometry.mode.key_signature() == latch.mode
                    && geometry.pixels_per_point.to_bits()
                        == latch.press_pixels_per_point.to_bits()
                    && geometry.regions.iter().any(|named| {
                        named.id == region
                            && named.widget_id == latch.widget_id
                            && named.coordinate_frame == latch.coordinate_frame
                            && (!prepared_is_down(&delivery.prepared)
                                || named.rect == latch.press_rect)
                    })
                    && (!prepared_is_down(&delivery.prepared)
                        || (geometry.page_index == latch.press_page_index
                            && geometry.item_identity == latch.press_item_identity))
        );
        if latch.region != key_region(region)
            || latch.widget_id != response.id
            || latch.press_pixels_per_point.to_bits() != response.ctx.pixels_per_point().to_bits()
            || child_witness != Some(delivery.callback_witness)
            || !current_input_proves_step(&response.ctx, &delivery.prepared)
            || !callback_geometry_matches
        {
            return;
        }
        if prepared_is_down(&delivery.prepared) {
            if latch.press_rect != response.rect || !response.rect.contains(latch.press_point) {
                return;
            }
        }
        let response_proves_hit = match delivery.prepared.step.kind {
            SyntheticPointerStepKind::Down { .. } => response.is_pointer_button_down_on(),
            SyntheticPointerStepKind::Move { .. } => response.dragged(),
            SyntheticPointerStepKind::Up { .. } => response.drag_stopped() || response.clicked(),
            SyntheticPointerStepKind::CleanupUp { .. } => false,
        };
        if !response_proves_hit {
            return;
        }
        let effect_after = match delivery.prepared.step.kind {
            SyntheticPointerStepKind::Down { .. } => HandlerEffect::Pressed,
            SyntheticPointerStepKind::Move { .. } | SyntheticPointerStepKind::Up { .. } => {
                let Some(effect) = effect_after else {
                    return;
                };
                effect
            }
            SyntheticPointerStepKind::CleanupUp { .. } => return,
        };
        let receipt = HandlerReceipt {
            prepared: delivery.prepared.clone(),
            observed_point: pointer_step_point(delivery.prepared.step.kind),
            effect_after,
            child_witness: delivery.callback_witness,
        };
        drop(slot);
        record_handler_receipt(receipt);
    });
}

/// Called exactly once by the pointer branch of the child `input_hook`, after the timeline has
/// atomically accepted delivery and before returning the mutated RawInput. This proof is lexical-
/// show-local and immutable; it is not a second transaction owner.
pub(crate) fn record_delivery_proof(
    delivered: &PreparedSyntheticPointerStep,
    child_input: &egui::RawInput,
) {
    let callback_witness = eframe::miv_test_script_window_witness::active();
    let child_raw_time_bits = child_input
        .time
        .expect("key_input rejects pointer delivery without child RawInput.time")
        .to_bits();
    ACTIVE_SHOW.with(|slot| {
        let mut slot = slot.borrow_mut();
        let Some(show) = slot.as_mut() else {
            return;
        };
        let expected_matches = show.owner.as_ref() == Some(&prepared_show_owner(delivered));
        let witness_matches = callback_witness.is_some_and(|witness| {
            delivered
                .step
                .latch
                .owner
                .identity
                .matches_backend_witness(witness)
                && witness.viewport_id() == delivered.step.latch.owner.identity.viewport_id()
                && child_input.viewport_id == witness.viewport_id()
        });
        if expected_matches && witness_matches {
            show.delivery.observe(InputDeliveryProof {
                prepared: delivered.clone(),
                callback_witness: callback_witness.expect("checked above"),
                child_viewport: child_input.viewport_id,
                child_raw_time_bits,
            });
        } else {
            show.ownership_error =
                Some("pointer delivery did not belong to the lexical show owner".to_string());
        }
    });
}

pub(crate) fn active_show_owner() -> Option<ShowOwner> {
    ACTIVE_SHOW.with(|slot| slot.borrow().as_ref().and_then(|show| show.owner.clone()))
}

pub(crate) fn active_show_matches(owner: &SyntheticPointerOwner) -> bool {
    active_show_owner().as_ref() == Some(&show_owner_from_pointer(owner))
}

#[derive(Clone, Debug)]
pub(crate) struct ShowOutput {
    owner: Option<ShowOwner>,
    geometry: GeometryAccumulator,
    tail: TailAccumulator,
    delivery: DeliveryAccumulator,
    receipt: ReceiptAccumulator,
    ownership_error: Option<String>,
}

#[derive(Clone, Debug)]
pub(crate) struct DeliveredShowStep {
    pub(crate) delivery: InputDeliveryProof,
    pub(crate) handler: DeliveredHandlerProof,
}

#[derive(Clone, Debug)]
pub(crate) enum DeliveredHandlerProof {
    Success(HandlerReceipt),
    /// The event reached this show but no supported widget proved an effect. The post-show reducer
    /// fails/cleans up this step rather than borrowing a later show's receipt or tail.
    Missing,
    /// Conflicting handler observations are an effect failure passed to the reducer. Callback
    /// ownership contradictions remain separate hard errors on `ShowOutput`.
    Contradiction(String),
}

impl ShowOutput {
    pub(crate) fn pointer_owner(&self) -> Option<SyntheticPointerOwner> {
        self.owner.as_ref().map(|owner| SyntheticPointerOwner {
            identity: owner.identity.clone(),
            items_generation: owner.items_generation,
        })
    }

    pub(crate) fn catalog_owner(&self) -> Option<TestScriptWindowIdentity> {
        self.owner.as_ref().map(|owner| owner.identity.clone())
    }

    pub(crate) fn has_pointer_obligation(&self) -> bool {
        if !matches!(&self.delivery, DeliveryAccumulator::Empty) {
            return true;
        }
        self.pointer_owner()
            .as_ref()
            .is_some_and(crate::key_input::synthetic_pointer_obligates_owner)
    }

    pub(crate) fn delivered_cancel_handle(
        &self,
    ) -> Option<crate::key_input::SyntheticPointerCancelHandle> {
        match &self.delivery {
            DeliveryAccumulator::Exact(delivery) => {
                Some(crate::key_input::SyntheticPointerCancelHandle {
                    transaction_id: delivery.prepared.step.latch.transaction_id,
                    step_id: delivery.prepared.step.step_id,
                    owner: delivery.prepared.step.latch.owner.clone(),
                })
            }
            DeliveryAccumulator::Empty | DeliveryAccumulator::Contradiction(_) => None,
        }
    }

    fn take_delivered_step(&mut self) -> Result<Option<DeliveredShowStep>, String> {
        if let Some(reason) = self.ownership_error.take() {
            return Err(reason);
        }
        let delivery = match std::mem::replace(&mut self.delivery, DeliveryAccumulator::Empty) {
            DeliveryAccumulator::Empty => {
                if matches!(&self.receipt, ReceiptAccumulator::Empty) {
                    return Ok(None);
                }
                return Err("pointer handler proof exists without show-local delivery".to_string());
            }
            DeliveryAccumulator::Exact(delivery) => delivery,
            DeliveryAccumulator::Contradiction(reason) => return Err(reason),
        };
        let handler = match std::mem::replace(&mut self.receipt, ReceiptAccumulator::Empty) {
            ReceiptAccumulator::Empty => DeliveredHandlerProof::Missing,
            ReceiptAccumulator::Valid(receipt) if receipt.prepared == delivery.prepared => {
                DeliveredHandlerProof::Success(receipt)
            }
            ReceiptAccumulator::Valid(_) => {
                return Err("pointer handler proof belongs to another delivered step".to_string());
            }
            ReceiptAccumulator::Contradiction(reason) => {
                DeliveredHandlerProof::Contradiction(reason)
            }
        };
        Ok(Some(DeliveredShowStep { delivery, handler }))
    }

    /// Join actual callback evidence only after ordinary post-show host registration has refreshed
    /// the authoritative App identity. The owner is attached to geometry only after every exact
    /// equality below succeeds.
    pub(crate) fn take_joined_region_frame(
        &mut self,
        current_identity: &TestScriptWindowIdentity,
        current_items_generation: u64,
    ) -> Result<(RegionFrame, CallbackTail, Option<DeliveredShowStep>), String> {
        let geometry = match std::mem::replace(&mut self.geometry, GeometryAccumulator::Empty) {
            GeometryAccumulator::Valid(geometry) => geometry,
            GeometryAccumulator::Empty => {
                return Err("pointer show produced no geometry pass".to_string());
            }
            GeometryAccumulator::Contradiction(reason) => return Err(reason),
        };
        if geometry.items_generation != current_items_generation {
            return Err("pointer show catalog changed before publication".to_string());
        }
        let expected = match self.owner.clone() {
            Some(expected)
                if expected.identity == *current_identity
                    && expected.items_generation == current_items_generation =>
            {
                expected
            }
            Some(_) => {
                return Err("pointer show owner/catalog changed before publication".to_string());
            }
            None if matches!(&self.delivery, DeliveryAccumulator::Empty) => ShowOwner {
                identity: current_identity.clone(),
                items_generation: current_items_generation,
            },
            None => {
                return Err("pointer delivery exists without a show-pre exact owner".to_string());
            }
        };
        let tail = match std::mem::replace(&mut self.tail, TailAccumulator::Empty) {
            TailAccumulator::Valid(tail) => tail,
            TailAccumulator::Empty => {
                return Err("pointer show produced no callback tail".to_string());
            }
            TailAccumulator::Contradiction(reason) => return Err(reason),
        };
        if geometry.callback_viewport != tail.callback_viewport
            || geometry.callback_witness != tail.callback_witness
            || geometry.callback_viewport != current_identity.viewport_id()
            || !current_identity.matches_backend_witness(geometry.callback_witness)
        {
            return Err(
                "pointer geometry/tail did not join the current exact backend host".to_string(),
            );
        }
        let delivered_step = self.take_delivered_step()?;
        if delivered_step.as_ref().is_some_and(|delivered| {
            prepared_show_owner(&delivered.delivery.prepared) != expected
                || delivered.delivery.callback_witness != geometry.callback_witness
                || delivered.delivery.child_viewport != geometry.callback_viewport
                || delivered.delivery.child_raw_time_bits != geometry.raw_input_time_bits
        }) {
            return Err(
                "pointer delivery did not join the geometry/tail callback host".to_string(),
            );
        }

        Ok((
            RegionFrame {
                owner: current_identity.clone(),
                items_generation: geometry.items_generation,
                page_index: geometry.page_index,
                item_identity: geometry.item_identity,
                mode: geometry.mode,
                viewport_frame: geometry.viewport_frame,
                raw_input_time_bits: geometry.raw_input_time_bits,
                pixels_per_point: geometry.pixels_per_point,
                revision: 0,
                regions: geometry.regions,
                strip_layout: geometry.strip_layout,
            },
            tail,
            delivered_step,
        ))
    }
}

fn key_handler_effect(effect: HandlerEffect) -> SyntheticPointerHandlerEffect {
    match effect {
        HandlerEffect::Pressed => SyntheticPointerHandlerEffect::Pressed,
        HandlerEffect::StripCenter(center) => SyntheticPointerHandlerEffect::StripCenter(center),
        HandlerEffect::TrackTarget(target) => SyntheticPointerHandlerEffect::TrackTarget(target),
        HandlerEffect::ReleasedStripCenter(center) => {
            SyntheticPointerHandlerEffect::ReleasedStripCenter(center)
        }
        HandlerEffect::ReleasedTrackTarget(target) => {
            SyntheticPointerHandlerEffect::ReleasedTrackTarget(target)
        }
    }
}

/// Complete exactly the step delivered in this lexical child show. The caller invokes this only
/// after the ordinary fullscreen navigation tail and passes the resulting current page. Region
/// publication remains a separate next-paint observation.
pub(crate) struct JoinedShowOutput {
    pub(crate) frame: RegionFrame,
    delivered: Option<JoinedDeliveredStep>,
}

struct JoinedDeliveredStep {
    step: crate::key_input::SyntheticPointerStep,
    primary_down: bool,
    handler: DeliveredHandlerProof,
    page_before: usize,
    page_after: usize,
}

impl JoinedShowOutput {
    /// Finish the timeline only after this exact show's geometry has been published and assigned
    /// its real catalog revision. This keeps a completion from racing ahead of its paint barrier.
    pub(crate) fn finish(self, after_revision: u64) -> Result<(), String> {
        let Some(delivered) = self.delivered else {
            return Ok(());
        };
        let step_id = delivered.step.step_id;
        let handler = match delivered.handler {
            DeliveredHandlerProof::Success(receipt) => SyntheticPointerHandlerProof::Success(
                crate::key_input::SyntheticPointerCompletion {
                    step_id,
                    effect: key_handler_effect(receipt.effect_after),
                    page_before: delivered.page_before,
                    page_after: delivered.page_after,
                    after_revision,
                },
            ),
            DeliveredHandlerProof::Missing => SyntheticPointerHandlerProof::Missing,
            DeliveredHandlerProof::Contradiction(reason) => {
                SyntheticPointerHandlerProof::Contradiction(reason)
            }
        };
        crate::key_input::finish_synthetic_pointer_show(SyntheticPointerShowTail {
            step: delivered.step,
            primary_down: delivered.primary_down,
            handler,
        })
    }
}

pub(crate) fn join_show_output(
    mut output: ShowOutput,
    current_identity: &TestScriptWindowIdentity,
    current_items_generation: u64,
    page_after_navigation: usize,
) -> Result<JoinedShowOutput, String> {
    let joined = output.take_joined_region_frame(current_identity, current_items_generation);
    let (frame, tail, delivered) = match joined {
        Ok(joined) => joined,
        Err(error) => return Err(error),
    };
    let delivered = delivered.map(|delivered| JoinedDeliveredStep {
        step: delivered.delivery.prepared.step,
        primary_down: tail.primary_down,
        handler: delivered.handler,
        page_before: frame.page_index,
        page_after: page_after_navigation,
    });
    Ok(JoinedShowOutput { frame, delivered })
}

fn rhai_error(message: impl Into<String>) -> Box<EvalAltResult> {
    EvalAltResult::ErrorRuntime(Dynamic::from(message.into()), rhai::Position::NONE).into()
}

fn checked_normalized(x: rhai::FLOAT, y: rhai::FLOAT) -> Result<[f32; 2], Box<EvalAltResult>> {
    if !x.is_finite() || !y.is_finite() {
        return Err(rhai_error("egui pointer coordinates must be finite"));
    }
    // Do not silently clamp. Values outside 0..=1 are useful only if a reviewed scenario needs to
    // leave the region (e.g. downward close); the initial S2 scripts stay within the latched rect.
    if !(0.0..=1.0).contains(&x) || !(0.0..=1.0).contains(&y) {
        return Err(rhai_error(
            "egui pointer coordinates must be between zero and one",
        ));
    }
    Ok([x as f32, y as f32])
}

fn checked_timeout(timeout_ms: rhai::INT) -> Result<Duration, Box<EvalAltResult>> {
    let timeout = u64::try_from(timeout_ms)
        .ok()
        .filter(|value| *value > 0)
        .map(Duration::from_millis)
        .ok_or_else(|| rhai_error("egui pointer timeout_ms must be greater than zero"))?;
    Ok(timeout)
}

fn wait_for_region(
    bridge: &RunnerBridge,
    catalog: &SharedRegionCatalog,
    region: RegionId,
    after_revision: u64,
    timeout: Duration,
) -> Result<Map, String> {
    let deadline = Instant::now()
        .checked_add(timeout)
        .ok_or_else(|| "egui pointer timeout is too large".to_string())?;
    loop {
        bridge.interrupt.check()?;
        let owner = bridge.selected_detached_identity()?;
        let snapshot = bridge.latest_snapshot()?;
        let current = snapshot
            .windows
            .iter()
            .find(|window| window.identity.as_ref() == Some(&owner))
            .ok_or_else(|| {
                format!(
                    "selected egui pointer owner is no longer current: {}",
                    owner.describe()
                )
            })?;
        let current_region = catalog
            .read()
            .map_err(|_| "test-script pointer region catalog is poisoned".to_string())?
            .current_region(&owner, current, region, after_revision)
            .map(|(frame, named)| (frame.clone(), named.clone()));
        if let Some((frame, named)) = current_region {
            return Ok(region_to_map(&frame, &named));
        }
        if Instant::now() >= deadline {
            return Err(format!(
                "timed out waiting for current egui pointer region {}",
                region.as_str()
            ));
        }
        (bridge.wake)();
        bridge.interrupt.wait(
            Duration::from_millis(10).min(deadline.saturating_duration_since(Instant::now())),
        )?;
    }
}

fn region_to_map(frame: &RegionFrame, named: &NamedRegion) -> Map {
    let mut map = Map::new();
    map.insert("name".into(), Dynamic::from(named.id.as_str()));
    map.insert(
        "revision".into(),
        Dynamic::from(super::saturating_rhai_int(frame.revision)),
    );
    map.insert(
        "geometry_token".into(),
        Dynamic::from(super::saturating_rhai_int(named.geometry_token)),
    );
    map.insert(
        "items_generation".into(),
        Dynamic::from(super::saturating_rhai_int(frame.items_generation)),
    );
    map.insert(
        "page_index".into(),
        Dynamic::from(super::saturating_rhai_int(frame.page_index as u64)),
    );
    map.insert(
        "item_identity".into(),
        Dynamic::from(frame.item_identity.clone()),
    );
    map.insert(
        "spread_mode".into(),
        Dynamic::from(frame.mode.spread_mode.clone()),
    );
    map.insert(
        "reading_flow".into(),
        Dynamic::from(frame.mode.reading_flow.clone()),
    );
    map.insert("strip_rtl".into(), Dynamic::from(frame.mode.strip_rtl));
    map.insert(
        "seek_bar_rtl".into(),
        Dynamic::from(frame.mode.seek_bar_rtl),
    );
    map.insert(
        "strip_visible".into(),
        Dynamic::from(frame.mode.strip_visible),
    );
    map.insert(
        "strip_locked".into(),
        Dynamic::from(frame.mode.strip_locked),
    );
    map.insert("bar_locked".into(), Dynamic::from(frame.mode.bar_locked));
    map.insert(
        "left".into(),
        Dynamic::from(named.rect.left() as rhai::FLOAT),
    );
    map.insert("top".into(), Dynamic::from(named.rect.top() as rhai::FLOAT));
    map.insert(
        "right".into(),
        Dynamic::from(named.rect.right() as rhai::FLOAT),
    );
    map.insert(
        "bottom".into(),
        Dynamic::from(named.rect.bottom() as rhai::FLOAT),
    );
    map.insert(
        "coordinate_left".into(),
        Dynamic::from(named.coordinate_frame.left() as rhai::FLOAT),
    );
    map.insert(
        "coordinate_top".into(),
        Dynamic::from(named.coordinate_frame.top() as rhai::FLOAT),
    );
    map.insert(
        "coordinate_right".into(),
        Dynamic::from(named.coordinate_frame.right() as rhai::FLOAT),
    );
    map.insert(
        "coordinate_bottom".into(),
        Dynamic::from(named.coordinate_frame.bottom() as rhai::FLOAT),
    );
    map.insert(
        "pixels_per_point".into(),
        Dynamic::from(frame.pixels_per_point as rhai::FLOAT),
    );
    if let Some(layout) = &frame.strip_layout {
        map.insert(
            "strip_center".into(),
            Dynamic::from(super::saturating_rhai_int(layout.center_pos as u64)),
        );
        let cells = layout
            .cell_indices
            .iter()
            .map(|index| Dynamic::from(super::saturating_rhai_int(*index as u64)))
            .collect::<rhai::Array>();
        map.insert("strip_cells".into(), Dynamic::from_array(cells));
    }
    map
}

fn selected_current_region(
    bridge: &RunnerBridge,
    catalog: &SharedRegionCatalog,
    region: RegionId,
    geometry_token: u64,
    deadline: Instant,
) -> Result<(TestScriptWindowIdentity, RegionFrame, NamedRegion), String> {
    let owner = bridge.selected_detached_identity()?;
    bridge.validate_selected_owner_fresh(&owner, deadline)?;
    let snapshot = bridge.latest_snapshot()?;
    let current = snapshot
        .windows
        .iter()
        .find(|window| window.identity.as_ref() == Some(&owner))
        .ok_or_else(|| format!("selected egui pointer owner is stale: {}", owner.describe()))?;
    let (frame, named) = catalog
        .read()
        .map_err(|_| "test-script pointer region catalog is poisoned".to_string())?
        .current_region(&owner, current, region, 0)
        .filter(|(_, named)| named.geometry_token == geometry_token)
        .map(|(frame, named)| (frame.clone(), named.clone()))
        .ok_or_else(|| {
            format!(
                "egui pointer region geometry changed; reacquire it: region={} geometry_token={geometry_token}",
                region.as_str()
            )
        })?;
    Ok((owner, frame, named))
}

fn validate_held_owner_fresh(
    bridge: &RunnerBridge,
    catalog: &SharedRegionCatalog,
    latch: &crate::key_input::SyntheticPointerLatch,
    deadline: Instant,
) -> Result<(), String> {
    let owner = &latch.owner;
    bridge.validate_selected_owner_fresh(&owner.identity, deadline)?;
    let snapshot = bridge.latest_snapshot()?;
    let Some(current) = snapshot
        .windows
        .iter()
        .find(|window| window.identity.as_ref() == Some(&owner.identity))
    else {
        return Err(format!(
            "synthetic pointer owner disappeared during gesture: {}",
            owner.identity.describe()
        ));
    };
    if current.items_generation != owner.items_generation {
        return Err(format!(
            "synthetic pointer catalog changed during gesture: owner={} latched_generation={} current_generation={}",
            owner.identity.describe(),
            owner.items_generation,
            current.items_generation
        ));
    }
    let current_geometry = catalog
        .read()
        .map_err(|_| "test-script pointer region catalog is poisoned".to_string())?
        .held_region(
            &owner.identity,
            current.items_generation,
            region_from_key(latch.region),
        )
        .map(|(frame, named)| (frame.clone(), named.clone()));
    let Some((frame, named)) = current_geometry else {
        return Err(format!(
            "synthetic pointer region is not current during gesture: owner={} region={:?}",
            owner.identity.describe(),
            latch.region
        ));
    };
    if !held_region_coordinate_frame_matches(latch, &frame, &named) {
        return Err(format!(
            "synthetic pointer coordinate frame changed during gesture: owner={} region={:?}",
            owner.identity.describe(),
            latch.region
        ));
    }
    Ok(())
}

fn held_region_coordinate_frame_matches(
    latch: &crate::key_input::SyntheticPointerLatch,
    frame: &RegionFrame,
    named: &NamedRegion,
) -> bool {
    frame.owner == latch.owner.identity
        && frame.items_generation == latch.owner.items_generation
        && frame.mode.key_signature() == latch.mode
        && frame.pixels_per_point.to_bits() == latch.press_pixels_per_point.to_bits()
        && named.id == region_from_key(latch.region)
        && named.widget_id == latch.widget_id
        && named.coordinate_frame == latch.coordinate_frame
    // `named.rect` and its Down-only geometry token intentionally do not participate here. The
    // visible strip row moves as cells move; Move/Up stay in the immutable press coordinate frame.
}

fn wait_for_completion(
    bridge: &RunnerBridge,
    cancel_handle: &crate::key_input::SyntheticPointerCancelHandle,
    completion: std::sync::mpsc::Receiver<
        Result<crate::key_input::SyntheticPointerCompletion, String>,
    >,
    deadline: Instant,
) -> Result<crate::key_input::SyntheticPointerCompletion, String> {
    let cancel_and_wake = || {
        let _ = crate::key_input::cancel_synthetic_pointer_step(cancel_handle);
        (bridge.wake)();
    };
    loop {
        if let Err(error) = bridge.interrupt.check() {
            cancel_and_wake();
            return Err(error);
        }
        let now = Instant::now();
        if now >= deadline {
            cancel_and_wake();
            return Err("timed out waiting for egui pointer step completion".to_string());
        }
        match completion
            .recv_timeout(super::WAIT_POLL_INTERVAL.min(deadline.saturating_duration_since(now)))
        {
            Ok(Ok(result)) => return Ok(result),
            Ok(Err(error)) => {
                cancel_and_wake();
                return Err(error);
            }
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
                cancel_and_wake();
                return Err("egui pointer completion channel disconnected".to_string());
            }
        }
    }
}

fn completion_to_map(completion: crate::key_input::SyntheticPointerCompletion) -> Map {
    let mut map = Map::new();
    map.insert(
        "step_id".into(),
        Dynamic::from(super::saturating_rhai_int(completion.step_id)),
    );
    map.insert(
        "page_before".into(),
        Dynamic::from(super::saturating_rhai_int(completion.page_before as u64)),
    );
    map.insert(
        "page_after".into(),
        Dynamic::from(super::saturating_rhai_int(completion.page_after as u64)),
    );
    map.insert(
        "after_revision".into(),
        Dynamic::from(super::saturating_rhai_int(completion.after_revision)),
    );
    let (effect, value) = match completion.effect {
        SyntheticPointerHandlerEffect::Pressed => ("pressed", None),
        SyntheticPointerHandlerEffect::StripCenter(center) => ("strip_center", Some(center)),
        SyntheticPointerHandlerEffect::TrackTarget(target) => ("track_target", target),
        SyntheticPointerHandlerEffect::ReleasedStripCenter(center) => {
            ("released_strip_center", Some(center))
        }
        SyntheticPointerHandlerEffect::ReleasedTrackTarget(target) => {
            ("released_track_target", target)
        }
    };
    map.insert("effect".into(), Dynamic::from(effect));
    map.insert(
        "effect_value".into(),
        value
            .map(|value| Dynamic::from(super::saturating_rhai_int(value as u64)))
            .unwrap_or(Dynamic::UNIT),
    );
    map
}

pub(super) fn register(engine: &mut Engine, bridge: RunnerBridge, catalog: SharedRegionCatalog) {
    let region_bridge = bridge.clone();
    let region_catalog = Arc::clone(&catalog);
    engine.register_fn(
        "egui_pointer_region",
        move |name: ImmutableString,
              after_revision: rhai::INT,
              timeout_ms: rhai::INT|
              -> Result<Map, Box<EvalAltResult>> {
            let region = RegionId::parse(&name).map_err(rhai_error)?;
            let after_revision = u64::try_from(after_revision)
                .map_err(|_| rhai_error("after_revision must be non-negative"))?;
            wait_for_region(
                &region_bridge,
                &region_catalog,
                region,
                after_revision,
                checked_timeout(timeout_ms)?,
            )
            .map_err(rhai_error)
        },
    );

    let down_bridge = bridge.clone();
    let down_catalog = Arc::clone(&catalog);
    engine.register_fn(
        "egui_pointer_down",
        move |name: ImmutableString,
              geometry_token: rhai::INT,
              x: rhai::FLOAT,
              y: rhai::FLOAT,
              timeout_ms: rhai::INT|
              -> Result<Map, Box<EvalAltResult>> {
            let region = RegionId::parse(&name).map_err(rhai_error)?;
            let geometry_token = u64::try_from(geometry_token)
                .map_err(|_| rhai_error("egui pointer geometry_token must be non-negative"))?;
            let normalized = checked_normalized(x, y)?;
            let timeout = checked_timeout(timeout_ms)?;
            let deadline = Instant::now()
                .checked_add(timeout)
                .ok_or_else(|| rhai_error("egui pointer timeout is too large"))?;
            let (identity, frame, named) = selected_current_region(
                &down_bridge,
                &down_catalog,
                region,
                geometry_token,
                deadline,
            )
            .map_err(rhai_error)?;
            let press_point = egui::pos2(
                egui::lerp(named.rect.x_range(), normalized[0]),
                egui::lerp(named.rect.y_range(), normalized[1]),
            );
            let key_region = match region {
                RegionId::StillSeekStripRow => SyntheticPointerRegion::StillSeekStripRow,
                RegionId::StillSeekTrack => SyntheticPointerRegion::StillSeekTrack,
            };
            let (cancel_handle, completion) =
                crate::key_input::enqueue_synthetic_pointer_down(SyntheticPointerDownRequest {
                    owner: SyntheticPointerOwner {
                        identity,
                        items_generation: frame.items_generation,
                    },
                    region: key_region,
                    mode: frame.mode.key_signature(),
                    region_geometry_token: named.geometry_token,
                    press_page_index: frame.page_index,
                    press_item_identity: frame.item_identity,
                    widget_id: named.widget_id,
                    press_rect: named.rect,
                    coordinate_frame: named.coordinate_frame,
                    press_pixels_per_point: frame.pixels_per_point,
                    press_point,
                })
                .map_err(rhai_error)?;
            (down_bridge.wake)();
            Ok(completion_to_map(
                wait_for_completion(&down_bridge, &cancel_handle, completion, deadline)
                    .map_err(rhai_error)?,
            ))
        },
    );

    for (name, phase) in [
        (
            "egui_pointer_move",
            crate::key_input::SyntheticPointerHeldPhase::Move,
        ),
        (
            "egui_pointer_up",
            crate::key_input::SyntheticPointerHeldPhase::Up,
        ),
    ] {
        let held_bridge = bridge.clone();
        let held_catalog = Arc::clone(&catalog);
        engine.register_fn(
            name,
            move |x: rhai::FLOAT,
                  y: rhai::FLOAT,
                  timeout_ms: rhai::INT|
                  -> Result<Map, Box<EvalAltResult>> {
                let normalized = checked_normalized(x, y)?;
                let timeout = checked_timeout(timeout_ms)?;
                let deadline = Instant::now()
                    .checked_add(timeout)
                    .ok_or_else(|| rhai_error("egui pointer timeout is too large"))?;
                let held = crate::key_input::synthetic_pointer_held_snapshot()
                    .ok_or_else(|| rhai_error("synthetic pointer is not held"))?;
                if let Err(error) =
                    validate_held_owner_fresh(&held_bridge, &held_catalog, &held.latch, deadline)
                {
                    if crate::key_input::cancel_synthetic_pointer_step(&held.cancel_handle) {
                        (held_bridge.wake)();
                    }
                    return Err(rhai_error(error));
                }
                let (cancel_handle, completion) =
                    match crate::key_input::enqueue_synthetic_pointer_held(
                        &held.latch.owner,
                        phase,
                        normalized,
                    ) {
                        Ok(queued) => queued,
                        Err(error) => {
                            if crate::key_input::cancel_synthetic_pointer_step(&held.cancel_handle)
                            {
                                (held_bridge.wake)();
                            }
                            return Err(rhai_error(error));
                        }
                    };
                (held_bridge.wake)();
                Ok(completion_to_map(
                    wait_for_completion(&held_bridge, &cancel_handle, completion, deadline)
                        .map_err(rhai_error)?,
                ))
            },
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn detached_identity(
        viewport_id: egui::ViewportId,
        backend_token: u64,
    ) -> TestScriptWindowIdentity {
        TestScriptWindowIdentity::Detached {
            window_id: 1,
            context_serial: 2,
            viewport_id,
            host_incarnation: 3,
            hwnd: 4,
            backend_token,
        }
    }

    fn region_frame(
        owner: &TestScriptWindowIdentity,
        page_index: usize,
        item_identity: &str,
        rect: egui::Rect,
    ) -> RegionFrame {
        RegionFrame {
            owner: owner.clone(),
            items_generation: 7,
            page_index,
            item_identity: item_identity.to_string(),
            mode: mode_proof(),
            viewport_frame: 1,
            raw_input_time_bits: 0.0_f64.to_bits(),
            pixels_per_point: 1.0,
            revision: 0,
            regions: vec![NamedRegion {
                id: RegionId::StillSeekTrack,
                widget_id: egui::Id::new("pointer-track"),
                rect,
                coordinate_frame: rect,
                geometry_token: 0,
            }],
            strip_layout: None,
        }
    }

    fn mode_proof() -> FullscreenModeProof {
        FullscreenModeProof {
            spread_mode: "Single".to_string(),
            reading_flow: "Paged".to_string(),
            strip_rtl: false,
            seek_bar_rtl: false,
            strip_visible: true,
            strip_locked: true,
            bar_locked: true,
        }
    }

    fn current_window_snapshot(
        owner: &TestScriptWindowIdentity,
        page_index: usize,
        item_identity: &str,
    ) -> TestScriptWindowSnapshot {
        TestScriptWindowSnapshot {
            identity: Some(owner.clone()),
            role: "detached".to_string(),
            window_id: owner.window_id(),
            context_serial: owner.context_serial(),
            viewport_id: owner.viewport_id(),
            host_incarnation: owner.host_incarnation(),
            hwnd: Some(owner.hwnd()),
            backend_token: Some(owner.backend_token()),
            residence: "mounted".to_string(),
            media_kind: "image".to_string(),
            page_index: Some(page_index),
            items_generation: 7,
            item_identity: item_identity.to_string(),
            page_ready: true,
            viewport_rendered: true,
            viewport_revision: 1,
            paint_matches_current_page: true,
            full_texture_painted: true,
            paint_source: "fullscreen".to_string(),
            paint_source_texture: "source".to_string(),
            painted_page_index: Some(page_index),
            paint_revision: 1,
            seek_strip: crate::test_script::TestScriptSeekStripSnapshot::closed(),
        }
    }

    fn prepared_step(
        identity: &TestScriptWindowIdentity,
        root_time: f64,
        kind: SyntheticPointerStepKind,
    ) -> PreparedSyntheticPointerStep {
        crate::key_input::prepared_synthetic_pointer_step_for_test(
            9,
            crate::key_input::SyntheticPointerLatch {
                transaction_id: 8,
                owner: SyntheticPointerOwner {
                    identity: identity.clone(),
                    items_generation: 7,
                },
                region: SyntheticPointerRegion::StillSeekTrack,
                mode: mode_proof().key_signature(),
                region_geometry_token: 6,
                press_page_index: 2,
                press_item_identity: "page-2".to_string(),
                widget_id: egui::Id::new("pointer-track"),
                press_rect: egui::Rect::from_min_max(
                    egui::pos2(10.0, 20.0),
                    egui::pos2(110.0, 40.0),
                ),
                coordinate_frame: egui::Rect::from_min_max(
                    egui::pos2(10.0, 20.0),
                    egui::pos2(110.0, 40.0),
                ),
                press_pixels_per_point: 1.0,
                press_point: egui::pos2(20.0, 30.0),
            },
            kind,
            10,
            root_time.to_bits(),
        )
    }

    #[test]
    fn receipt_accumulator_keeps_release_from_first_pass() {
        let ctx = egui::Context::default();
        let viewport = egui::ViewportId::ROOT;
        let backend = eframe::miv_test_script_window_witness::WindowWitnessFixture::new();
        let _backend_scope = backend.enter(&ctx, viewport, 4);
        let witness = eframe::miv_test_script_window_witness::active().unwrap();
        let identity = detached_identity(viewport, witness.token());
        ctx.begin_pass(egui::RawInput {
            viewport_id: viewport,
            time: Some(0.0),
            ..Default::default()
        });
        let prepared = prepared_step(
            &identity,
            1.0,
            SyntheticPointerStepKind::Up {
                held_before: egui::pos2(20.0, 30.0),
                final_point: egui::pos2(80.0, 30.0),
            },
        );
        let show = enter_show(Some(ShowOwner {
            identity,
            items_generation: 7,
        }));
        begin_pass(&ctx, 7, 2, "page-2".to_string(), mode_proof());
        ACTIVE_SHOW.with(|slot| {
            slot.borrow_mut()
                .as_mut()
                .unwrap()
                .receipt
                .observe(HandlerReceipt {
                    prepared,
                    observed_point: egui::pos2(80.0, 30.0),
                    effect_after: HandlerEffect::ReleasedTrackTarget(Some(2)),
                    child_witness: witness,
                });
        });
        // A later egui discard pass has new geometry/tail evidence but no repeated release edge.
        // `begin_pass` must keep the run-local handler receipt from the actual delivery pass.
        begin_pass(&ctx, 7, 2, "page-2".to_string(), mode_proof());
        let output = show.finish();
        let _ = ctx.end_pass();
        assert!(matches!(output.receipt, ReceiptAccumulator::Valid(_)));
    }

    #[test]
    fn child_delivery_clock_joins_callback_without_matching_root_clock() {
        let ctx = egui::Context::default();
        let viewport = egui::ViewportId::ROOT;
        let backend = eframe::miv_test_script_window_witness::WindowWitnessFixture::new();
        let _backend_scope = backend.enter(&ctx, viewport, 4);
        let witness = eframe::miv_test_script_window_witness::active().unwrap();
        let identity = detached_identity(viewport, witness.token());
        let prepared = prepared_step(
            &identity,
            1.0,
            SyntheticPointerStepKind::Move {
                held_before: egui::pos2(20.0, 30.0),
                point: egui::pos2(40.0, 30.0),
            },
        );
        let child_input = egui::RawInput {
            viewport_id: viewport,
            time: Some(1.005),
            ..Default::default()
        };
        ctx.begin_pass(child_input.clone());
        let show = enter_show(Some(ShowOwner {
            identity: identity.clone(),
            items_generation: 7,
        }));
        begin_pass(&ctx, 7, 2, "page-2".to_string(), mode_proof());
        record_delivery_proof(&prepared, &child_input);
        finish_pass(&ctx);
        // A second egui discard pass in the same child run keeps the child input time and the
        // immutable first-pass delivery proof. ROOT's 1.0 timestamp is intentionally different.
        begin_pass(&ctx, 7, 2, "page-2".to_string(), mode_proof());
        finish_pass(&ctx);
        let mut output = show.finish();
        let joined = output.take_joined_region_frame(&identity, 7);
        let _ = ctx.end_pass();
        assert!(joined.is_ok());
    }

    #[test]
    fn completion_revision_rejects_its_own_show_and_accepts_only_a_following_paint() {
        let _serial = crate::key_input::TEST_INPUT_LOCK
            .get_or_init(|| std::sync::Mutex::new(()))
            .lock()
            .expect("key input test lock poisoned");
        crate::key_input::clear_test_synthetic_input();
        let ctx = egui::Context::default();
        let viewport = egui::ViewportId::ROOT;
        let backend = eframe::miv_test_script_window_witness::WindowWitnessFixture::new();
        let _backend_scope = backend.enter(&ctx, viewport, 4);
        let witness = eframe::miv_test_script_window_witness::active().unwrap();
        let owner = detached_identity(viewport, witness.token());
        let prepared = prepared_step(
            &owner,
            1.0,
            SyntheticPointerStepKind::Down {
                point: egui::pos2(20.0, 30.0),
            },
        );
        let completion =
            crate::key_input::install_synthetic_pointer_delivered_for_test(prepared.clone());
        assert!(matches!(
            completion.try_recv(),
            Err(std::sync::mpsc::TryRecvError::Empty)
        ));
        let frame = region_frame(
            &owner,
            prepared.step.latch.press_page_index,
            &prepared.step.latch.press_item_identity,
            prepared.step.latch.press_rect,
        );
        let mut catalog = RegionCatalog::default();
        let published_revision = catalog.publish(frame.clone());
        assert!(matches!(
            completion.try_recv(),
            Err(std::sync::mpsc::TryRecvError::Empty)
        ));
        JoinedShowOutput {
            frame,
            delivered: Some(JoinedDeliveredStep {
                step: prepared.step.clone(),
                primary_down: true,
                handler: DeliveredHandlerProof::Success(HandlerReceipt {
                    prepared,
                    observed_point: egui::pos2(20.0, 30.0),
                    effect_after: HandlerEffect::Pressed,
                    child_witness: witness,
                }),
                page_before: 2,
                page_after: 2,
            }),
        }
        .finish(published_revision)
        .expect("the published show must finish its exact timeline step");
        let completion = completion
            .recv()
            .expect("completion sender")
            .expect("successful pointer completion");
        assert_eq!(completion.after_revision, published_revision);

        let current = current_window_snapshot(&owner, 2, "page-2");
        assert!(
            catalog
                .current_region(
                    &owner,
                    &current,
                    RegionId::StillSeekTrack,
                    completion.after_revision,
                )
                .is_none(),
            "the handler's own pre-effect geometry must not satisfy its next-paint barrier"
        );
        let following_revision = catalog.publish(region_frame(
            &owner,
            2,
            "page-2",
            egui::Rect::from_min_max(egui::pos2(10.0, 20.0), egui::pos2(110.0, 40.0)),
        ));
        assert!(following_revision > completion.after_revision);
        assert!(
            catalog
                .current_region(
                    &owner,
                    &current,
                    RegionId::StillSeekTrack,
                    completion.after_revision,
                )
                .is_some(),
            "only the following paint may satisfy the completion barrier"
        );
        crate::key_input::clear_test_synthetic_input();
    }

    #[test]
    fn callback_clock_must_match_child_delivery_clock() {
        let ctx = egui::Context::default();
        let viewport = egui::ViewportId::ROOT;
        let backend = eframe::miv_test_script_window_witness::WindowWitnessFixture::new();
        let _backend_scope = backend.enter(&ctx, viewport, 4);
        let witness = eframe::miv_test_script_window_witness::active().unwrap();
        let identity = detached_identity(viewport, witness.token());
        let prepared = prepared_step(
            &identity,
            1.0,
            SyntheticPointerStepKind::Move {
                held_before: egui::pos2(20.0, 30.0),
                point: egui::pos2(40.0, 30.0),
            },
        );
        let delivered_input = egui::RawInput {
            viewport_id: viewport,
            time: Some(1.005),
            ..Default::default()
        };
        ctx.begin_pass(egui::RawInput {
            viewport_id: viewport,
            time: Some(1.006),
            ..Default::default()
        });
        let show = enter_show(Some(ShowOwner {
            identity: identity.clone(),
            items_generation: 7,
        }));
        begin_pass(&ctx, 7, 2, "page-2".to_string(), mode_proof());
        record_delivery_proof(&prepared, &delivered_input);
        finish_pass(&ctx);
        let mut output = show.finish();
        let error = output.take_joined_region_frame(&identity, 7).unwrap_err();
        let _ = ctx.end_pass();
        assert!(error.contains("delivery did not join"), "{error}");
    }

    fn assert_down_provenance(
        callback_page: usize,
        callback_item: &str,
        callback_mode: FullscreenModeProof,
        expected_receipt: bool,
    ) {
        let ctx = egui::Context::default();
        let viewport = egui::ViewportId::ROOT;
        let backend = eframe::miv_test_script_window_witness::WindowWitnessFixture::new();
        let _backend_scope = backend.enter(&ctx, viewport, 4);
        let witness = eframe::miv_test_script_window_witness::active().unwrap();
        let identity = detached_identity(viewport, witness.token());
        let prepared = prepared_step(
            &identity,
            1.0,
            SyntheticPointerStepKind::Down {
                point: egui::pos2(20.0, 30.0),
            },
        );
        let rect = egui::Rect::from_min_max(egui::pos2(10.0, 20.0), egui::pos2(110.0, 40.0));
        ctx.begin_pass(egui::RawInput {
            viewport_id: viewport,
            time: Some(1.0),
            ..Default::default()
        });
        egui::CentralPanel::default().show(&ctx, |ui| {
            let _ = ui.interact(
                rect,
                egui::Id::new("pointer-track"),
                egui::Sense::click_and_drag(),
            );
        });
        let _ = ctx.end_pass();
        let child_input = egui::RawInput {
            viewport_id: viewport,
            time: Some(1.005),
            events: vec![
                egui::Event::PointerMoved(egui::pos2(20.0, 30.0)),
                egui::Event::PointerButton {
                    pos: egui::pos2(20.0, 30.0),
                    button: egui::PointerButton::Primary,
                    pressed: true,
                    modifiers: egui::Modifiers::NONE,
                },
            ],
            ..Default::default()
        };
        let show = enter_show(Some(ShowOwner {
            identity,
            items_generation: 7,
        }));
        ctx.begin_pass(child_input.clone());
        record_delivery_proof(&prepared, &child_input);
        begin_pass(
            &ctx,
            7,
            callback_page,
            callback_item.to_string(),
            callback_mode,
        );
        egui::CentralPanel::default().show(&ctx, |ui| {
            let response = ui.interact(
                rect,
                egui::Id::new("pointer-track"),
                egui::Sense::click_and_drag(),
            );
            assert!(
                response.is_pointer_button_down_on(),
                "test precondition: egui must hit the actual response"
            );
            record_region(RegionId::StillSeekTrack, &response, rect, None);
            observe_region_handler(&response, RegionId::StillSeekTrack, None);
        });
        finish_pass(&ctx);
        let output = show.finish();
        let _ = ctx.end_pass();
        assert!(matches!(output.delivery, DeliveryAccumulator::Exact(_)));
        assert_eq!(
            matches!(output.receipt, ReceiptAccumulator::Valid(_)),
            expected_receipt,
            "Down handler receipt did not match the expected press-provenance result"
        );
    }

    #[test]
    fn down_handler_accepts_exact_press_provenance() {
        assert_down_provenance(2, "page-2", mode_proof(), true);
    }

    #[test]
    fn down_handler_rejects_page_change_after_fresh_barrier() {
        assert_down_provenance(3, "page-3", mode_proof(), false);
    }

    #[test]
    fn down_handler_rejects_rtl_change_after_fresh_barrier() {
        let mut changed = mode_proof();
        changed.strip_rtl = true;
        assert_down_provenance(2, "page-2", changed, false);
    }

    #[test]
    fn geometry_token_survives_repaint_but_never_page_change_or_invalidation() {
        let owner = detached_identity(egui::ViewportId::from_hash_of("geometry-owner"), 5);
        let rect = egui::Rect::from_min_max(egui::pos2(1.0, 2.0), egui::pos2(101.0, 22.0));
        let mut catalog = RegionCatalog::default();

        catalog.publish(region_frame(&owner, 2, "page-2", rect));
        let first = catalog.frames[0].regions[0].geometry_token;
        let first_revision = catalog.frames[0].revision;

        catalog.publish(region_frame(&owner, 2, "page-2", rect));
        assert_eq!(catalog.frames[0].regions[0].geometry_token, first);
        assert!(catalog.frames[0].revision > first_revision);

        catalog.publish(region_frame(&owner, 3, "page-3", rect));
        let changed_page = catalog.frames[0].regions[0].geometry_token;
        assert_ne!(changed_page, first);

        let mut changed_mode_frame = region_frame(&owner, 3, "page-3", rect);
        changed_mode_frame.mode.strip_rtl = true;
        catalog.publish(changed_mode_frame);
        let changed_mode = catalog.frames[0].regions[0].geometry_token;
        assert_ne!(changed_mode, changed_page);

        catalog.invalidate(Some(&owner));
        catalog.publish(region_frame(&owner, 2, "page-2", rect));
        let republished = catalog.frames[0].regions[0].geometry_token;
        assert_ne!(republished, first);
        assert_ne!(republished, changed_page);
        assert_ne!(republished, changed_mode);
    }

    #[test]
    fn held_region_uses_exact_owner_and_generation_without_requiring_the_press_page() {
        let owner = detached_identity(egui::ViewportId::from_hash_of("held-page-change"), 51);
        let sibling = detached_identity(egui::ViewportId::from_hash_of("held-sibling"), 52);
        let rect = egui::Rect::from_min_max(egui::pos2(10.0, 20.0), egui::pos2(110.0, 40.0));
        let mut catalog = RegionCatalog::default();
        catalog.publish(region_frame(&owner, 2, "page-2", rect));
        catalog.publish(region_frame(&sibling, 9, "page-9", rect));

        let latch = prepared_step(
            &owner,
            1.0,
            SyntheticPointerStepKind::Move {
                held_before: egui::pos2(20.0, 30.0),
                point: egui::pos2(80.0, 30.0),
            },
        )
        .step
        .latch;
        let (frame, named) = catalog
            .held_region(&owner, 7, RegionId::StillSeekTrack)
            .expect("the held owner must retain its latest pre-navigation geometry");
        assert_eq!(frame.page_index, 2);
        assert!(held_region_coordinate_frame_matches(&latch, frame, named));
        assert!(
            catalog
                .held_region(&owner, 8, RegionId::StillSeekTrack)
                .is_none(),
            "another catalog generation must never reuse held geometry"
        );
        assert!(
            catalog
                .held_region(&sibling, 7, RegionId::StillSeekTrack)
                .is_some(),
            "the sibling remains independently queryable"
        );

        catalog.publish(region_frame(&owner, 3, "page-3", rect));
        let (frame, named) = catalog
            .held_region(&owner, 7, RegionId::StillSeekTrack)
            .expect("track navigation keeps the exact held owner current");
        assert!(
            held_region_coordinate_frame_matches(&latch, frame, named),
            "a track page change alone must not require an intervening paint barrier"
        );

        let mut navigated = region_frame(&owner, 3, "page-3", rect);
        navigated.mode.strip_rtl = true;
        catalog.publish(navigated);
        let (frame, named) = catalog
            .held_region(&owner, 7, RegionId::StillSeekTrack)
            .expect("the navigated owner remains present");
        assert!(
            !held_region_coordinate_frame_matches(&latch, frame, named),
            "a callback mode change must still reject the held step"
        );

        let mut resized = region_frame(&owner, 3, "page-3", rect);
        resized.regions[0].coordinate_frame = rect.translate(egui::vec2(1.0, 0.0));
        catalog.publish(resized);
        let (frame, named) = catalog
            .held_region(&owner, 7, RegionId::StillSeekTrack)
            .expect("the resized owner remains present");
        assert!(
            !held_region_coordinate_frame_matches(&latch, frame, named),
            "a coordinate-frame resize must reject the held step"
        );

        let mut scaled = region_frame(&owner, 3, "page-3", rect);
        scaled.pixels_per_point = 2.0;
        catalog.publish(scaled);
        let (frame, named) = catalog
            .held_region(&owner, 7, RegionId::StillSeekTrack)
            .expect("the rescaled owner remains present");
        assert!(
            !held_region_coordinate_frame_matches(&latch, frame, named),
            "a pixels-per-point change must reject the held step"
        );
    }

    #[test]
    fn held_freshness_allows_immediate_track_up_after_page_navigation_before_next_paint() {
        let ctx = egui::Context::default();
        let viewport = egui::ViewportId::from_hash_of("held-immediate-track-up");
        let backend = eframe::miv_test_script_window_witness::WindowWitnessFixture::new();
        let _backend_scope = backend.enter(&ctx, viewport, 4);
        let witness = eframe::miv_test_script_window_witness::active().unwrap();
        let owner = detached_identity(viewport, witness.token());
        let current = current_window_snapshot(&owner, 3, "page-3");
        let snapshot = Arc::new(RwLock::new(super::super::TestScriptSnapshot {
            windows: vec![current.clone()],
            ..super::super::TestScriptSnapshot::default()
        }));
        let catalog = new_shared_catalog();
        catalog.write().unwrap().publish(region_frame(
            &owner,
            2,
            "page-2",
            egui::Rect::from_min_max(egui::pos2(10.0, 20.0), egui::pos2(110.0, 40.0)),
        ));
        let (tx, rx) = std::sync::mpsc::channel();
        let interrupt = Arc::new(super::super::InterruptState::default());
        let bridge = RunnerBridge {
            tx,
            snapshot: Arc::clone(&snapshot),
            interrupt: Arc::clone(&interrupt),
            wake: Arc::new(|| {}),
            next_hold_id: Arc::new(std::sync::atomic::AtomicU64::new(0)),
            action_selection: Arc::new(std::sync::Mutex::new(
                super::super::TestScriptActionSelection::Targeted(owner.clone()),
            )),
            pointer_regions: Arc::clone(&catalog),
        };
        let latch = prepared_step(
            &owner,
            1.0,
            SyntheticPointerStepKind::Up {
                held_before: egui::pos2(20.0, 30.0),
                final_point: egui::pos2(80.0, 30.0),
            },
        )
        .step
        .latch;
        let worker = std::thread::spawn(move || {
            validate_held_owner_fresh(
                &bridge,
                &catalog,
                &latch,
                Instant::now() + Duration::from_secs(1),
            )
        });

        let (_unused_tx, unused_rx) = std::sync::mpsc::channel();
        let mut runtime =
            super::super::UiRuntime::new(unused_rx, snapshot, interrupt, new_shared_catalog());
        runtime.publish_windows(vec![current]).unwrap();
        match rx.recv_timeout(Duration::from_secs(1)).unwrap() {
            super::super::UiCommand::ValidateSelectedOwner {
                expected_identity,
                reply,
            } => reply
                .send(runtime.validate_selected_owner(&expected_identity))
                .unwrap(),
            command => panic!("unexpected command: {command:?}"),
        }
        worker
            .join()
            .unwrap()
            .expect("page-only navigation must allow the immediate held Up before repaint");
    }

    #[test]
    fn held_gesture_keeps_press_coordinate_frame_while_strip_response_rect_moves() {
        let owner = detached_identity(egui::ViewportId::from_hash_of("moving-strip-row"), 6);
        let press_rect = egui::Rect::from_min_max(egui::pos2(20.0, 70.0), egui::pos2(180.0, 96.0));
        let coordinate_frame =
            egui::Rect::from_min_max(egui::pos2(10.0, 60.0), egui::pos2(210.0, 104.0));
        let mut frame = region_frame(&owner, 2, "page-2", press_rect);
        frame.regions[0].id = RegionId::StillSeekStripRow;
        frame.regions[0].widget_id = egui::Id::new("pointer-strip");
        frame.regions[0].coordinate_frame = coordinate_frame;
        let mut latch = prepared_step(
            &owner,
            1.0,
            SyntheticPointerStepKind::Down {
                point: press_rect.center(),
            },
        )
        .step
        .latch;
        latch.region = SyntheticPointerRegion::StillSeekStripRow;
        latch.widget_id = egui::Id::new("pointer-strip");
        latch.press_rect = press_rect;
        latch.coordinate_frame = coordinate_frame;

        frame.regions[0].rect =
            egui::Rect::from_min_max(egui::pos2(35.0, 70.0), egui::pos2(195.0, 96.0));
        assert!(held_region_coordinate_frame_matches(
            &latch,
            &frame,
            &frame.regions[0]
        ));

        frame.regions[0].coordinate_frame = coordinate_frame.translate(egui::vec2(1.0, 0.0));
        assert!(!held_region_coordinate_frame_matches(
            &latch,
            &frame,
            &frame.regions[0]
        ));
    }

    #[test]
    fn nested_none_scope_restores_parent() {
        let owner = ShowOwner {
            identity: TestScriptWindowIdentity::Detached {
                window_id: 1,
                context_serial: 2,
                viewport_id: egui::ViewportId::from_hash_of("draft-child"),
                host_incarnation: 3,
                hwnd: 4,
                backend_token: 5,
            },
            items_generation: 6,
        };
        let outer = enter_show(Some(owner.clone()));
        assert_eq!(active_show_owner(), Some(owner.clone()));
        {
            let _inner = enter_show(None);
            assert_eq!(active_show_owner(), None);
        }
        assert_eq!(active_show_owner(), Some(owner));
        drop(outer);
        assert_eq!(active_show_owner(), None);
    }
}
