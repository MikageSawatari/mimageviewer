//! Per-viewport Win32 key edge queue.
//!
//! egui flattens some physical keys (notably numpad digits and JIS-specific
//! keys).  The keymap still lets egui handle text/IME normally, but shortcut
//! matching reads key-down edges from this queue when the target viewport's
//! HWND subclass is installed. Each edge is stamped with its source HWND and
//! registered `ViewportId` before it enters the queue.

use std::collections::VecDeque;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

#[cfg(all(windows, any(test, feature = "test-script")))]
use std::sync::mpsc;

use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
use windows::Win32::UI::Input::KeyboardAndMouse::{GetKeyState, VK_CONTROL, VK_MENU, VK_SHIFT};
use windows::Win32::UI::Shell::{DefSubclassProc, SetWindowSubclass};
use windows::Win32::UI::WindowsAndMessaging::{
    WM_KEYDOWN, WM_KEYUP, WM_KILLFOCUS, WM_NCDESTROY, WM_SYSKEYDOWN, WM_SYSKEYUP,
};

const MAIN_KEY_INPUT_SUBCLASS_ID: usize = 0x6D69_6B31; // "mik1"
const MAX_PENDING_EVENTS: usize = 256;
const MAX_LOGGED_UNREGISTERED_HWND: usize = 16;
const MAX_SYNTHETIC_INPUT_ISSUES: usize = 64;
const DEFAULT_REPEAT_DELAY: Duration = Duration::from_millis(250);
const DEFAULT_REPEAT_INTERVAL: Duration = Duration::from_nanos(33_333_333);

/// A physical Windows key slot used by level-sensitive input consumers.
///
/// `extended` is required because main Enter and numpad Enter share
/// `VK_RETURN`. The initial synthetic-input surface deliberately exposes only
/// [`SyntheticNavigationKey`]; characters, JIS punctuation, numpad keys,
/// clipboard operations, and IME input are outside this stage.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PhysicalKeySlot {
    pub vk: u32,
    pub extended: bool,
}

impl PhysicalKeySlot {
    pub const fn new(vk: u32, extended: bool) -> Self {
        Self { vk, extended }
    }
}

/// Navigation keys supported by the initial synthetic-input timeline.
///
/// Printable keys, JIS symbols, numpad-specific keys, clipboard shortcuts,
/// text events, and IME events are intentionally not representable here.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SyntheticNavigationKey {
    Right,
    Left,
    Up,
    Down,
    PageUp,
    PageDown,
    Home,
    End,
    Enter,
    Escape,
}

impl SyntheticNavigationKey {
    #[cfg(feature = "test-script")]
    const fn as_str(self) -> &'static str {
        match self {
            Self::Right => "Right",
            Self::Left => "Left",
            Self::Up => "Up",
            Self::Down => "Down",
            Self::PageUp => "PageUp",
            Self::PageDown => "PageDown",
            Self::Home => "Home",
            Self::End => "End",
            Self::Enter => "Enter",
            Self::Escape => "Escape",
        }
    }

    const fn physical_slot(self) -> PhysicalKeySlot {
        match self {
            Self::Right => PhysicalKeySlot::new(0x27, true),
            Self::Left => PhysicalKeySlot::new(0x25, true),
            Self::Up => PhysicalKeySlot::new(0x26, true),
            Self::Down => PhysicalKeySlot::new(0x28, true),
            Self::PageUp => PhysicalKeySlot::new(0x21, true),
            Self::PageDown => PhysicalKeySlot::new(0x22, true),
            Self::Home => PhysicalKeySlot::new(0x24, true),
            Self::End => PhysicalKeySlot::new(0x23, true),
            Self::Enter => PhysicalKeySlot::new(0x0D, false),
            Self::Escape => PhysicalKeySlot::new(0x1B, false),
        }
    }

    const fn scan_code(self) -> u16 {
        match self {
            Self::Right => 0x4D,
            Self::Left => 0x4B,
            Self::Up => 0x48,
            Self::Down => 0x50,
            Self::PageUp => 0x49,
            Self::PageDown => 0x51,
            Self::Home => 0x47,
            Self::End => 0x4F,
            Self::Enter => 0x1C,
            Self::Escape => 0x01,
        }
    }

    const fn egui_key(self) -> egui::Key {
        match self {
            Self::Right => egui::Key::ArrowRight,
            Self::Left => egui::Key::ArrowLeft,
            Self::Up => egui::Key::ArrowUp,
            Self::Down => egui::Key::ArrowDown,
            Self::PageUp => egui::Key::PageUp,
            Self::PageDown => egui::Key::PageDown,
            Self::Home => egui::Key::Home,
            Self::End => egui::Key::End,
            Self::Enter => egui::Key::Enter,
            Self::Escape => egui::Key::Escape,
        }
    }
}

/// Generic modifier level attached to a synthetic navigation hold.
///
/// Left/right-specific modifier keys are not part of the initial navigation
/// API. On Windows `command` is derived from `ctrl`, while `mac_cmd` is always
/// false.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SyntheticModifiers {
    pub ctrl: bool,
    pub shift: bool,
    pub alt: bool,
}

impl SyntheticModifiers {
    const fn union(self, other: Self) -> Self {
        Self {
            ctrl: self.ctrl || other.ctrl,
            shift: self.shift || other.shift,
            alt: self.alt || other.alt,
        }
    }

    const fn to_egui(self) -> egui::Modifiers {
        egui::Modifiers {
            alt: self.alt,
            ctrl: self.ctrl,
            shift: self.shift,
            mac_cmd: false,
            command: self.ctrl,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SyntheticKeyCommandKind {
    Down {
        key: SyntheticNavigationKey,
        modifiers: SyntheticModifiers,
    },
    Up {
        key: SyntheticNavigationKey,
    },
    CancelAll,
}

/// A monotonic-clock command consumed by the synthetic input timeline.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SyntheticKeyCommand {
    pub at: Instant,
    pub kind: SyntheticKeyCommandKind,
    pub(crate) hold_id: Option<u64>,
}

impl SyntheticKeyCommand {
    pub const fn down(
        at: Instant,
        key: SyntheticNavigationKey,
        modifiers: SyntheticModifiers,
    ) -> Self {
        Self {
            at,
            kind: SyntheticKeyCommandKind::Down { key, modifiers },
            hold_id: None,
        }
    }

    pub const fn up(at: Instant, key: SyntheticNavigationKey) -> Self {
        Self {
            at,
            kind: SyntheticKeyCommandKind::Up { key },
            hold_id: None,
        }
    }

    pub const fn cancel_all(at: Instant) -> Self {
        Self {
            at,
            kind: SyntheticKeyCommandKind::CancelAll,
            hold_id: None,
        }
    }

    #[cfg(any(test, feature = "test-script"))]
    pub const fn with_hold_id(mut self, hold_id: u64) -> Self {
        self.hold_id = Some(hold_id);
        self
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SyntheticRoutingTarget {
    pub hwnd: u64,
    pub viewport: egui::ViewportId,
}

/// A typed routing result so a future script runner can wait or fail instead
/// of silently redirecting an unregistered foreground HWND to ROOT.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SyntheticRoutingTargetError {
    NoForegroundWindow,
    UnregisteredForegroundWindow { hwnd: u64 },
}

#[derive(Clone, Debug, PartialEq)]
pub enum SyntheticInputIssue {
    WaitingForRouting(SyntheticRoutingTargetError),
    WaitingForFocus(SyntheticRoutingTarget),
    FocusLost {
        viewport: egui::ViewportId,
    },
    TargetViewportNotRendered {
        viewport: egui::ViewportId,
        raw_input_time: Option<f64>,
        event_count: usize,
    },
    #[cfg(all(windows, any(test, feature = "test-script")))]
    PointerOwnerMismatch {
        handle: SyntheticPointerCancelHandle,
        detail: String,
    },
    #[cfg(all(windows, any(test, feature = "test-script")))]
    PointerPhysicalInputMixed {
        handle: SyntheticPointerCancelHandle,
    },
    #[cfg(all(windows, any(test, feature = "test-script")))]
    PointerViewportNotRendered {
        handle: SyntheticPointerCancelHandle,
        viewport: egui::ViewportId,
    },
    #[cfg(all(windows, any(test, feature = "test-script")))]
    MissingPointerShowTail {
        handle: SyntheticPointerCancelHandle,
        viewport: egui::ViewportId,
    },
}

#[cfg(all(windows, any(test, feature = "test-script")))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum SyntheticPointerRegion {
    StillSeekStripRow,
    StillSeekTrack,
}

#[cfg(all(windows, any(test, feature = "test-script")))]
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct SyntheticPointerOwner {
    pub(crate) identity: crate::test_script::TestScriptWindowIdentity,
    pub(crate) items_generation: u64,
}

#[cfg(all(windows, any(test, feature = "test-script")))]
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct SyntheticPointerModeSignature {
    pub(crate) spread_mode: String,
    pub(crate) reading_flow: String,
    pub(crate) strip_rtl: bool,
    pub(crate) seek_bar_rtl: bool,
    pub(crate) strip_visible: bool,
    pub(crate) strip_locked: bool,
    pub(crate) bar_locked: bool,
}

#[cfg(all(windows, any(test, feature = "test-script")))]
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct SyntheticPointerLatch {
    pub(crate) transaction_id: u64,
    pub(crate) owner: SyntheticPointerOwner,
    pub(crate) region: SyntheticPointerRegion,
    pub(crate) mode: SyntheticPointerModeSignature,
    pub(crate) region_geometry_token: u64,
    pub(crate) press_page_index: usize,
    pub(crate) press_item_identity: String,
    pub(crate) widget_id: egui::Id,
    pub(crate) press_rect: egui::Rect,
    pub(crate) coordinate_frame: egui::Rect,
    pub(crate) press_pixels_per_point: f32,
    pub(crate) press_point: egui::Pos2,
}

#[cfg(all(windows, any(test, feature = "test-script")))]
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct SyntheticPointerDownRequest {
    pub(crate) owner: SyntheticPointerOwner,
    pub(crate) region: SyntheticPointerRegion,
    pub(crate) mode: SyntheticPointerModeSignature,
    pub(crate) region_geometry_token: u64,
    pub(crate) press_page_index: usize,
    pub(crate) press_item_identity: String,
    pub(crate) widget_id: egui::Id,
    pub(crate) press_rect: egui::Rect,
    pub(crate) coordinate_frame: egui::Rect,
    pub(crate) press_pixels_per_point: f32,
    pub(crate) press_point: egui::Pos2,
}

#[cfg(all(windows, any(test, feature = "test-script")))]
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SyntheticPointerCancelHandle {
    pub(crate) transaction_id: u64,
    pub(crate) step_id: u64,
    pub(crate) owner: SyntheticPointerOwner,
}

#[cfg(all(windows, any(test, feature = "test-script")))]
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct SyntheticPointerHeldSnapshot {
    pub(crate) cancel_handle: SyntheticPointerCancelHandle,
    pub(crate) latch: SyntheticPointerLatch,
}

#[cfg(all(windows, any(test, feature = "test-script")))]
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum SyntheticPointerStepKind {
    Down {
        point: egui::Pos2,
    },
    Move {
        held_before: egui::Pos2,
        point: egui::Pos2,
    },
    Up {
        held_before: egui::Pos2,
        final_point: egui::Pos2,
    },
    CleanupUp {
        final_point: egui::Pos2,
    },
}

#[cfg(all(windows, any(test, feature = "test-script")))]
impl SyntheticPointerStepKind {
    fn point_after_delivery(self) -> egui::Pos2 {
        match self {
            Self::Down { point } | Self::Move { point, .. } => point,
            Self::Up { final_point, .. } | Self::CleanupUp { final_point } => final_point,
        }
    }

    fn held_before(self) -> Option<egui::Pos2> {
        match self {
            Self::Move { held_before, .. } | Self::Up { held_before, .. } => Some(held_before),
            Self::Down { .. } | Self::CleanupUp { .. } => None,
        }
    }
}

#[cfg(all(windows, any(test, feature = "test-script")))]
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum SyntheticPointerHandlerEffect {
    Pressed,
    StripCenter(usize),
    TrackTarget(Option<usize>),
    ReleasedStripCenter(usize),
    ReleasedTrackTarget(Option<usize>),
}

#[cfg(all(windows, any(test, feature = "test-script")))]
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct SyntheticPointerCompletion {
    pub(crate) step_id: u64,
    pub(crate) effect: SyntheticPointerHandlerEffect,
    pub(crate) page_before: usize,
    pub(crate) page_after: usize,
    /// Catalog revision published for the handler's own show. A subsequent paint proof must have
    /// a strictly greater revision.
    pub(crate) after_revision: u64,
}

#[cfg(all(windows, any(test, feature = "test-script")))]
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum SyntheticPointerHandlerProof {
    Success(SyntheticPointerCompletion),
    Missing,
    Contradiction(String),
}

#[cfg(all(windows, any(test, feature = "test-script")))]
#[derive(Clone, Debug)]
pub(crate) struct SyntheticPointerShowTail {
    pub(crate) step: SyntheticPointerStep,
    pub(crate) primary_down: bool,
    pub(crate) handler: SyntheticPointerHandlerProof,
}

#[cfg(all(windows, any(test, feature = "test-script")))]
#[derive(Clone, Debug, PartialEq, Eq)]
enum SyntheticPointerFailure {
    Busy,
    NotHeld,
    WrongStep,
    ContradictoryReceipt,
    MissingHandlerProof,
    PointerLevelMismatch,
    Cancelled,
    TargetLostDuringCleanup,
    CleanupDidNotRelease,
    MissingShowTail,
}

#[cfg(all(windows, any(test, feature = "test-script")))]
impl SyntheticPointerFailure {
    fn describe(&self) -> &'static str {
        match self {
            Self::Busy => "a synthetic pointer transaction is already active",
            Self::NotHeld => "synthetic pointer is not held",
            Self::WrongStep => "synthetic pointer step did not match the transaction phase",
            Self::ContradictoryReceipt => "synthetic pointer handler proof was contradictory",
            Self::MissingHandlerProof => "synthetic pointer reached no supported widget",
            Self::PointerLevelMismatch => "synthetic pointer primary level did not match the step",
            Self::Cancelled => "synthetic pointer input was cancelled",
            Self::TargetLostDuringCleanup => {
                "synthetic pointer target was lost before cleanup completed"
            }
            Self::CleanupDidNotRelease => "synthetic pointer cleanup did not release primary",
            Self::MissingShowTail => "synthetic pointer show ended without its callback tail",
        }
    }
}

#[cfg(all(windows, any(test, feature = "test-script")))]
#[derive(Clone, Debug)]
pub(crate) struct SyntheticPointerStep {
    pub(crate) step_id: u64,
    pub(crate) latch: SyntheticPointerLatch,
    pub(crate) kind: SyntheticPointerStepKind,
    completion: Option<mpsc::SyncSender<Result<SyntheticPointerCompletion, String>>>,
}

#[cfg(all(windows, any(test, feature = "test-script")))]
impl SyntheticPointerStep {
    pub(crate) fn same_payload(&self, other: &Self) -> bool {
        self.step_id == other.step_id && self.latch == other.latch && self.kind == other.kind
    }

    fn complete(&mut self, result: Result<SyntheticPointerCompletion, String>) {
        if let Some(reply) = self.completion.take() {
            let _ = reply.send(result);
        }
    }

    fn cancel_handle(&self) -> SyntheticPointerCancelHandle {
        SyntheticPointerCancelHandle {
            transaction_id: self.latch.transaction_id,
            step_id: self.step_id,
            owner: self.latch.owner.clone(),
        }
    }

    fn egui_events(&self) -> Vec<egui::Event> {
        let (point, pressed) = match self.kind {
            SyntheticPointerStepKind::Down { point } => (point, Some(true)),
            SyntheticPointerStepKind::Move { point, .. } => (point, None),
            SyntheticPointerStepKind::Up { final_point, .. }
            | SyntheticPointerStepKind::CleanupUp { final_point } => (final_point, Some(false)),
        };
        let mut events = vec![egui::Event::PointerMoved(point)];
        if let Some(pressed) = pressed {
            events.push(egui::Event::PointerButton {
                pos: point,
                button: egui::PointerButton::Primary,
                pressed,
                modifiers: egui::Modifiers::NONE,
            });
        }
        events
    }
}

#[cfg(all(windows, any(test, feature = "test-script")))]
#[derive(Clone, Debug)]
pub(crate) struct PreparedSyntheticPointerStep {
    pub(crate) raw_frame: u64,
    pub(crate) raw_time_bits: u64,
    pub(crate) step: SyntheticPointerStep,
}

#[cfg(all(windows, any(test, feature = "test-script")))]
impl PreparedSyntheticPointerStep {
    pub(crate) fn same_payload(&self, other: &Self) -> bool {
        self.raw_frame == other.raw_frame
            && self.raw_time_bits == other.raw_time_bits
            && self.step.same_payload(&other.step)
    }
}

#[cfg(all(windows, any(test, feature = "test-script")))]
impl PartialEq for PreparedSyntheticPointerStep {
    fn eq(&self, other: &Self) -> bool {
        self.same_payload(other)
    }
}

#[cfg(all(windows, any(test, feature = "test-script")))]
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum SyntheticPointerRequestedHeldStep {
    Move { point: egui::Pos2 },
    Up { final_point: egui::Pos2 },
}

#[cfg(all(windows, any(test, feature = "test-script")))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum SyntheticPointerHeldPhase {
    Move,
    Up,
}

#[cfg(all(windows, any(test, feature = "test-script")))]
#[derive(Clone, Debug)]
enum SyntheticPointerCancelPhase {
    Queued(SyntheticPointerStep),
    Prepared(SyntheticPointerStep),
    AwaitingOriginalTail(SyntheticPointerStep),
    DeliveredCleanupAwaitingTail(SyntheticPointerStep),
}

#[cfg(all(windows, any(test, feature = "test-script")))]
#[derive(Clone, Debug)]
enum SyntheticPointerTransaction {
    Idle,
    Queued(SyntheticPointerStep),
    Prepared(SyntheticPointerStep),
    DeliveredAwaitingTail(SyntheticPointerStep),
    Held {
        latch: SyntheticPointerLatch,
        last_point: egui::Pos2,
        next_step_id: u64,
    },
    Cancelling(SyntheticPointerCancelPhase),
    TerminalFailure {
        handle: SyntheticPointerCancelHandle,
        failure: SyntheticPointerFailure,
    },
}

#[cfg(all(windows, any(test, feature = "test-script")))]
#[derive(Clone, Debug, PartialEq, Eq)]
enum PreparedPointerDeliveryDisposition {
    Delivered,
    Rejected(SyntheticPointerCancelHandle),
    Stale,
}

#[cfg(all(windows, any(test, feature = "test-script")))]
impl Default for SyntheticPointerTransaction {
    fn default() -> Self {
        Self::Idle
    }
}

#[cfg(all(windows, any(test, feature = "test-script")))]
impl SyntheticPointerTransaction {
    fn is_idle(&self) -> bool {
        matches!(self, Self::Idle)
    }

    fn is_terminal_failure(&self, expected: &SyntheticPointerCancelHandle) -> bool {
        matches!(self, Self::TerminalFailure { handle, .. } if handle == expected)
    }

    fn owner(&self) -> Option<&SyntheticPointerOwner> {
        match self {
            Self::Queued(step)
            | Self::Prepared(step)
            | Self::DeliveredAwaitingTail(step)
            | Self::Cancelling(SyntheticPointerCancelPhase::Queued(step))
            | Self::Cancelling(SyntheticPointerCancelPhase::Prepared(step))
            | Self::Cancelling(SyntheticPointerCancelPhase::AwaitingOriginalTail(step))
            | Self::Cancelling(SyntheticPointerCancelPhase::DeliveredCleanupAwaitingTail(step)) => {
                Some(&step.latch.owner)
            }
            Self::Held { latch, .. } => Some(&latch.owner),
            Self::TerminalFailure { handle, .. } => Some(&handle.owner),
            Self::Idle => None,
        }
    }

    fn held_latch(&self) -> Option<&SyntheticPointerLatch> {
        match self {
            Self::Held { latch, .. } => Some(latch),
            _ => None,
        }
    }

    fn matches_cancel_handle(&self, handle: &SyntheticPointerCancelHandle) -> bool {
        let (latch, step_matches) = match self {
            Self::Queued(step) | Self::Prepared(step) | Self::DeliveredAwaitingTail(step) => {
                (&step.latch, step.step_id == handle.step_id)
            }
            Self::Held {
                latch,
                next_step_id,
                ..
            } => (latch, *next_step_id == handle.step_id.wrapping_add(1)),
            Self::Cancelling(SyntheticPointerCancelPhase::Queued(step))
            | Self::Cancelling(SyntheticPointerCancelPhase::Prepared(step))
            | Self::Cancelling(SyntheticPointerCancelPhase::AwaitingOriginalTail(step))
            | Self::Cancelling(SyntheticPointerCancelPhase::DeliveredCleanupAwaitingTail(step)) => {
                (&step.latch, true)
            }
            Self::Idle | Self::TerminalFailure { .. } => return false,
        };
        step_matches && latch.transaction_id == handle.transaction_id && latch.owner == handle.owner
    }

    fn cancel_handle(&mut self, handle: &SyntheticPointerCancelHandle) -> bool {
        if !self.matches_cancel_handle(handle) {
            return false;
        }
        self.cancel();
        true
    }

    fn held_snapshot(&self) -> Option<SyntheticPointerHeldSnapshot> {
        let Self::Held {
            latch,
            next_step_id,
            ..
        } = self
        else {
            return None;
        };
        Some(SyntheticPointerHeldSnapshot {
            cancel_handle: SyntheticPointerCancelHandle {
                transaction_id: latch.transaction_id,
                // A handle names the last completed step while Held. A subsequently queued held
                // step gets `next_step_id` and returns its own handle.
                step_id: next_step_id.wrapping_sub(1),
                owner: latch.owner.clone(),
            },
            latch: latch.clone(),
        })
    }

    fn fail_physical_input_mixed(
        &mut self,
        viewport: egui::ViewportId,
    ) -> Option<SyntheticPointerCancelHandle> {
        if self.owner()?.identity.viewport_id() != viewport {
            return None;
        }
        let (handle, pending) = match self {
            Self::Queued(step)
            | Self::Prepared(step)
            | Self::DeliveredAwaitingTail(step)
            | Self::Cancelling(SyntheticPointerCancelPhase::Queued(step))
            | Self::Cancelling(SyntheticPointerCancelPhase::Prepared(step))
            | Self::Cancelling(SyntheticPointerCancelPhase::AwaitingOriginalTail(step))
            | Self::Cancelling(SyntheticPointerCancelPhase::DeliveredCleanupAwaitingTail(step)) => {
                (step.cancel_handle(), Some(step))
            }
            Self::Held {
                latch,
                next_step_id,
                ..
            } => (
                SyntheticPointerCancelHandle {
                    transaction_id: latch.transaction_id,
                    step_id: *next_step_id,
                    owner: latch.owner.clone(),
                },
                None,
            ),
            Self::Idle | Self::TerminalFailure { .. } => return None,
        };
        if let Some(step) = pending {
            step.complete(Err(SyntheticPointerFailure::PointerLevelMismatch
                .describe()
                .to_string()));
        }
        *self = Self::TerminalFailure {
            handle: handle.clone(),
            failure: SyntheticPointerFailure::PointerLevelMismatch,
        };
        Some(handle)
    }

    fn fail_owner_lost(
        &mut self,
        owner: &SyntheticPointerOwner,
    ) -> Option<SyntheticPointerCancelHandle> {
        if self.owner() != Some(owner) {
            return None;
        }
        let (handle, pending) = match self {
            Self::Queued(step)
            | Self::Prepared(step)
            | Self::DeliveredAwaitingTail(step)
            | Self::Cancelling(SyntheticPointerCancelPhase::Queued(step))
            | Self::Cancelling(SyntheticPointerCancelPhase::Prepared(step))
            | Self::Cancelling(SyntheticPointerCancelPhase::AwaitingOriginalTail(step))
            | Self::Cancelling(SyntheticPointerCancelPhase::DeliveredCleanupAwaitingTail(step)) => {
                (step.cancel_handle(), Some(step))
            }
            Self::Held {
                latch,
                next_step_id,
                ..
            } => (
                SyntheticPointerCancelHandle {
                    transaction_id: latch.transaction_id,
                    step_id: *next_step_id,
                    owner: latch.owner.clone(),
                },
                None,
            ),
            Self::Idle | Self::TerminalFailure { .. } => return None,
        };
        if let Some(step) = pending {
            step.complete(Err(SyntheticPointerFailure::TargetLostDuringCleanup
                .describe()
                .to_string()));
        }
        *self = Self::TerminalFailure {
            handle: handle.clone(),
            failure: SyntheticPointerFailure::TargetLostDuringCleanup,
        };
        Some(handle)
    }

    fn observable_delivered_step(&self) -> Option<&SyntheticPointerStep> {
        match self {
            Self::DeliveredAwaitingTail(step)
            | Self::Cancelling(SyntheticPointerCancelPhase::AwaitingOriginalTail(step))
            | Self::Cancelling(SyntheticPointerCancelPhase::DeliveredCleanupAwaitingTail(step)) => {
                Some(step)
            }
            _ => None,
        }
    }

    fn queue_down(&mut self, step: SyntheticPointerStep) -> Result<(), String> {
        if !matches!(step.kind, SyntheticPointerStepKind::Down { .. }) {
            return Err("first synthetic pointer step must be Down".to_string());
        }
        if !matches!(self, Self::Idle) {
            return Err("a synthetic pointer transaction is already active".to_string());
        }
        *self = Self::Queued(step);
        Ok(())
    }

    fn queue_held(
        &mut self,
        request: SyntheticPointerRequestedHeldStep,
        completion: mpsc::SyncSender<Result<SyntheticPointerCompletion, String>>,
    ) -> Result<u64, String> {
        let Self::Held {
            latch,
            last_point,
            next_step_id,
        } = self
        else {
            return Err("synthetic pointer is not held".to_string());
        };
        let step_id = *next_step_id;
        let kind = match request {
            SyntheticPointerRequestedHeldStep::Move { point } => SyntheticPointerStepKind::Move {
                held_before: *last_point,
                point,
            },
            SyntheticPointerRequestedHeldStep::Up { final_point } => SyntheticPointerStepKind::Up {
                held_before: *last_point,
                final_point,
            },
        };
        *self = Self::Queued(SyntheticPointerStep {
            step_id,
            latch: latch.clone(),
            kind,
            completion: Some(completion),
        });
        Ok(step_id)
    }

    fn prepare(&mut self) -> Option<SyntheticPointerStep> {
        match self {
            Self::Queued(step) => {
                let prepared = step.clone();
                *self = Self::Prepared(prepared.clone());
                Some(prepared)
            }
            Self::Cancelling(SyntheticPointerCancelPhase::Queued(step)) => {
                let prepared = step.clone();
                *self = Self::Cancelling(SyntheticPointerCancelPhase::Prepared(prepared.clone()));
                Some(prepared)
            }
            _ => None,
        }
    }

    fn mark_delivered(&mut self, delivered: &SyntheticPointerStep) -> bool {
        match self {
            Self::Prepared(step) if step.same_payload(delivered) => {
                *self = Self::DeliveredAwaitingTail(step.clone());
                true
            }
            Self::Cancelling(SyntheticPointerCancelPhase::Prepared(step))
                if step.same_payload(delivered) =>
            {
                *self = Self::Cancelling(
                    SyntheticPointerCancelPhase::DeliveredCleanupAwaitingTail(step.clone()),
                );
                true
            }
            _ => false,
        }
    }

    fn fail_delivered(
        &mut self,
        expected: &SyntheticPointerCancelHandle,
        failure: SyntheticPointerFailure,
        detail: String,
    ) -> bool {
        if self.is_terminal_failure(expected) {
            return true;
        }
        let step =
            match self {
                Self::DeliveredAwaitingTail(step)
                | Self::Cancelling(SyntheticPointerCancelPhase::AwaitingOriginalTail(step))
                | Self::Cancelling(SyntheticPointerCancelPhase::DeliveredCleanupAwaitingTail(
                    step,
                )) if step.cancel_handle() == *expected => step,
                _ => return false,
            };
        step.complete(Err(detail));
        *self = Self::TerminalFailure {
            handle: expected.clone(),
            failure,
        };
        true
    }

    fn queue_cleanup(&mut self, step: &SyntheticPointerStep, final_point: egui::Pos2) {
        *self = Self::Cancelling(SyntheticPointerCancelPhase::Queued(SyntheticPointerStep {
            step_id: step.step_id.wrapping_add(1),
            latch: step.latch.clone(),
            kind: SyntheticPointerStepKind::CleanupUp { final_point },
            completion: None,
        }));
    }

    fn finish_show(&mut self, tail: SyntheticPointerShowTail) -> Result<(), String> {
        let Self::DeliveredAwaitingTail(step) = self else {
            return Err(SyntheticPointerFailure::WrongStep.describe().to_string());
        };
        if !step.same_payload(&tail.step) {
            return Err(SyntheticPointerFailure::ContradictoryReceipt
                .describe()
                .to_string());
        }
        let mut step = step.clone();
        let completion = match tail.handler {
            SyntheticPointerHandlerProof::Success(completion)
                if completion.step_id == step.step_id =>
            {
                completion
            }
            SyntheticPointerHandlerProof::Success(_)
            | SyntheticPointerHandlerProof::Contradiction(_) => {
                let failure = SyntheticPointerFailure::ContradictoryReceipt;
                let detail = failure.describe().to_string();
                step.complete(Err(detail.clone()));
                if tail.primary_down {
                    let final_point = step.kind.point_after_delivery();
                    self.queue_cleanup(&step, final_point);
                } else {
                    *self = Self::Idle;
                }
                return Err(detail);
            }
            SyntheticPointerHandlerProof::Missing => {
                let failure = SyntheticPointerFailure::MissingHandlerProof;
                let detail = failure.describe().to_string();
                step.complete(Err(detail.clone()));
                if tail.primary_down {
                    let final_point = step.kind.point_after_delivery();
                    self.queue_cleanup(&step, final_point);
                } else {
                    *self = Self::Idle;
                }
                return Err(detail);
            }
        };

        match step.kind {
            SyntheticPointerStepKind::Down { .. } | SyntheticPointerStepKind::Move { .. }
                if tail.primary_down =>
            {
                let last_point = step.kind.point_after_delivery();
                let next_step_id = step.step_id.wrapping_add(1);
                let latch = step.latch.clone();
                step.complete(Ok(completion));
                *self = Self::Held {
                    latch,
                    last_point,
                    next_step_id,
                };
                Ok(())
            }
            SyntheticPointerStepKind::Up { .. } if !tail.primary_down => {
                step.complete(Ok(completion));
                *self = Self::Idle;
                Ok(())
            }
            SyntheticPointerStepKind::Down { .. } | SyntheticPointerStepKind::Move { .. } => {
                let failure = SyntheticPointerFailure::PointerLevelMismatch;
                let detail = failure.describe().to_string();
                step.complete(Err(detail.clone()));
                *self = Self::Idle;
                Err(detail)
            }
            SyntheticPointerStepKind::Up { .. } => {
                let failure = SyntheticPointerFailure::CleanupDidNotRelease;
                let detail = failure.describe().to_string();
                step.complete(Err(detail.clone()));
                let final_point = step.kind.point_after_delivery();
                self.queue_cleanup(&step, final_point);
                Err(detail)
            }
            SyntheticPointerStepKind::CleanupUp { .. } => {
                Err(SyntheticPointerFailure::WrongStep.describe().to_string())
            }
        }
    }

    fn finish_cancellation_show(&mut self, tail: SyntheticPointerShowTail) -> Result<(), String> {
        match self {
            Self::Cancelling(SyntheticPointerCancelPhase::AwaitingOriginalTail(step)) => {
                if !step.same_payload(&tail.step) {
                    return Err(SyntheticPointerFailure::ContradictoryReceipt
                        .describe()
                        .to_string());
                }
                let step = step.clone();
                if tail.primary_down {
                    let final_point = step.kind.point_after_delivery();
                    self.queue_cleanup(&step, final_point);
                } else {
                    *self = Self::Idle;
                }
                Ok(())
            }
            Self::Cancelling(SyntheticPointerCancelPhase::DeliveredCleanupAwaitingTail(step)) => {
                if !step.same_payload(&tail.step)
                    || !matches!(step.kind, SyntheticPointerStepKind::CleanupUp { .. })
                {
                    return Err(SyntheticPointerFailure::ContradictoryReceipt
                        .describe()
                        .to_string());
                }
                if tail.primary_down {
                    let failure = SyntheticPointerFailure::CleanupDidNotRelease;
                    let detail = failure.describe().to_string();
                    *self = Self::TerminalFailure {
                        handle: step.cancel_handle(),
                        failure,
                    };
                    Err(detail)
                } else {
                    *self = Self::Idle;
                    Ok(())
                }
            }
            _ => Err(SyntheticPointerFailure::WrongStep.describe().to_string()),
        }
    }

    fn finish_delivered_show(&mut self, tail: SyntheticPointerShowTail) -> Result<(), String> {
        match self {
            Self::DeliveredAwaitingTail(_) => self.finish_show(tail),
            Self::Cancelling(SyntheticPointerCancelPhase::AwaitingOriginalTail(_))
            | Self::Cancelling(SyntheticPointerCancelPhase::DeliveredCleanupAwaitingTail(_)) => {
                self.finish_cancellation_show(tail)
            }
            _ => Err(SyntheticPointerFailure::WrongStep.describe().to_string()),
        }
    }

    fn cancel(&mut self) {
        match self {
            Self::Idle | Self::Cancelling(_) | Self::TerminalFailure { .. } => {}
            Self::Queued(step) | Self::Prepared(step)
                if matches!(step.kind, SyntheticPointerStepKind::Down { .. }) =>
            {
                step.complete(Err("synthetic pointer input was cancelled".to_string()));
                *self = Self::Idle;
            }
            Self::Queued(step) | Self::Prepared(step) => {
                step.complete(Err("synthetic pointer input was cancelled".to_string()));
                let Some(point) = step.kind.held_before() else {
                    *self = Self::Idle;
                    return;
                };
                let cleanup = SyntheticPointerStep {
                    step_id: step.step_id.wrapping_add(1),
                    latch: step.latch.clone(),
                    kind: SyntheticPointerStepKind::CleanupUp { final_point: point },
                    completion: None,
                };
                *self = Self::Cancelling(SyntheticPointerCancelPhase::Queued(cleanup));
            }
            Self::DeliveredAwaitingTail(step) => {
                step.complete(Err("synthetic pointer input was cancelled".to_string()));
                *self = Self::Cancelling(SyntheticPointerCancelPhase::AwaitingOriginalTail(
                    step.clone(),
                ));
            }
            Self::Held {
                latch,
                last_point,
                next_step_id,
            } => {
                let cleanup = SyntheticPointerStep {
                    step_id: *next_step_id,
                    latch: latch.clone(),
                    kind: SyntheticPointerStepKind::CleanupUp {
                        final_point: *last_point,
                    },
                    completion: None,
                };
                *self = Self::Cancelling(SyntheticPointerCancelPhase::Queued(cleanup));
            }
        }
    }

    fn fail_missing_show_tail(&mut self, expected: &SyntheticPointerStep) -> bool {
        let matches = self
            .observable_delivered_step()
            .is_some_and(|step| step.same_payload(expected));
        if !matches {
            return false;
        }
        if let Some(step) = match self {
            Self::DeliveredAwaitingTail(step)
            | Self::Cancelling(SyntheticPointerCancelPhase::AwaitingOriginalTail(step))
            | Self::Cancelling(SyntheticPointerCancelPhase::DeliveredCleanupAwaitingTail(step)) => {
                Some(step)
            }
            _ => None,
        } {
            step.complete(Err(SyntheticPointerFailure::MissingShowTail
                .describe()
                .to_string()));
        }
        *self = Self::TerminalFailure {
            handle: expected.cancel_handle(),
            failure: SyntheticPointerFailure::MissingShowTail,
        };
        true
    }

    fn fail_undelivered(&mut self, expected: &SyntheticPointerStep) -> bool {
        match self {
            Self::Prepared(step) if step.same_payload(expected) => {
                let mut step = step.clone();
                step.complete(Err(
                    "synthetic pointer target viewport was not rendered".into()
                ));
                if matches!(step.kind, SyntheticPointerStepKind::Down { .. }) {
                    // An un-delivered Down cannot have changed egui's primary level.
                    *self = Self::Idle;
                } else {
                    *self = Self::TerminalFailure {
                        handle: step.cancel_handle(),
                        failure: SyntheticPointerFailure::TargetLostDuringCleanup,
                    };
                }
                true
            }
            Self::Cancelling(SyntheticPointerCancelPhase::Prepared(step))
                if step.same_payload(expected) =>
            {
                *self = Self::TerminalFailure {
                    handle: step.cancel_handle(),
                    failure: SyntheticPointerFailure::TargetLostDuringCleanup,
                };
                true
            }
            _ => false,
        }
    }

    fn acknowledge_terminal_failure(&mut self, expected: &SyntheticPointerCancelHandle) -> bool {
        if self.is_terminal_failure(expected) {
            *self = Self::Idle;
            true
        } else {
            false
        }
    }

    /// Classify and consume a prepared transport under the same timeline lock. A cache entry can
    /// survive waiter cancellation; only the exact still-prepared step may mutate the transaction
    /// or report an environment failure.
    fn resolve_prepared_delivery(
        &mut self,
        expected: &SyntheticPointerStep,
        reject: bool,
    ) -> PreparedPointerDeliveryDisposition {
        let is_current = matches!(
            self,
            Self::Prepared(step) if step.same_payload(expected)
        ) || matches!(
            self,
            Self::Cancelling(SyntheticPointerCancelPhase::Prepared(step))
                if step.same_payload(expected)
        );
        if !is_current {
            return PreparedPointerDeliveryDisposition::Stale;
        }
        if reject {
            let handle = expected.cancel_handle();
            let changed = self.fail_undelivered(expected);
            debug_assert!(changed);
            PreparedPointerDeliveryDisposition::Rejected(handle)
        } else {
            let changed = self.mark_delivered(expected);
            debug_assert!(changed);
            PreparedPointerDeliveryDisposition::Delivered
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct KeyEdge {
    pub source_hwnd: u64,
    pub source_viewport: egui::ViewportId,
    pub virtual_key: u32,
    pub scan_code: u16,
    pub extended: bool,
    pub pressed: bool,
    pub repeat: bool,
    pub ctrl: bool,
    pub shift: bool,
    pub alt: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct RawKeyEdge {
    virtual_key: u32,
    scan_code: u16,
    extended: bool,
    pressed: bool,
    repeat: bool,
    ctrl: bool,
    shift: bool,
    alt: bool,
}

impl RawKeyEdge {
    fn with_source(self, source_hwnd: u64, source_viewport: egui::ViewportId) -> KeyEdge {
        KeyEdge {
            source_hwnd,
            source_viewport,
            virtual_key: self.virtual_key,
            scan_code: self.scan_code,
            extended: self.extended,
            pressed: self.pressed,
            repeat: self.repeat,
            ctrl: self.ctrl,
            shift: self.shift,
            alt: self.alt,
        }
    }
}

#[derive(Default)]
struct ReturnKeyState {
    main_down: bool,
    numpad_down: bool,
}

impl ReturnKeyState {
    fn apply_edge(&mut self, edge: &KeyEdge) {
        const VK_RETURN: u32 = 0x0D;
        if edge.virtual_key != VK_RETURN {
            return;
        }
        if edge.extended {
            self.numpad_down = edge.pressed;
        } else {
            self.main_down = edge.pressed;
        }
    }

    fn is_down(&self, extended: bool) -> bool {
        if extended {
            self.numpad_down
        } else {
            self.main_down
        }
    }

    #[cfg(test)]
    fn clear(&mut self) {
        self.main_down = false;
        self.numpad_down = false;
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct InstalledHwnd {
    hwnd_raw: u64,
    viewport: egui::ViewportId,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum RegisterHwndResult {
    Inserted,
    AlreadyRegistered,
    ConflictingViewport(egui::ViewportId),
}

#[derive(Default)]
struct HwndViewportRegistry {
    entries: Vec<InstalledHwnd>,
}

impl HwndViewportRegistry {
    fn register(&mut self, hwnd_raw: u64, viewport: egui::ViewportId) -> RegisterHwndResult {
        if let Some(existing) = self.entries.iter().find(|entry| entry.hwnd_raw == hwnd_raw) {
            return if existing.viewport == viewport {
                RegisterHwndResult::AlreadyRegistered
            } else {
                RegisterHwndResult::ConflictingViewport(existing.viewport)
            };
        }
        self.entries.push(InstalledHwnd { hwnd_raw, viewport });
        RegisterHwndResult::Inserted
    }

    fn viewport_for_hwnd(&self, hwnd_raw: u64) -> Option<egui::ViewportId> {
        self.entries
            .iter()
            .find(|entry| entry.hwnd_raw == hwnd_raw)
            .map(|entry| entry.viewport)
    }

    fn contains_viewport(&self, viewport: egui::ViewportId) -> bool {
        self.entries.iter().any(|entry| entry.viewport == viewport)
    }

    fn remove(&mut self, hwnd_raw: u64) -> Option<egui::ViewportId> {
        let index = self
            .entries
            .iter()
            .position(|entry| entry.hwnd_raw == hwnd_raw)?;
        Some(self.entries.remove(index).viewport)
    }

    fn unique_viewports(&self) -> Vec<egui::ViewportId> {
        let mut viewports = Vec::new();
        for entry in &self.entries {
            if !viewports.contains(&entry.viewport) {
                viewports.push(entry.viewport);
            }
        }
        viewports
    }

    fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

struct ViewportReturnKeyState {
    viewport: egui::ViewportId,
    keys: ReturnKeyState,
}

#[derive(Default)]
struct ViewportReturnKeyStates {
    entries: Vec<ViewportReturnKeyState>,
}

impl ViewportReturnKeyStates {
    fn apply_edge(&mut self, viewport: egui::ViewportId, edge: &KeyEdge) {
        if let Some(entry) = self
            .entries
            .iter_mut()
            .find(|entry| entry.viewport == viewport)
        {
            entry.keys.apply_edge(edge);
            return;
        }
        let mut keys = ReturnKeyState::default();
        keys.apply_edge(edge);
        self.entries.push(ViewportReturnKeyState { viewport, keys });
    }

    fn is_down(&self, viewport: egui::ViewportId, extended: bool) -> bool {
        self.entries
            .iter()
            .find(|entry| entry.viewport == viewport)
            .is_some_and(|entry| entry.keys.is_down(extended))
    }

    fn clear_viewport(&mut self, viewport: egui::ViewportId) {
        self.entries.retain(|entry| entry.viewport != viewport);
    }

    fn clear(&mut self) {
        self.entries.clear();
    }
}

#[derive(Clone, Copy, Debug)]
struct QueuedSyntheticCommand {
    sequence: u64,
    command: SyntheticKeyCommand,
}

#[derive(Clone, Copy, Debug)]
struct HeldSyntheticKey {
    key: SyntheticNavigationKey,
    modifiers: SyntheticModifiers,
    target: SyntheticRoutingTarget,
    hold_id: Option<u64>,
    next_repeat_at: Instant,
    repeat_interval: Duration,
    order: u64,
    materialized_down_count: u64,
    materialized_repeat_count: u64,
}

#[derive(Clone, Copy, Debug)]
struct MaterializedSyntheticEvent {
    at: Instant,
    key: SyntheticNavigationKey,
    target: SyntheticRoutingTarget,
    hold_id: Option<u64>,
    pressed: bool,
    repeat: bool,
    modifiers: SyntheticModifiers,
}

impl MaterializedSyntheticEvent {
    fn key_edge(self) -> KeyEdge {
        let slot = self.key.physical_slot();
        KeyEdge {
            source_hwnd: self.target.hwnd,
            source_viewport: self.target.viewport,
            virtual_key: slot.vk,
            scan_code: self.key.scan_code(),
            extended: slot.extended,
            pressed: self.pressed,
            repeat: self.repeat,
            ctrl: self.modifiers.ctrl,
            shift: self.modifiers.shift,
            alt: self.modifiers.alt,
        }
    }

    fn egui_event(self) -> egui::Event {
        let key = self.key.egui_key();
        egui::Event::Key {
            key,
            physical_key: Some(key),
            pressed: self.pressed,
            // Match egui-winit: egui derives repeat from viewport-local
            // keys_down while processing the event stream.
            repeat: false,
            modifiers: self.modifiers.to_egui(),
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
enum SyntheticInputFact {
    HoldBegin {
        hold_id: u64,
        key: SyntheticNavigationKey,
        target: SyntheticRoutingTarget,
        repeat_delay: Duration,
        repeat_hz: f64,
    },
    HoldEnd {
        hold_id: u64,
        down_count: u64,
        repeat_count: u64,
        up_count: u64,
    },
    FrameInput {
        hold_id: u64,
        held: bool,
        edge_count: u64,
        // Number of repeat edges materialized in this outer input frame.
        // The analyzer uses values greater than one as accumulated-repeat
        // evidence; Down and Up are represented by edge_count instead.
        materialized_in_frame: u64,
    },
}

#[derive(Debug)]
struct SyntheticMaterialization {
    armed: bool,
    events: Vec<MaterializedSyntheticEvent>,
    facts: Vec<SyntheticInputFact>,
    final_modifiers: SyntheticModifiers,
    issue: Option<SyntheticInputIssue>,
}

impl SyntheticMaterialization {
    fn disarmed() -> Self {
        Self {
            armed: false,
            events: Vec::new(),
            facts: Vec::new(),
            final_modifiers: SyntheticModifiers::default(),
            issue: None,
        }
    }
}

struct SyntheticTimeline {
    armed: bool,
    commands: VecDeque<QueuedSyntheticCommand>,
    held: Vec<HeldSyntheticKey>,
    #[cfg_attr(not(test), allow(dead_code))]
    next_sequence: u64,
    repeat_delay: Duration,
    repeat_interval: Duration,
    #[cfg(all(windows, any(test, feature = "test-script")))]
    pointer: SyntheticPointerTransaction,
    #[cfg(all(windows, any(test, feature = "test-script")))]
    next_pointer_transaction_id: u64,
}

impl Default for SyntheticTimeline {
    fn default() -> Self {
        Self {
            armed: false,
            commands: VecDeque::new(),
            held: Vec::new(),
            next_sequence: 0,
            repeat_delay: DEFAULT_REPEAT_DELAY,
            repeat_interval: DEFAULT_REPEAT_INTERVAL,
            #[cfg(all(windows, any(test, feature = "test-script")))]
            pointer: SyntheticPointerTransaction::Idle,
            #[cfg(all(windows, any(test, feature = "test-script")))]
            next_pointer_transaction_id: 1,
        }
    }
}

impl SyntheticTimeline {
    #[cfg(any(test, feature = "test-script"))]
    fn arm(&mut self) {
        *self = Self::default();
        self.armed = true;
    }

    #[cfg(any(test, feature = "test-script"))]
    fn disarm(&mut self) {
        *self = Self::default();
    }

    #[cfg(any(test, feature = "test-script"))]
    fn set_repeat(&mut self, delay: Duration, hz: f64) -> bool {
        if !hz.is_finite() || hz <= 0.0 {
            return false;
        }
        let interval = Duration::from_secs_f64(1.0 / hz);
        if interval.is_zero() {
            return false;
        }
        self.repeat_delay = delay;
        self.repeat_interval = interval;
        true
    }

    #[cfg(any(test, feature = "test-script"))]
    fn is_idle(&self) -> bool {
        let key_idle = self.commands.is_empty() && self.held.is_empty();
        #[cfg(all(windows, any(test, feature = "test-script")))]
        let pointer_idle = self.pointer.is_idle();
        #[cfg(not(all(windows, any(test, feature = "test-script"))))]
        let pointer_idle = true;
        key_idle && pointer_idle
    }

    #[cfg(all(windows, any(test, feature = "test-script")))]
    fn queue_pointer_down(
        &mut self,
        request: SyntheticPointerDownRequest,
        completion: mpsc::SyncSender<Result<SyntheticPointerCompletion, String>>,
    ) -> Result<SyntheticPointerCancelHandle, String> {
        if !self.armed {
            return Err("synthetic input is not armed".to_string());
        }
        let transaction_id = self.next_pointer_transaction_id;
        self.next_pointer_transaction_id = self.next_pointer_transaction_id.wrapping_add(1).max(1);
        let step_id = transaction_id.wrapping_shl(32);
        let owner = request.owner.clone();
        self.pointer.queue_down(SyntheticPointerStep {
            step_id,
            latch: SyntheticPointerLatch {
                transaction_id,
                owner: request.owner,
                region: request.region,
                mode: request.mode,
                region_geometry_token: request.region_geometry_token,
                press_page_index: request.press_page_index,
                press_item_identity: request.press_item_identity,
                widget_id: request.widget_id,
                press_rect: request.press_rect,
                coordinate_frame: request.coordinate_frame,
                press_pixels_per_point: request.press_pixels_per_point,
                press_point: request.press_point,
            },
            kind: SyntheticPointerStepKind::Down {
                point: request.press_point,
            },
            completion: Some(completion),
        })?;
        Ok(SyntheticPointerCancelHandle {
            transaction_id,
            step_id,
            owner,
        })
    }

    #[cfg(all(windows, any(test, feature = "test-script")))]
    fn queue_pointer_held(
        &mut self,
        request: SyntheticPointerRequestedHeldStep,
        completion: mpsc::SyncSender<Result<SyntheticPointerCompletion, String>>,
    ) -> Result<u64, String> {
        if !self.armed {
            return Err("synthetic input is not armed".to_string());
        }
        self.pointer.queue_held(request, completion)
    }

    #[cfg(all(windows, any(test, feature = "test-script")))]
    fn queue_pointer_held_normalized(
        &mut self,
        expected_owner: &SyntheticPointerOwner,
        phase: SyntheticPointerHeldPhase,
        normalized: [f32; 2],
        completion: mpsc::SyncSender<Result<SyntheticPointerCompletion, String>>,
    ) -> Result<SyntheticPointerCancelHandle, String> {
        let latch = self
            .pointer
            .held_latch()
            .cloned()
            .ok_or_else(|| SyntheticPointerFailure::NotHeld.describe().to_string())?;
        if latch.owner != *expected_owner {
            self.pointer.cancel();
            return Err(format!(
                "selected pointer owner/catalog changed during gesture: latched={} selected={}",
                latch.owner.identity.describe(),
                expected_owner.identity.describe()
            ));
        }
        let rect = latch.press_rect;
        let point = egui::pos2(
            egui::lerp(rect.x_range(), normalized[0]),
            egui::lerp(rect.y_range(), normalized[1]),
        );
        let request = match phase {
            SyntheticPointerHeldPhase::Move => SyntheticPointerRequestedHeldStep::Move { point },
            SyntheticPointerHeldPhase::Up => {
                SyntheticPointerRequestedHeldStep::Up { final_point: point }
            }
        };
        let step_id = self.queue_pointer_held(request, completion)?;
        Ok(SyntheticPointerCancelHandle {
            transaction_id: latch.transaction_id,
            step_id,
            owner: latch.owner,
        })
    }

    #[cfg_attr(not(test), allow(dead_code))]
    fn enqueue(&mut self, command: SyntheticKeyCommand) {
        let queued = QueuedSyntheticCommand {
            sequence: self.next_sequence,
            command,
        };
        self.next_sequence = self.next_sequence.wrapping_add(1);
        let index = self
            .commands
            .iter()
            .position(|existing| {
                (existing.command.at, existing.sequence) > (queued.command.at, queued.sequence)
            })
            .unwrap_or(self.commands.len());
        self.commands.insert(index, queued);
    }

    fn modifiers(&self) -> SyntheticModifiers {
        self.held
            .iter()
            .fold(SyntheticModifiers::default(), |current, held| {
                current.union(held.modifiers)
            })
    }

    fn physical_key_down(&self, slot: PhysicalKeySlot) -> bool {
        if !self.armed {
            return false;
        }
        let modifiers = self.modifiers();
        match slot.vk {
            0x10 => modifiers.shift,
            0x11 => modifiers.ctrl,
            0x12 => modifiers.alt,
            // Sided modifier slots are intentionally outside the initial
            // synthetic navigation API.
            0xA0..=0xA5 => false,
            _ => self.held.iter().any(|held| {
                let held_slot = held.key.physical_slot();
                held_slot.vk == slot.vk && (slot.vk != 0x0D || held_slot.extended == slot.extended)
            }),
        }
    }

    fn next_repeat_index_through(&self, cutoff: Instant) -> Option<usize> {
        self.held
            .iter()
            .enumerate()
            .filter(|(_, held)| held.next_repeat_at <= cutoff)
            .min_by_key(|(_, held)| (held.next_repeat_at, held.order))
            .map(|(index, _)| index)
    }

    fn emit_repeats_through(
        &mut self,
        cutoff: Instant,
        events: &mut Vec<MaterializedSyntheticEvent>,
    ) {
        while let Some(index) = self.next_repeat_index_through(cutoff) {
            let modifiers = self.modifiers();
            let held = &mut self.held[index];
            let at = held.next_repeat_at;
            held.materialized_repeat_count = held.materialized_repeat_count.saturating_add(1);
            events.push(MaterializedSyntheticEvent {
                at,
                key: held.key,
                target: held.target,
                hold_id: held.hold_id,
                pressed: true,
                repeat: true,
                modifiers,
            });
            held.next_repeat_at += held.repeat_interval;
        }
    }

    fn release_key(
        &mut self,
        key: SyntheticNavigationKey,
        hold_id: Option<u64>,
        at: Instant,
        events: &mut Vec<MaterializedSyntheticEvent>,
        facts: &mut Vec<SyntheticInputFact>,
        collect_facts: bool,
    ) {
        let Some(index) = self.held.iter().position(|held| {
            held.key == key && hold_id.map_or(true, |hold_id| held.hold_id == Some(hold_id))
        }) else {
            return;
        };
        let modifiers = self.modifiers();
        let held = self.held.remove(index);
        events.push(MaterializedSyntheticEvent {
            at,
            key: held.key,
            target: held.target,
            hold_id: held.hold_id,
            pressed: false,
            repeat: false,
            modifiers,
        });
        if collect_facts && let Some(hold_id) = held.hold_id {
            facts.push(SyntheticInputFact::HoldEnd {
                hold_id,
                down_count: held.materialized_down_count,
                repeat_count: held.materialized_repeat_count,
                up_count: 1,
            });
        }
    }

    fn cancel_all_at(
        &mut self,
        at: Instant,
        events: &mut Vec<MaterializedSyntheticEvent>,
        facts: &mut Vec<SyntheticInputFact>,
        collect_facts: bool,
    ) {
        while !self.held.is_empty() {
            let index = self
                .held
                .iter()
                .enumerate()
                .min_by_key(|(_, held)| held.order)
                .map(|(index, _)| index)
                .unwrap_or(0);
            let modifiers = self.modifiers();
            let held = self.held.remove(index);
            events.push(MaterializedSyntheticEvent {
                at,
                key: held.key,
                target: held.target,
                hold_id: held.hold_id,
                pressed: false,
                repeat: false,
                modifiers,
            });
            if collect_facts && let Some(hold_id) = held.hold_id {
                facts.push(SyntheticInputFact::HoldEnd {
                    hold_id,
                    down_count: held.materialized_down_count,
                    repeat_count: held.materialized_repeat_count,
                    up_count: 1,
                });
            }
        }
    }

    fn append_frame_input_facts(
        &self,
        events: &[MaterializedSyntheticEvent],
        facts: &mut Vec<SyntheticInputFact>,
    ) {
        let mut hold_ids = Vec::new();
        for hold_id in events
            .iter()
            .filter_map(|event| event.hold_id)
            .chain(self.held.iter().filter_map(|held| held.hold_id))
        {
            if !hold_ids.contains(&hold_id) {
                hold_ids.push(hold_id);
            }
        }
        for hold_id in hold_ids {
            facts.push(SyntheticInputFact::FrameInput {
                hold_id,
                held: self.held.iter().any(|held| held.hold_id == Some(hold_id)),
                edge_count: events
                    .iter()
                    .filter(|event| event.hold_id == Some(hold_id))
                    .count() as u64,
                materialized_in_frame: events
                    .iter()
                    .filter(|event| event.hold_id == Some(hold_id) && event.repeat)
                    .count() as u64,
            });
        }
    }

    fn materialize<F, R>(
        &mut self,
        now: Instant,
        mut resolve: R,
        mut focused: F,
        collect_facts: bool,
    ) -> SyntheticMaterialization
    where
        F: FnMut(egui::ViewportId) -> Option<bool>,
        R: FnMut() -> Result<SyntheticRoutingTarget, SyntheticRoutingTargetError>,
    {
        if !self.armed {
            return SyntheticMaterialization::disarmed();
        }

        let mut events = Vec::new();
        let mut facts = Vec::new();
        if let Some(lost) = self
            .held
            .iter()
            .find(|held| focused(held.target.viewport) == Some(false))
            .map(|held| held.target.viewport)
        {
            self.cancel_all_at(now, &mut events, &mut facts, collect_facts);
            self.commands.clear();
            if collect_facts {
                self.append_frame_input_facts(&events, &mut facts);
            }
            return SyntheticMaterialization {
                armed: true,
                events,
                facts,
                final_modifiers: SyntheticModifiers::default(),
                issue: Some(SyntheticInputIssue::FocusLost { viewport: lost }),
            };
        }

        let mut issue = None;
        loop {
            let Some(queued) = self.commands.front().copied() else {
                break;
            };
            if queued.command.at > now {
                break;
            }

            // Repeats due at the same timestamp as an Up/Cancel are emitted
            // first, preserving Down -> due repeats -> Up after a sleeping UI.
            self.emit_repeats_through(queued.command.at, &mut events);
            match queued.command.kind {
                SyntheticKeyCommandKind::Down { key, modifiers } => {
                    if self.held.iter().any(|held| held.key == key) {
                        self.commands.pop_front();
                        continue;
                    }
                    let target = match resolve() {
                        Ok(target) => target,
                        Err(error) => {
                            issue = Some(SyntheticInputIssue::WaitingForRouting(error));
                            break;
                        }
                    };
                    if focused(target.viewport) != Some(true) {
                        issue = Some(SyntheticInputIssue::WaitingForFocus(target));
                        break;
                    }
                    self.commands.pop_front();
                    self.held.push(HeldSyntheticKey {
                        key,
                        modifiers,
                        target,
                        hold_id: queued.command.hold_id,
                        next_repeat_at: queued.command.at + self.repeat_delay,
                        repeat_interval: self.repeat_interval,
                        order: queued.sequence,
                        materialized_down_count: 1,
                        materialized_repeat_count: 0,
                    });
                    events.push(MaterializedSyntheticEvent {
                        at: queued.command.at,
                        key,
                        target,
                        hold_id: queued.command.hold_id,
                        pressed: true,
                        repeat: false,
                        modifiers: self.modifiers(),
                    });
                    if collect_facts && let Some(hold_id) = queued.command.hold_id {
                        facts.push(SyntheticInputFact::HoldBegin {
                            hold_id,
                            key,
                            target,
                            repeat_delay: self.repeat_delay,
                            repeat_hz: 1.0 / self.repeat_interval.as_secs_f64(),
                        });
                    }
                }
                SyntheticKeyCommandKind::Up { key } => {
                    self.commands.pop_front();
                    self.release_key(
                        key,
                        queued.command.hold_id,
                        queued.command.at,
                        &mut events,
                        &mut facts,
                        collect_facts,
                    );
                }
                SyntheticKeyCommandKind::CancelAll => {
                    self.commands.pop_front();
                    self.cancel_all_at(queued.command.at, &mut events, &mut facts, collect_facts);
                }
            }
        }
        if issue.is_none() {
            self.emit_repeats_through(now, &mut events);
        }

        if collect_facts {
            self.append_frame_input_facts(&events, &mut facts);
        }

        SyntheticMaterialization {
            armed: true,
            events,
            facts,
            final_modifiers: self.modifiers(),
            issue,
        }
    }
}

#[derive(Default)]
struct KeyInputState {
    installed_hwnds: HwndViewportRegistry,
    pending: VecDeque<KeyEdge>,
    frame: Vec<KeyEdge>,
    frame_active_viewports: Vec<egui::ViewportId>,
    return_keys: ViewportReturnKeyStates,
    logged_unregistered_hwnds: Vec<u64>,
    synthetic: SyntheticTimeline,
    #[cfg(test)]
    test_foreground_hwnd: Option<u64>,
}

impl KeyInputState {
    fn register_hwnd(&mut self, hwnd_raw: u64, viewport: egui::ViewportId) -> RegisterHwndResult {
        let result = self.installed_hwnds.register(hwnd_raw, viewport);
        if matches!(result, RegisterHwndResult::Inserted) {
            self.logged_unregistered_hwnds
                .retain(|logged| *logged != hwnd_raw);
        }
        result
    }

    fn unregister_hwnd(&mut self, hwnd_raw: u64) -> Option<egui::ViewportId> {
        let viewport = self.installed_hwnds.remove(hwnd_raw)?;
        // Edges are stamped with their source HWND. Once that HWND dies, do
        // not let an edge queued before WM_NCDESTROY reach a recreated
        // viewport that happens to reuse the same ViewportId.
        self.pending.retain(|edge| edge.source_hwnd != hwnd_raw);
        self.frame.retain(|edge| edge.source_hwnd != hwnd_raw);
        if !self.installed_hwnds.contains_viewport(viewport) {
            self.return_keys.clear_viewport(viewport);
        }
        if self.installed_hwnds.is_empty() {
            self.pending.clear();
            self.frame.clear();
            self.frame_active_viewports.clear();
            self.return_keys.clear();
        }
        Some(viewport)
    }

    fn enqueue_key_edge(&mut self, edge: KeyEdge) {
        self.return_keys.apply_edge(edge.source_viewport, &edge);
        while self.pending.len() >= MAX_PENDING_EVENTS {
            self.pending.pop_front();
        }
        self.pending.push_back(edge);
    }

    fn enqueue_raw_edge(&mut self, hwnd_raw: u64, raw: RawKeyEdge) -> (KeyEdge, bool) {
        // The root HWND is installed before its subclass can publish input.
        // Missing registration is therefore an invariant violation. Route it
        // explicitly to ROOT for compatibility, but make the violation
        // observable instead of exposing the edge to every viewport.
        let source_viewport = self
            .installed_hwnds
            .viewport_for_hwnd(hwnd_raw)
            .unwrap_or(egui::ViewportId::ROOT);
        let edge = raw.with_source(hwnd_raw, source_viewport);
        self.enqueue_key_edge(edge);

        let unregistered = self.installed_hwnds.viewport_for_hwnd(hwnd_raw).is_none();
        let should_log = unregistered && !self.logged_unregistered_hwnds.contains(&hwnd_raw);
        if should_log {
            while self.logged_unregistered_hwnds.len() >= MAX_LOGGED_UNREGISTERED_HWND {
                self.logged_unregistered_hwnds.remove(0);
            }
            self.logged_unregistered_hwnds.push(hwnd_raw);
        }
        (edge, should_log)
    }

    fn routed_return_key_held(&self, viewport: egui::ViewportId, extended: bool) -> Option<bool> {
        self.frame_active_viewports
            .contains(&viewport)
            .then(|| self.return_keys.is_down(viewport, extended))
    }

    fn materialize_synthetic<F>(&mut self, now: Instant, focused: F) -> SyntheticMaterialization
    where
        F: FnMut(egui::ViewportId) -> Option<bool>,
    {
        #[cfg(test)]
        let foreground_override = self.test_foreground_hwnd;
        let registry = &self.installed_hwnds;
        let materialized = self.synthetic.materialize(
            now,
            || {
                #[cfg(test)]
                let hwnd = foreground_override.unwrap_or_else(current_foreground_hwnd_raw);
                #[cfg(not(test))]
                let hwnd = current_foreground_hwnd_raw();
                resolve_registered_target(registry, hwnd)
            },
            focused,
            synthetic_perf_facts_enabled(),
        );
        for event in &materialized.events {
            self.enqueue_key_edge(event.key_edge());
        }
        materialized
    }
}

fn state() -> &'static Mutex<KeyInputState> {
    static STATE: OnceLock<Mutex<KeyInputState>> = OnceLock::new();
    STATE.get_or_init(|| Mutex::new(KeyInputState::default()))
}

#[inline]
fn synthetic_perf_facts_enabled() -> bool {
    #[cfg(test)]
    {
        true
    }
    #[cfg(all(not(test), feature = "test-script"))]
    {
        crate::perf::is_enabled()
    }
    #[cfg(all(not(test), not(feature = "test-script")))]
    {
        false
    }
}

#[cfg(windows)]
fn current_foreground_hwnd_raw() -> u64 {
    use windows::Win32::UI::WindowsAndMessaging::GetForegroundWindow;

    unsafe { GetForegroundWindow().0 as usize as u64 }
}

#[cfg(not(windows))]
fn current_foreground_hwnd_raw() -> u64 {
    0
}

fn resolve_registered_target(
    registry: &HwndViewportRegistry,
    hwnd: u64,
) -> Result<SyntheticRoutingTarget, SyntheticRoutingTargetError> {
    if hwnd == 0 {
        return Err(SyntheticRoutingTargetError::NoForegroundWindow);
    }
    let Some(viewport) = registry.viewport_for_hwnd(hwnd) else {
        return Err(SyntheticRoutingTargetError::UnregisteredForegroundWindow { hwnd });
    };
    Ok(SyntheticRoutingTarget { hwnd, viewport })
}

pub fn resolve_synthetic_routing_target()
-> Result<SyntheticRoutingTarget, SyntheticRoutingTargetError> {
    let Ok(guard) = state().lock() else {
        return Err(SyntheticRoutingTargetError::NoForegroundWindow);
    };
    #[cfg(test)]
    let hwnd = guard
        .test_foreground_hwnd
        .unwrap_or_else(current_foreground_hwnd_raw);
    #[cfg(not(test))]
    let hwnd = current_foreground_hwnd_raw();
    resolve_registered_target(&guard.installed_hwnds, hwnd)
}

/// Return the current physical level from the synthetic timeline while it is
/// armed, or from the caller's operating-system source otherwise.
///
/// The OS source is a parameter because the two readers need different Win32
/// calls. Callers asking "is this key held right now" want `GetAsyncKeyState`,
/// while the subclass proc must stamp an edge with `GetKeyState`, whose value is
/// synchronized with the message being processed. Substituting the async state
/// there would describe the wrong moment whenever messages are drained late,
/// which is exactly the slow-frame case this timeline exists to reproduce.
fn physical_key_down_from(slot: PhysicalKeySlot, os_level: impl FnOnce() -> bool) -> bool {
    if let Ok(guard) = state().lock()
        && guard.synthetic.armed
    {
        return guard.synthetic.physical_key_down(slot);
    }
    os_level()
}

/// Return the current physical level from the synthetic timeline while it is
/// armed, or from the operating system otherwise.
///
/// This chokepoint is deliberately not feature-gated. Before the script runner
/// exists, production has no arming path and therefore follows the OS branch.
pub fn physical_key_down(slot: PhysicalKeySlot) -> bool {
    physical_key_down_from(slot, || {
        use windows::Win32::UI::Input::KeyboardAndMouse::GetAsyncKeyState;

        unsafe { GetAsyncKeyState(slot.vk as i32) < 0 }
    })
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct RawInputTimeKey(Option<u64>);

impl From<Option<f64>> for RawInputTimeKey {
    fn from(time: Option<f64>) -> Self {
        Self(time.map(f64::to_bits))
    }
}

#[derive(Clone, Debug)]
struct SyntheticViewportBatch {
    viewport: egui::ViewportId,
    events: Vec<egui::Event>,
}

#[derive(Clone, Debug)]
struct PreparedSyntheticFrame {
    #[cfg_attr(not(feature = "test-script"), allow(dead_code))]
    frame_nr: u64,
    time_key: RawInputTimeKey,
    raw_input_time: Option<f64>,
    batches: Vec<SyntheticViewportBatch>,
    /// Synthetic holds whose events were materialized for each target viewport
    /// in this RawInput frame. This keeps the release frame attributable after
    /// the timeline has removed the held level.
    #[cfg_attr(not(feature = "test-script"), allow(dead_code))]
    hold_attributions: Vec<(egui::ViewportId, u64)>,
    delivered_viewports: Vec<egui::ViewportId>,
    final_modifiers: egui::Modifiers,
    armed: bool,
    #[cfg(all(windows, any(test, feature = "test-script")))]
    pointer_delivery: PreparedSyntheticPointerDelivery,
}

#[cfg(all(windows, any(test, feature = "test-script")))]
#[derive(Clone, Debug)]
enum PreparedSyntheticPointerDelivery {
    Absent,
    Pending(PreparedSyntheticPointerStep),
    Delivered(PreparedSyntheticPointerStep),
}

#[derive(Default)]
struct SyntheticInputPlugin {
    prepared: Option<PreparedSyntheticFrame>,
    issues: VecDeque<SyntheticInputIssue>,
    next_frame_nr: u64,
}

impl SyntheticInputPlugin {
    fn record_issue(&mut self, issue: SyntheticInputIssue) {
        while self.issues.len() >= MAX_SYNTHETIC_INPUT_ISSUES {
            self.issues.pop_front();
        }
        self.issues.push_back(issue);
    }

    fn record_undelivered_previous_frame(&mut self) {
        // Clone the immutable one-frame transport before recording issues, because issue
        // recording mutates the plugin queue.
        let Some(prepared) = self.prepared.clone() else {
            return;
        };
        let misses: Vec<_> = prepared
            .batches
            .iter()
            .filter(|batch| {
                batch.viewport != egui::ViewportId::ROOT
                    && !batch.events.is_empty()
                    && !prepared.delivered_viewports.contains(&batch.viewport)
            })
            .map(|batch| SyntheticInputIssue::TargetViewportNotRendered {
                viewport: batch.viewport,
                raw_input_time: prepared.raw_input_time,
                event_count: batch.events.len(),
            })
            .collect();
        for miss in misses {
            self.record_issue(miss);
        }

        #[cfg(all(windows, any(test, feature = "test-script")))]
        match prepared.pointer_delivery.clone() {
            PreparedSyntheticPointerDelivery::Absent => {}
            PreparedSyntheticPointerDelivery::Pending(pointer) => {
                let viewport = pointer.step.latch.owner.identity.viewport_id();
                let failed = state().lock().ok().and_then(|mut guard| {
                    guard
                        .synthetic
                        .pointer
                        .fail_undelivered(&pointer.step)
                        .then(|| pointer.step.cancel_handle())
                });
                if let Some(handle) = failed {
                    self.record_issue(SyntheticInputIssue::PointerViewportNotRendered {
                        handle,
                        viewport,
                    });
                }
            }
            PreparedSyntheticPointerDelivery::Delivered(pointer) => {
                let viewport = pointer.step.latch.owner.identity.viewport_id();
                let failed = state().lock().ok().and_then(|mut guard| {
                    guard
                        .synthetic
                        .pointer
                        .fail_missing_show_tail(&pointer.step)
                        .then(|| pointer.step.cancel_handle())
                });
                if let Some(handle) = failed {
                    self.record_issue(SyntheticInputIssue::MissingPointerShowTail {
                        handle,
                        viewport,
                    });
                }
            }
        }
    }

    fn prepare_root(&mut self, input: &egui::RawInput) {
        let time_key = RawInputTimeKey::from(input.time);
        if self
            .prepared
            .as_ref()
            .is_some_and(|prepared| prepared.time_key == time_key)
        {
            return;
        }

        self.record_undelivered_previous_frame();
        let frame_nr = self.next_frame_nr;
        self.next_frame_nr = self.next_frame_nr.saturating_add(1);
        let (materialized, pointer_step) = state()
            .lock()
            .map(|mut guard| {
                let materialized = guard.materialize_synthetic(Instant::now(), |viewport| {
                    if viewport == input.viewport_id {
                        Some(input.focused)
                    } else {
                        input.viewports.get(&viewport).and_then(|info| info.focused)
                    }
                });
                #[cfg(all(windows, any(test, feature = "test-script")))]
                let pointer_step = input.time.and_then(|_| guard.synthetic.pointer.prepare());
                #[cfg(not(all(windows, any(test, feature = "test-script"))))]
                let pointer_step = None::<()>;
                (materialized, pointer_step)
            })
            .unwrap_or_else(|_| (SyntheticMaterialization::disarmed(), None));
        #[cfg(not(all(windows, any(test, feature = "test-script"))))]
        let _ = pointer_step;
        debug_assert!(
            materialized
                .events
                .windows(2)
                .all(|pair| pair[0].at <= pair[1].at),
            "synthetic input materialization must stay chronological"
        );
        emit_synthetic_input_facts(&materialized.facts, frame_nr);

        let mut hold_attributions = Vec::new();
        for event in &materialized.events {
            let Some(hold_id) = event.hold_id else {
                continue;
            };
            let attribution = (event.target.viewport, hold_id);
            if !hold_attributions.contains(&attribution) {
                hold_attributions.push(attribution);
            }
        }

        let mut batches: Vec<SyntheticViewportBatch> = Vec::new();
        for event in materialized.events {
            let egui_event = event.egui_event();
            if let Some(batch) = batches
                .iter_mut()
                .find(|batch| batch.viewport == event.target.viewport)
            {
                batch.events.push(egui_event);
            } else {
                batches.push(SyntheticViewportBatch {
                    viewport: event.target.viewport,
                    events: vec![egui_event],
                });
            }
        }
        if let Some(issue) = materialized.issue {
            self.record_issue(issue);
        }
        self.prepared = Some(PreparedSyntheticFrame {
            frame_nr,
            time_key,
            raw_input_time: input.time,
            batches,
            hold_attributions,
            delivered_viewports: Vec::new(),
            final_modifiers: materialized.final_modifiers.to_egui(),
            armed: materialized.armed,
            #[cfg(all(windows, any(test, feature = "test-script")))]
            pointer_delivery: pointer_step
                .map(|step| {
                    PreparedSyntheticPointerDelivery::Pending(PreparedSyntheticPointerStep {
                        raw_frame: frame_nr,
                        raw_time_bits: input
                            .time
                            .expect("pointer preparation requires RawInput.time")
                            .to_bits(),
                        step,
                    })
                })
                .unwrap_or(PreparedSyntheticPointerDelivery::Absent),
        });
    }

    fn inject_prepared(&mut self, input: &mut egui::RawInput) {
        let Some(prepared) = self.prepared.as_mut() else {
            return;
        };
        if !prepared.armed {
            return;
        }
        input.modifiers = prepared.final_modifiers;
        let Some(batch) = prepared
            .batches
            .iter()
            .find(|batch| batch.viewport == input.viewport_id)
        else {
            return;
        };
        input.events.extend(batch.events.iter().cloned());
        if !prepared.delivered_viewports.contains(&input.viewport_id) {
            prepared.delivered_viewports.push(input.viewport_id);
        }
    }

    #[cfg(all(windows, any(test, feature = "test-script")))]
    fn reject_physical_pointer_mix(&mut self, input: &egui::RawInput) {
        let has_physical_pointer = input.events.iter().any(|event| {
            matches!(
                event,
                egui::Event::PointerMoved(_)
                    | egui::Event::PointerButton { .. }
                    | egui::Event::PointerGone
            )
        });
        if !has_physical_pointer {
            return;
        }
        let handle = state().lock().ok().and_then(|mut guard| {
            guard
                .synthetic
                .pointer
                .fail_physical_input_mixed(input.viewport_id)
        });
        if let Some(handle) = handle {
            self.record_issue(SyntheticInputIssue::PointerPhysicalInputMixed { handle });
        }
    }

    #[cfg(all(windows, any(test, feature = "test-script")))]
    fn inject_prepared_pointer(&mut self, input: &mut egui::RawInput) {
        let Some(pointer) =
            self.prepared
                .as_ref()
                .and_then(|frame| match &frame.pointer_delivery {
                    PreparedSyntheticPointerDelivery::Pending(pointer) => Some(pointer.clone()),
                    PreparedSyntheticPointerDelivery::Absent
                    | PreparedSyntheticPointerDelivery::Delivered(_) => None,
                })
        else {
            return;
        };
        if pointer.step.latch.owner.identity.viewport_id() != input.viewport_id {
            return;
        }

        let owner_matches =
            crate::test_script::pointer_input::active_show_matches(&pointer.step.latch.owner);
        let witness_matches =
            eframe::miv_test_script_window_witness::active().is_some_and(|witness| {
                pointer
                    .step
                    .latch
                    .owner
                    .identity
                    .matches_backend_witness(witness)
            });
        let rejection = if !owner_matches || !witness_matches {
            Some("prepared pointer did not join the active lexical child show".to_string())
        } else if input.time.is_none() {
            Some("pointer child RawInput had no delivery-time witness".to_string())
        } else {
            None
        };
        let disposition = state()
            .lock()
            .map(|mut guard| {
                guard
                    .synthetic
                    .pointer
                    .resolve_prepared_delivery(&pointer.step, rejection.is_some())
            })
            .unwrap_or(PreparedPointerDeliveryDisposition::Stale);
        match disposition {
            PreparedPointerDeliveryDisposition::Stale => {
                // Timeout/cancellation can outlive the immutable transport cache. A cache whose
                // exact step is no longer Prepared has no obligation and must not poison a later
                // host or inject the old Down.
                return;
            }
            PreparedPointerDeliveryDisposition::Rejected(handle) => {
                self.record_issue(SyntheticInputIssue::PointerOwnerMismatch {
                    handle,
                    detail: rejection.expect("rejected delivery has a reason"),
                });
                return;
            }
            PreparedPointerDeliveryDisposition::Delivered => {}
        }
        input.events.extend(pointer.step.egui_events());
        crate::test_script::pointer_input::record_delivery_proof(&pointer, input);
        if let Some(prepared_frame) = self.prepared.as_mut()
            && matches!(
                &prepared_frame.pointer_delivery,
                PreparedSyntheticPointerDelivery::Pending(cached) if cached.same_payload(&pointer)
            )
        {
            prepared_frame.pointer_delivery = PreparedSyntheticPointerDelivery::Delivered(pointer);
        }
    }
}

#[cfg(feature = "test-script")]
fn emit_synthetic_input_facts(facts: &[SyntheticInputFact], frame_nr: u64) {
    if !crate::perf::is_enabled() {
        return;
    }
    for fact in facts {
        match fact {
            SyntheticInputFact::HoldBegin {
                hold_id,
                key,
                target,
                repeat_delay,
                repeat_hz,
            } => {
                let target_viewport = if target.viewport == egui::ViewportId::ROOT {
                    "ROOT".to_string()
                } else {
                    format!("{:?}", target.viewport)
                };
                crate::perf::event(
                    "test_script",
                    "hold_begin",
                    Some(key.as_str()),
                    0,
                    &[
                        ("hold_id", serde_json::Value::from(*hold_id)),
                        ("target_viewport", serde_json::Value::from(target_viewport)),
                        (
                            "repeat_delay_ms",
                            serde_json::Value::from(repeat_delay.as_secs_f64() * 1000.0),
                        ),
                        ("repeat_hz", serde_json::Value::from(*repeat_hz)),
                    ],
                );
            }
            SyntheticInputFact::HoldEnd {
                hold_id,
                down_count,
                repeat_count,
                up_count,
            } => crate::perf::event(
                "test_script",
                "hold_end",
                None,
                0,
                &[
                    ("hold_id", serde_json::Value::from(*hold_id)),
                    ("down_count", serde_json::Value::from(*down_count)),
                    ("repeat_count", serde_json::Value::from(*repeat_count)),
                    ("up_count", serde_json::Value::from(*up_count)),
                ],
            ),
            SyntheticInputFact::FrameInput {
                hold_id,
                held,
                edge_count,
                materialized_in_frame,
            } => crate::perf::event(
                "test_script",
                "frame_input",
                None,
                0,
                &[
                    ("hold_id", serde_json::Value::from(*hold_id)),
                    ("held", serde_json::Value::from(*held)),
                    ("edge_count", serde_json::Value::from(*edge_count)),
                    (
                        "materialized_in_frame",
                        serde_json::Value::from(*materialized_in_frame),
                    ),
                    ("frame_nr", serde_json::Value::from(frame_nr)),
                ],
            ),
        }
    }
}

#[cfg(not(feature = "test-script"))]
fn emit_synthetic_input_facts(_facts: &[SyntheticInputFact], _frame_nr: u64) {}

impl egui::Plugin for SyntheticInputPlugin {
    fn debug_name(&self) -> &'static str {
        "miv_synthetic_input"
    }

    fn input_hook(&mut self, input: &mut egui::RawInput) {
        if input.viewport_id == egui::ViewportId::ROOT {
            self.prepare_root(input);
        }
        #[cfg(all(windows, any(test, feature = "test-script")))]
        self.reject_physical_pointer_mix(input);
        self.inject_prepared(input);
        #[cfg(all(windows, any(test, feature = "test-script")))]
        self.inject_prepared_pointer(input);
    }
}

/// Install before the IME input plugin so synthetic Escape and Enter traverse
/// the same normalization order as backend-generated key events.
pub(crate) fn install_synthetic_input_plugin(ctx: &egui::Context) {
    ctx.add_plugin(SyntheticInputPlugin::default());
}

pub fn take_synthetic_input_issues(ctx: &egui::Context) -> Vec<SyntheticInputIssue> {
    ctx.with_plugin(|plugin: &mut SyntheticInputPlugin| plugin.issues.drain(..).collect())
        .unwrap_or_default()
}

#[cfg(feature = "test-script")]
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct SyntheticHoldObservation {
    pub(crate) frame_nr: u64,
    pub(crate) key: SyntheticNavigationKey,
    pub(crate) hold_ids: Vec<u64>,
}

/// Return the synthetic holds that are active in the RawInput frame currently
/// being processed. The result only identifies which production level reads
/// need attribution; the held value itself must still be read through
/// `Keymap::key_held_chord`.
#[cfg(feature = "test-script")]
pub(crate) fn synthetic_hold_observations(ctx: &egui::Context) -> Vec<SyntheticHoldObservation> {
    let Some((frame_nr, armed)) = ctx
        .with_plugin(|plugin: &mut SyntheticInputPlugin| {
            plugin
                .prepared
                .as_ref()
                .map(|prepared| (prepared.frame_nr, prepared.armed))
        })
        .flatten()
    else {
        return Vec::new();
    };
    if !armed {
        return Vec::new();
    }

    let Ok(guard) = state().lock() else {
        return Vec::new();
    };
    let mut observations: Vec<SyntheticHoldObservation> = Vec::new();
    for held in &guard.synthetic.held {
        let Some(hold_id) = held.hold_id else {
            continue;
        };
        if let Some(existing) = observations
            .iter_mut()
            .find(|observation| observation.key == held.key)
        {
            existing.hold_ids.push(hold_id);
        } else {
            observations.push(SyntheticHoldObservation {
                frame_nr,
                key: held.key,
                hold_ids: vec![hold_id],
            });
        }
    }
    observations
}

/// Identify the single scripted hold attributable to the current RawInput
/// frame for a viewport. Active level ownership and a materialized release edge
/// are both facts from the synthetic input owner; no cross-frame history is used.
#[cfg(feature = "test-script")]
pub(crate) fn synthetic_frame_hold_id(
    ctx: &egui::Context,
    viewport: egui::ViewportId,
) -> Option<u64> {
    let mut hold_ids = ctx
        .with_plugin(|plugin: &mut SyntheticInputPlugin| {
            plugin
                .prepared
                .as_ref()
                .into_iter()
                .flat_map(|prepared| prepared.hold_attributions.iter())
                .filter_map(|(target, hold_id)| (*target == viewport).then_some(*hold_id))
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    if let Ok(guard) = state().lock() {
        hold_ids.extend(
            guard
                .synthetic
                .held
                .iter()
                .filter_map(|held| (held.target.viewport == viewport).then_some(held.hold_id))
                .flatten(),
        );
    }
    hold_ids.sort_unstable();
    hold_ids.dedup();
    if hold_ids.len() == 1 {
        hold_ids.first().copied()
    } else {
        None
    }
}

#[cfg(not(feature = "test-script"))]
pub(crate) fn synthetic_frame_hold_id(
    _ctx: &egui::Context,
    _viewport: egui::ViewportId,
) -> Option<u64> {
    None
}

/// Arm the production synthetic timeline without disturbing the HWND registry
/// or real key queues. The timeline remains the single owner of synthetic
/// level state used by [`physical_key_down`].
#[cfg(any(test, feature = "test-script"))]
pub fn arm_synthetic_input() -> bool {
    state()
        .lock()
        .map(|mut guard| guard.synthetic.arm())
        .is_ok()
}

/// Queue a timestamped synthetic key command. Materialization still happens
/// only in the ROOT input plugin on the UI thread.
#[cfg(any(test, feature = "test-script"))]
pub fn enqueue_synthetic_command(command: SyntheticKeyCommand) -> bool {
    state()
        .lock()
        .map(|mut guard| guard.synthetic.enqueue(command))
        .is_ok()
}

#[cfg(all(windows, any(test, feature = "test-script")))]
pub(crate) fn enqueue_synthetic_pointer_down(
    request: SyntheticPointerDownRequest,
) -> Result<
    (
        SyntheticPointerCancelHandle,
        mpsc::Receiver<Result<SyntheticPointerCompletion, String>>,
    ),
    String,
> {
    let (reply, completion) = mpsc::sync_channel(1);
    let cancel_handle = state()
        .lock()
        .map_err(|_| "synthetic input state is poisoned".to_string())?
        .synthetic
        .queue_pointer_down(request, reply)?;
    Ok((cancel_handle, completion))
}

#[cfg(all(windows, any(test, feature = "test-script")))]
pub(crate) fn enqueue_synthetic_pointer_held(
    expected_owner: &SyntheticPointerOwner,
    phase: SyntheticPointerHeldPhase,
    normalized: [f32; 2],
) -> Result<
    (
        SyntheticPointerCancelHandle,
        mpsc::Receiver<Result<SyntheticPointerCompletion, String>>,
    ),
    String,
> {
    let (reply, completion) = mpsc::sync_channel(1);
    let cancel_handle = state()
        .lock()
        .map_err(|_| "synthetic input state is poisoned".to_string())?
        .synthetic
        .queue_pointer_held_normalized(expected_owner, phase, normalized, reply)?;
    Ok((cancel_handle, completion))
}

#[cfg(all(windows, any(test, feature = "test-script")))]
pub(crate) fn cancel_synthetic_pointer_step(handle: &SyntheticPointerCancelHandle) -> bool {
    state()
        .lock()
        .map(|mut guard| guard.synthetic.pointer.cancel_handle(handle))
        .unwrap_or(false)
}

#[cfg(all(windows, any(test, feature = "test-script")))]
pub(crate) fn synthetic_pointer_held_snapshot() -> Option<SyntheticPointerHeldSnapshot> {
    state()
        .lock()
        .ok()
        .and_then(|guard| guard.synthetic.pointer.held_snapshot())
}

#[cfg(all(windows, any(test, feature = "test-script")))]
pub(crate) fn synthetic_pointer_obligates_owner(owner: &SyntheticPointerOwner) -> bool {
    state()
        .lock()
        .map(|guard| guard.synthetic.pointer.owner() == Some(owner))
        .unwrap_or(false)
}

#[cfg(all(windows, any(test, feature = "test-script")))]
pub(crate) fn finish_synthetic_pointer_show(tail: SyntheticPointerShowTail) -> Result<(), String> {
    state()
        .lock()
        .map_err(|_| "synthetic input state is poisoned".to_string())?
        .synthetic
        .pointer
        .finish_delivered_show(tail)
}

#[cfg(all(windows, any(test, feature = "test-script")))]
pub(crate) fn fail_synthetic_pointer_delivered(
    expected: &SyntheticPointerCancelHandle,
    detail: String,
) -> bool {
    state()
        .lock()
        .map(|mut guard| {
            guard.synthetic.pointer.fail_delivered(
                expected,
                SyntheticPointerFailure::TargetLostDuringCleanup,
                detail,
            )
        })
        .unwrap_or(false)
}

#[cfg(all(windows, any(test, feature = "test-script")))]
pub(crate) fn fail_synthetic_pointer_owner_lost(
    owner: &SyntheticPointerOwner,
) -> Option<SyntheticPointerCancelHandle> {
    state()
        .lock()
        .ok()
        .and_then(|mut guard| guard.synthetic.pointer.fail_owner_lost(owner))
}

#[cfg(all(windows, any(test, feature = "test-script")))]
pub(crate) fn acknowledge_synthetic_pointer_terminal_issue(
    expected: &SyntheticPointerCancelHandle,
) -> bool {
    state()
        .lock()
        .map(|mut guard| {
            guard
                .synthetic
                .pointer
                .acknowledge_terminal_failure(expected)
        })
        .unwrap_or(false)
}

#[cfg(all(windows, test))]
pub(crate) fn prepared_synthetic_pointer_step_for_test(
    step_id: u64,
    latch: SyntheticPointerLatch,
    kind: SyntheticPointerStepKind,
    raw_frame: u64,
    raw_time_bits: u64,
) -> PreparedSyntheticPointerStep {
    PreparedSyntheticPointerStep {
        raw_frame,
        raw_time_bits,
        step: SyntheticPointerStep {
            step_id,
            latch,
            kind,
            completion: None,
        },
    }
}

#[cfg(all(windows, test))]
pub(crate) fn install_synthetic_pointer_terminal_for_test(handle: SyntheticPointerCancelHandle) {
    let mut guard = state().lock().expect("key input state poisoned");
    guard.synthetic.armed = true;
    guard.synthetic.pointer = SyntheticPointerTransaction::TerminalFailure {
        handle,
        failure: SyntheticPointerFailure::CleanupDidNotRelease,
    };
}

#[cfg(all(windows, test))]
pub(crate) fn install_synthetic_pointer_delivered_for_test(
    mut prepared: PreparedSyntheticPointerStep,
) -> mpsc::Receiver<Result<SyntheticPointerCompletion, String>> {
    let (reply, completion) = mpsc::sync_channel(1);
    prepared.step.completion = Some(reply);
    state()
        .lock()
        .expect("synthetic input state is poisoned")
        .synthetic
        .pointer = SyntheticPointerTransaction::DeliveredAwaitingTail(prepared.step);
    completion
}

#[cfg(any(test, feature = "test-script"))]
pub fn set_synthetic_repeat(delay: Duration, hz: f64) -> bool {
    state()
        .lock()
        .map(|mut guard| guard.synthetic.set_repeat(delay, hz))
        .unwrap_or(false)
}

#[cfg(any(test, feature = "test-script"))]
pub fn synthetic_input_is_idle() -> bool {
    state()
        .lock()
        .map(|guard| guard.synthetic.is_idle())
        .unwrap_or(true)
}

/// Drop commands that have not acquired a routing target, then enqueue one
/// ordered CancelAll so any already-held keys still produce their Up events in
/// the ROOT plugin. This is used by every script terminal path.
#[cfg(any(test, feature = "test-script"))]
pub fn cancel_synthetic_input(at: Instant) -> bool {
    state()
        .lock()
        .map(|mut guard| {
            guard.synthetic.commands.clear();
            guard.synthetic.enqueue(SyntheticKeyCommand::cancel_all(at));
            #[cfg(all(windows, any(test, feature = "test-script")))]
            guard.synthetic.pointer.cancel();
        })
        .is_ok()
}

#[cfg(any(test, feature = "test-script"))]
pub fn disarm_synthetic_input() {
    if let Ok(mut guard) = state().lock() {
        guard.synthetic.disarm();
    }
}

pub fn install_main_window_subclass(hwnd_raw: u64) -> bool {
    install_window_subclass(hwnd_raw, egui::ViewportId::ROOT, "main")
}

pub fn install_viewport_window_subclass(hwnd_raw: u64, viewport: egui::ViewportId) -> bool {
    install_window_subclass(hwnd_raw, viewport, "viewport")
}

fn install_window_subclass(hwnd_raw: u64, viewport: egui::ViewportId, label: &'static str) -> bool {
    if hwnd_raw == 0 {
        return false;
    }
    let registration = match state().lock() {
        Ok(mut guard) => guard.register_hwnd(hwnd_raw, viewport),
        Err(_) => return false,
    };
    match registration {
        RegisterHwndResult::AlreadyRegistered => return true,
        RegisterHwndResult::ConflictingViewport(existing) => {
            crate::logger::log(format!(
                "key-input: HWND registration conflict label={label} hwnd=0x{hwnd_raw:x} \
                 existing_viewport={existing:?} requested_viewport={viewport:?}"
            ));
            return false;
        }
        RegisterHwndResult::Inserted => {}
    }
    let hwnd = HWND(hwnd_raw as *mut _);
    let ok = unsafe {
        SetWindowSubclass(
            hwnd,
            Some(main_key_input_subclass_proc),
            MAIN_KEY_INPUT_SUBCLASS_ID,
            0,
        )
        .as_bool()
    };
    if !ok {
        if let Ok(mut guard) = state().lock() {
            guard.unregister_hwnd(hwnd_raw);
        }
        crate::logger::log(format!(
            "key-input: SetWindowSubclass failed label={label} hwnd=0x{hwnd_raw:x} \
             viewport={viewport:?}"
        ));
    }
    ok
}

pub fn begin_frame() {
    if let Ok(mut guard) = state().lock() {
        guard.frame.clear();
        while let Some(edge) = guard.pending.pop_front() {
            guard.frame.push(edge);
        }
        guard.frame_active_viewports = guard.installed_hwnds.unique_viewports();
        let edge_viewports: Vec<_> = guard
            .frame
            .iter()
            .map(|edge| edge.source_viewport)
            .collect();
        for viewport in edge_viewports {
            if !guard.frame_active_viewports.contains(&viewport) {
                guard.frame_active_viewports.push(viewport);
            }
        }
    }
}

pub fn is_frame_active(viewport: egui::ViewportId) -> bool {
    state()
        .lock()
        .map(|guard| guard.frame_active_viewports.contains(&viewport))
        .unwrap_or(false)
}

pub fn frame_had_key_down(viewport: egui::ViewportId) -> bool {
    state()
        .lock()
        .map(|guard| {
            guard
                .frame
                .iter()
                .any(|edge| edge.source_viewport == viewport && edge.pressed)
        })
        .unwrap_or(false)
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ConsumeKeyDownResult {
    pub matched_count: usize,
    pub triggered_count: usize,
}

/// Ordered physical-edge facts for one consumed key press stream.
///
/// Unlike [`ConsumeKeyDownResult`], this preserves whether the frame contained
/// an initial press or only auto-repeat, and whether a later key-up ended the
/// same physical stream before its owner processed the frame.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ConsumeKeyPressEdgesResult {
    pub matched_count: usize,
    pub had_initial_press: bool,
    pub had_repeat: bool,
    pub released_after_match: bool,
}

/// Consume the ordered key-down stream for one chord plus any later release of
/// its physical slot.
///
/// `matches_key_down` owns modifier matching for presses. Releases deliberately
/// use `matches_physical_key`, because modifiers may be released before the main
/// key. A key-up that precedes the first matching key-down belongs to an older
/// press and is left untouched.
pub fn consume_key_press_edges_with_result<FDown, FPhysical>(
    viewport: egui::ViewportId,
    mut matches_key_down: FDown,
    mut matches_physical_key: FPhysical,
) -> ConsumeKeyPressEdgesResult
where
    FDown: FnMut(KeyEdge) -> bool,
    FPhysical: FnMut(KeyEdge) -> bool,
{
    state()
        .lock()
        .map(|mut guard| {
            let Some(mut index) = guard.frame.iter().position(|edge| {
                edge.source_viewport == viewport && edge.pressed && matches_key_down(*edge)
            }) else {
                return ConsumeKeyPressEdgesResult::default();
            };

            let mut result = ConsumeKeyPressEdgesResult::default();
            while index < guard.frame.len() {
                let edge = guard.frame[index];
                if edge.source_viewport != viewport {
                    index += 1;
                    continue;
                }

                let matching_down = edge.pressed && matches_key_down(edge);
                let matching_release = !edge.pressed && matches_physical_key(edge);
                if !matching_down && !matching_release {
                    index += 1;
                    continue;
                }

                guard.frame.remove(index);
                if matching_down {
                    result.matched_count += 1;
                    result.had_initial_press |= !edge.repeat;
                    result.had_repeat |= edge.repeat;
                    result.released_after_match = false;
                } else {
                    result.released_after_match = true;
                }
            }
            result
        })
        .unwrap_or_default()
}

pub fn consume_key_down_with_result<F>(
    viewport: egui::ViewportId,
    allow_repeat: bool,
    mut predicate: F,
) -> ConsumeKeyDownResult
where
    F: FnMut(KeyEdge) -> bool,
{
    consume_key_down_inner(viewport, allow_repeat, false, &mut predicate)
}

/// Consume every matching key-down edge from the current frame and return how
/// many action triggers they represent.
///
/// Non-repeat edges retain their physical cardinality. Auto-repeat edges keep
/// the historical per-frame behavior: when repeats are allowed they contribute
/// at most one trigger, and only when this frame has no matching physical press.
/// This prevents a long frame from turning accumulated OS repeats into delayed
/// navigation after the key is released.
pub fn consume_all_key_down_with_result<F>(
    viewport: egui::ViewportId,
    allow_repeat: bool,
    mut predicate: F,
) -> ConsumeKeyDownResult
where
    F: FnMut(KeyEdge) -> bool,
{
    consume_key_down_inner(viewport, allow_repeat, true, &mut predicate)
}

fn consume_key_down_inner<F>(
    viewport: egui::ViewportId,
    allow_repeat: bool,
    consume_all: bool,
    predicate: &mut F,
) -> ConsumeKeyDownResult
where
    F: FnMut(KeyEdge) -> bool,
{
    state()
        .lock()
        .map(|mut guard| {
            let mut result = ConsumeKeyDownResult::default();
            let mut physical_press_count = 0;
            let mut matched_repeat = false;
            let mut index = 0;
            while index < guard.frame.len() {
                let edge = guard.frame[index];
                if edge.source_viewport == viewport && edge.pressed && predicate(edge) {
                    result.matched_count += 1;
                    if edge.repeat {
                        matched_repeat = true;
                    } else {
                        physical_press_count += 1;
                    }
                    guard.frame.remove(index);
                    if !consume_all && allow_repeat {
                        break;
                    }
                } else {
                    index += 1;
                }
            }
            result.triggered_count = if consume_all {
                if physical_press_count > 0 {
                    physical_press_count
                } else {
                    usize::from(allow_repeat && matched_repeat)
                }
            } else {
                usize::from(physical_press_count > 0 || (allow_repeat && matched_repeat))
            };
            result
        })
        .unwrap_or_default()
}

pub fn consume_key_down<F>(viewport: egui::ViewportId, allow_repeat: bool, predicate: F) -> bool
where
    F: FnMut(KeyEdge) -> bool,
{
    consume_key_down_with_result(viewport, allow_repeat, predicate).triggered_count > 0
}

#[cfg(test)]
pub fn set_test_frame(edges: Vec<KeyEdge>) {
    set_test_frame_for_viewport(egui::ViewportId::ROOT, edges);
}

#[cfg(test)]
pub fn set_test_frame_for_viewport(viewport: egui::ViewportId, mut edges: Vec<KeyEdge>) {
    for edge in &mut edges {
        edge.source_viewport = viewport;
    }
    if let Ok(mut guard) = state().lock() {
        guard.frame = edges;
        guard.frame_active_viewports = vec![viewport];
    }
}

#[cfg(test)]
pub(crate) fn set_test_routed_frame(edges: Vec<KeyEdge>) {
    if let Ok(mut guard) = state().lock() {
        guard.frame_active_viewports.clear();
        for edge in &edges {
            if !guard.frame_active_viewports.contains(&edge.source_viewport) {
                guard.frame_active_viewports.push(edge.source_viewport);
            }
        }
        guard.frame = edges;
    }
}

#[cfg(test)]
pub fn clear_test_frame() {
    if let Ok(mut guard) = state().lock() {
        guard.frame.clear();
        guard.frame_active_viewports.clear();
        guard.return_keys.clear();
    }
}

/// 追加の viewport を frame-active に見せる (subclass 登録済みの兄弟 viewport 相当)。
#[cfg(test)]
pub fn add_test_frame_active_viewport(viewport: egui::ViewportId) {
    if let Ok(mut guard) = state().lock()
        && !guard.frame_active_viewports.contains(&viewport)
    {
        guard.frame_active_viewports.push(viewport);
    }
}

#[cfg(test)]
pub fn set_test_return_key_state(viewport: egui::ViewportId, main_down: bool, numpad_down: bool) {
    if let Ok(mut guard) = state().lock() {
        guard.return_keys.clear_viewport(viewport);
        for (extended, pressed) in [(false, main_down), (true, numpad_down)] {
            if !pressed {
                continue;
            }
            let edge = KeyEdge {
                source_hwnd: 1,
                source_viewport: viewport,
                virtual_key: 0x0D,
                scan_code: 0x1C,
                extended,
                pressed: true,
                repeat: false,
                ctrl: false,
                shift: false,
                alt: false,
            };
            guard.return_keys.apply_edge(viewport, &edge);
        }
    }
}

/// Arm a deterministic synthetic timeline and register its foreground target.
#[cfg(test)]
pub fn arm_test_synthetic_input(foreground_hwnd: u64, viewport: egui::ViewportId) {
    if let Ok(mut guard) = state().lock() {
        *guard = KeyInputState::default();
        guard.synthetic.arm();
        guard.test_foreground_hwnd = Some(foreground_hwnd);
        guard.register_hwnd(foreground_hwnd, viewport);
    }
}

#[cfg(test)]
pub fn arm_test_synthetic_input_without_registration(foreground_hwnd: u64) {
    if let Ok(mut guard) = state().lock() {
        *guard = KeyInputState::default();
        guard.synthetic.arm();
        guard.test_foreground_hwnd = Some(foreground_hwnd);
    }
}

#[cfg(test)]
pub fn register_test_synthetic_target(hwnd: u64, viewport: egui::ViewportId) {
    if let Ok(mut guard) = state().lock() {
        guard.register_hwnd(hwnd, viewport);
    }
}

#[cfg(test)]
pub fn set_test_synthetic_repeat(delay: Duration, hz: f64) -> bool {
    if let Ok(mut guard) = state().lock() {
        guard.synthetic.set_repeat(delay, hz)
    } else {
        false
    }
}

#[cfg(test)]
pub fn enqueue_test_synthetic_command(command: SyntheticKeyCommand) {
    if let Ok(mut guard) = state().lock() {
        guard.synthetic.enqueue(command);
    }
}

#[cfg(test)]
pub fn advance_test_synthetic_input(now: Instant) {
    if let Ok(mut guard) = state().lock() {
        let _ = guard.materialize_synthetic(now, |_| Some(true));
    }
}

#[cfg(test)]
pub fn clear_test_synthetic_input() {
    if let Ok(mut guard) = state().lock() {
        *guard = KeyInputState::default();
    }
}

#[cfg(test)]
fn materialize_test_synthetic_input(
    now: Instant,
    focused: impl FnMut(egui::ViewportId) -> Option<bool>,
) -> SyntheticMaterialization {
    state()
        .lock()
        .map(|mut guard| guard.materialize_synthetic(now, focused))
        .unwrap_or_else(|_| SyntheticMaterialization::disarmed())
}

#[cfg(test)]
pub(crate) static TEST_INPUT_LOCK: OnceLock<Mutex<()>> = OnceLock::new();

pub fn pressed_key_down<F>(viewport: egui::ViewportId, predicate: F) -> bool
where
    F: Fn(KeyEdge) -> bool,
{
    state()
        .lock()
        .map(|guard| {
            guard
                .frame
                .iter()
                .any(|edge| edge.source_viewport == viewport && edge.pressed && predicate(*edge))
        })
        .unwrap_or(false)
}

/// Return the first matching key-down edge regardless of source viewport without consuming it.
///
/// This is a diagnostics-only projection over the current frame. Application input routing must
/// continue to use the viewport-scoped APIs above; this cross-viewport read must never decide or
/// dispatch an action.
pub fn diagnostic_pressed_key_down_any_viewport<F>(predicate: F) -> Option<KeyEdge>
where
    F: Fn(KeyEdge) -> bool,
{
    state().lock().ok().and_then(|guard| {
        guard
            .frame
            .iter()
            .copied()
            .find(|edge| edge.pressed && predicate(*edge))
    })
}

/// Consume all matching physical key edges from the current frame.
///
/// Unlike egui's `Event::Key`, the Win32 edge retains scan-code and extended-bit
/// information, so callers can distinguish main Enter from numpad Enter on both
/// key-down and key-up.
pub fn consume_key_edges<F>(viewport: egui::ViewportId, mut predicate: F) -> (bool, bool)
where
    F: FnMut(KeyEdge) -> bool,
{
    state()
        .lock()
        .map(|mut guard| {
            let mut pressed = false;
            let mut released = false;
            let mut index = 0;
            while index < guard.frame.len() {
                let edge = guard.frame[index];
                if edge.source_viewport == viewport && predicate(edge) {
                    guard.frame.remove(index);
                    if edge.pressed {
                        if !edge.repeat {
                            pressed = true;
                        }
                    } else {
                        released = true;
                    }
                } else {
                    index += 1;
                }
            }
            (pressed, released)
        })
        .unwrap_or((false, false))
}

/// Return the source-routed physical held state for VK_RETURN, split by the
/// WM_KEY* extended bit (`false` = main Enter, `true` = numpad Enter).
///
/// `None` means this viewport has no subclass-routed input source in the
/// current frame, so callers must not infer a held key from process-global OS
/// state.
pub fn routed_return_key_held(viewport: egui::ViewportId, extended: bool) -> Option<bool> {
    state()
        .lock()
        .ok()
        .and_then(|guard| guard.routed_return_key_held(viewport, extended))
}

fn push_edge(hwnd_raw: u64, raw: RawKeyEdge) {
    let Ok(mut guard) = state().lock() else {
        return;
    };
    let (edge, should_log_unregistered) = guard.enqueue_raw_edge(hwnd_raw, raw);
    drop(guard);
    if should_log_unregistered {
        crate::logger::log(format!(
            "key-input: edge from unregistered HWND routed to ROOT hwnd=0x{hwnd_raw:x}"
        ));
    }
    crate::key_debug::record_raw_edge(crate::key_debug::KeyDebugSource::MainWin32, edge);
}

fn key_state(vk: windows::Win32::UI::Input::KeyboardAndMouse::VIRTUAL_KEY) -> bool {
    // `GetKeyState`, not `GetAsyncKeyState`: this stamps the modifier flags on an
    // edge built from a message, so it must report the state that belonged to
    // that message rather than the state at drain time.
    physical_key_down_from(PhysicalKeySlot::new(vk.0.into(), false), || unsafe {
        GetKeyState(vk.0 as i32) < 0
    })
}

fn key_edge_from_message(msg: u32, wparam: WPARAM, lparam: LPARAM) -> Option<RawKeyEdge> {
    let pressed = matches!(msg, WM_KEYDOWN | WM_SYSKEYDOWN);
    if !pressed && !matches!(msg, WM_KEYUP | WM_SYSKEYUP) {
        return None;
    }
    let raw = lparam.0 as u64;
    Some(RawKeyEdge {
        virtual_key: wparam.0 as u32,
        scan_code: ((raw >> 16) & 0xff) as u16,
        extended: (raw & (1 << 24)) != 0,
        pressed,
        repeat: (raw & (1 << 30)) != 0,
        ctrl: key_state(VK_CONTROL),
        shift: key_state(VK_SHIFT),
        alt: key_state(VK_MENU),
    })
}

unsafe extern "system" fn main_key_input_subclass_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
    _id: usize,
    _ref_data: usize,
) -> LRESULT {
    let hwnd_raw = hwnd.0 as u64;
    if let Some(edge) = key_edge_from_message(msg, wparam, lparam) {
        push_edge(hwnd_raw, edge);
    } else if msg == WM_KILLFOCUS {
        if let Ok(mut guard) = state().lock() {
            // A key-up can be delivered to another HWND after focus moves. Do
            // not let an Enter flavor remain latched in a later frame.
            let viewport = guard
                .installed_hwnds
                .viewport_for_hwnd(hwnd_raw)
                .unwrap_or(egui::ViewportId::ROOT);
            guard.return_keys.clear_viewport(viewport);
        }
    } else if msg == WM_NCDESTROY
        && let Ok(mut guard) = state().lock()
    {
        guard.unregister_hwnd(hwnd_raw);
    }
    unsafe { DefSubclassProc(hwnd, msg, wparam, lparam) }
}

#[cfg(test)]
mod tests {
    use super::{
        KeyEdge, KeyInputState, PhysicalKeySlot, RawKeyEdge, RegisterHwndResult, ReturnKeyState,
        SyntheticInputFact, SyntheticInputIssue, SyntheticInputPlugin, SyntheticKeyCommand,
        SyntheticModifiers, SyntheticNavigationKey, SyntheticRoutingTargetError, TEST_INPUT_LOCK,
        arm_test_synthetic_input, arm_test_synthetic_input_without_registration, begin_frame,
        clear_test_synthetic_input, consume_all_key_down_with_result, consume_key_down,
        consume_key_edges, diagnostic_pressed_key_down_any_viewport,
        enqueue_test_synthetic_command, materialize_test_synthetic_input, physical_key_down,
        physical_key_down_from, pressed_key_down, register_test_synthetic_target,
        resolve_synthetic_routing_target, set_test_frame, set_test_routed_frame,
        set_test_synthetic_repeat, state,
    };
    #[cfg(windows)]
    use super::{
        PreparedPointerDeliveryDisposition, PreparedSyntheticFrame,
        PreparedSyntheticPointerDelivery, PreparedSyntheticPointerStep,
        SyntheticPointerCancelPhase, SyntheticPointerHandlerProof, SyntheticPointerLatch,
        SyntheticPointerModeSignature, SyntheticPointerOwner, SyntheticPointerRegion,
        SyntheticPointerShowTail, SyntheticPointerStep, SyntheticPointerStepKind,
        SyntheticPointerTransaction,
    };
    use std::time::{Duration, Instant};

    struct ClearSyntheticInput;

    impl Drop for ClearSyntheticInput {
        fn drop(&mut self) {
            clear_test_synthetic_input();
        }
    }

    fn raw_input(viewport: egui::ViewportId, time: f64, focused: bool) -> egui::RawInput {
        let mut input = egui::RawInput {
            viewport_id: viewport,
            time: Some(time),
            focused,
            ..Default::default()
        };
        input.viewports.entry(viewport).or_default().focused = Some(focused);
        input
    }

    fn run_input_hook(plugin: &mut SyntheticInputPlugin, input: &mut egui::RawInput) {
        <SyntheticInputPlugin as egui::Plugin>::input_hook(plugin, input);
    }

    fn raw_edge(virtual_key: u32, pressed: bool) -> RawKeyEdge {
        RawKeyEdge {
            virtual_key,
            scan_code: 0x1C,
            extended: false,
            pressed,
            repeat: false,
            ctrl: false,
            shift: false,
            alt: false,
        }
    }

    fn return_edge(extended: bool, pressed: bool) -> KeyEdge {
        KeyEdge {
            source_hwnd: 1,
            source_viewport: egui::ViewportId::ROOT,
            virtual_key: 0x0D,
            scan_code: 0x1C,
            extended,
            pressed,
            repeat: false,
            ctrl: false,
            shift: false,
            alt: false,
        }
    }

    #[test]
    fn return_key_latch_distinguishes_main_and_numpad_enter() {
        let mut state = ReturnKeyState::default();

        state.apply_edge(&return_edge(true, true));
        assert!(!state.is_down(false));
        assert!(state.is_down(true));

        state.apply_edge(&return_edge(false, true));
        assert!(state.is_down(false));
        assert!(state.is_down(true));

        state.apply_edge(&return_edge(true, false));
        assert!(state.is_down(false));
        assert!(!state.is_down(true));

        state.apply_edge(&return_edge(false, false));
        assert!(!state.is_down(false));
        assert!(!state.is_down(true));
    }

    #[test]
    fn return_key_latch_clear_drops_stale_focus_state() {
        let mut state = ReturnKeyState::default();
        state.apply_edge(&return_edge(false, true));
        state.apply_edge(&return_edge(true, true));

        state.clear();

        assert!(!state.is_down(false));
        assert!(!state.is_down(true));
    }

    #[test]
    fn routed_return_key_hold_requires_the_source_viewport_to_be_active() {
        let mut input = KeyInputState::default();
        let source = egui::ViewportId::from_hash_of(3_u64);
        let sibling = egui::ViewportId::from_hash_of(4_u64);
        let mut edge = return_edge(false, true);
        edge.source_viewport = source;
        input.return_keys.apply_edge(source, &edge);

        assert_eq!(input.routed_return_key_held(source, false), None);

        input.frame_active_viewports.push(source);
        assert_eq!(input.routed_return_key_held(source, false), Some(true));
        assert_eq!(input.routed_return_key_held(source, true), Some(false));
        assert_eq!(input.routed_return_key_held(sibling, false), None);
    }

    #[test]
    fn unconsumed_frame_edges_expire_at_next_begin_frame() {
        let _serial = TEST_INPUT_LOCK
            .get_or_init(|| std::sync::Mutex::new(()))
            .lock()
            .expect("key input test lock poisoned");
        set_test_frame(vec![
            KeyEdge {
                source_hwnd: 1,
                source_viewport: egui::ViewportId::ROOT,
                virtual_key: 0x28,
                scan_code: 0x50,
                extended: true,
                pressed: true,
                repeat: false,
                ctrl: true,
                shift: false,
                alt: false,
            },
            KeyEdge {
                source_hwnd: 1,
                source_viewport: egui::ViewportId::ROOT,
                virtual_key: 0x28,
                scan_code: 0x50,
                extended: true,
                pressed: true,
                repeat: false,
                ctrl: true,
                shift: false,
                alt: false,
            },
        ]);

        assert!(consume_key_down(egui::ViewportId::ROOT, true, |edge| {
            edge.virtual_key == 0x28
        }));
        assert!(pressed_key_down(egui::ViewportId::ROOT, |edge| {
            edge.virtual_key == 0x28
        }));

        begin_frame();

        assert!(!pressed_key_down(egui::ViewportId::ROOT, |edge| {
            edge.virtual_key == 0x28
        }));
    }

    #[test]
    fn different_viewport_cannot_consume_source_edge() {
        let _serial = TEST_INPUT_LOCK
            .get_or_init(|| std::sync::Mutex::new(()))
            .lock()
            .expect("key input test lock poisoned");
        let source = egui::ViewportId::from_hash_of("key-source");
        let sibling = egui::ViewportId::from_hash_of("key-sibling");
        let edge = raw_edge(0x25, true).with_source(0x101, source);
        set_test_routed_frame(vec![edge]);

        assert!(!consume_key_down(sibling, true, |_| true));
        assert!(pressed_key_down(source, |_| true));
        assert!(consume_key_down(source, true, |_| true));
    }

    #[test]
    fn cross_viewport_diagnostic_scan_does_not_consume() {
        let _serial = TEST_INPUT_LOCK
            .get_or_init(|| std::sync::Mutex::new(()))
            .lock()
            .unwrap();
        let source = egui::ViewportId::from_hash_of(51_u64);
        let sibling = egui::ViewportId::from_hash_of(52_u64);
        set_test_routed_frame(vec![raw_edge(0x5A, true).with_source(0x102, source)]);

        let observed = diagnostic_pressed_key_down_any_viewport(|e| e.virtual_key == 0x5A);
        assert_eq!(observed.map(|e| e.source_viewport), Some(source));
        assert!(!pressed_key_down(sibling, |e| e.virtual_key == 0x5A));
        assert!(consume_key_down(source, true, |e| e.virtual_key == 0x5A));
    }

    #[test]
    fn hwnd_registration_and_removal_leave_no_stale_mapping_or_edge() {
        let mut input = KeyInputState::default();
        let viewport = egui::ViewportId::from_hash_of("registered-viewport");

        assert_eq!(
            input.register_hwnd(0x201, viewport),
            RegisterHwndResult::Inserted
        );
        assert_eq!(
            input.installed_hwnds.viewport_for_hwnd(0x201),
            Some(viewport)
        );
        input.enqueue_raw_edge(0x201, raw_edge(0x26, true));

        assert_eq!(input.unregister_hwnd(0x201), Some(viewport));
        assert_eq!(input.installed_hwnds.viewport_for_hwnd(0x201), None);
        assert!(input.pending.is_empty());

        let replacement = egui::ViewportId::from_hash_of("replacement-viewport");
        assert_eq!(
            input.register_hwnd(0x201, replacement),
            RegisterHwndResult::Inserted
        );
        assert_eq!(
            input.installed_hwnds.viewport_for_hwnd(0x201),
            Some(replacement)
        );
    }

    #[test]
    fn unregistered_hwnd_edge_is_explicitly_routed_to_root() {
        let mut input = KeyInputState::default();
        let (edge, should_log) = input.enqueue_raw_edge(0x301, raw_edge(0x27, true));

        assert_eq!(edge.source_hwnd, 0x301);
        assert_eq!(edge.source_viewport, egui::ViewportId::ROOT);
        assert!(should_log);
        assert_eq!(input.pending.pop_front(), Some(edge));
        let (_, should_log_again) = input.enqueue_raw_edge(0x301, raw_edge(0x27, false));
        assert!(!should_log_again, "one diagnostic per unregistered HWND");
    }

    #[test]
    fn synthetic_timeline_fans_out_the_same_order_to_win32_and_egui() {
        let _serial = TEST_INPUT_LOCK
            .get_or_init(|| std::sync::Mutex::new(()))
            .lock()
            .expect("key input test lock poisoned");
        let _cleanup = ClearSyntheticInput;
        let viewport = egui::ViewportId::from_hash_of("synthetic-child");
        let hwnd = 0x401;
        arm_test_synthetic_input(hwnd, viewport);
        assert!(set_test_synthetic_repeat(Duration::from_millis(100), 20.0));
        let start = Instant::now() - Duration::from_millis(500);
        enqueue_test_synthetic_command(SyntheticKeyCommand::down(
            start,
            SyntheticNavigationKey::Right,
            SyntheticModifiers::default(),
        ));
        enqueue_test_synthetic_command(SyntheticKeyCommand::up(
            start + Duration::from_millis(160),
            SyntheticNavigationKey::Right,
        ));

        let mut plugin = SyntheticInputPlugin::default();
        let mut root = raw_input(egui::ViewportId::ROOT, 1.0, true);
        root.viewports.entry(viewport).or_default().focused = Some(true);
        run_input_hook(&mut plugin, &mut root);
        assert!(root.events.is_empty());
        let mut child = raw_input(viewport, 1.0, true);
        run_input_hook(&mut plugin, &mut child);

        let edges: Vec<_> = state()
            .lock()
            .expect("key input state poisoned")
            .pending
            .iter()
            .copied()
            .collect();
        assert_eq!(edges.len(), 4);
        assert_eq!(
            edges.iter().map(|edge| edge.pressed).collect::<Vec<_>>(),
            [true, true, true, false]
        );
        assert_eq!(
            edges.iter().map(|edge| edge.repeat).collect::<Vec<_>>(),
            [false, true, true, false]
        );
        assert!(edges.iter().all(|edge| edge.source_hwnd == hwnd));
        assert!(edges.iter().all(|edge| edge.source_viewport == viewport));

        let egui_edges: Vec<_> = child
            .events
            .iter()
            .filter_map(|event| match event {
                egui::Event::Key {
                    key,
                    pressed,
                    repeat,
                    ..
                } => Some((*key, *pressed, *repeat)),
                _ => None,
            })
            .collect();
        assert_eq!(egui_edges.len(), edges.len());
        assert_eq!(
            egui_edges.iter().map(|edge| edge.1).collect::<Vec<_>>(),
            edges.iter().map(|edge| edge.pressed).collect::<Vec<_>>()
        );
        assert!(
            egui_edges
                .iter()
                .all(|edge| edge.0 == egui::Key::ArrowRight)
        );
        assert!(egui_edges.iter().all(|edge| !edge.2));
    }

    #[test]
    fn disarmed_level_reads_the_callers_own_os_source() {
        // The subclass proc stamps edge modifiers with `GetKeyState` so they
        // describe the message being processed, while held-key readers want
        // `GetAsyncKeyState`. Routing both through one hard-coded OS call would
        // silently give queued edges the modifier state at drain time instead.
        let _serial = TEST_INPUT_LOCK
            .get_or_init(|| std::sync::Mutex::new(()))
            .lock()
            .expect("key input test lock poisoned");
        let _cleanup = ClearSyntheticInput;
        clear_test_synthetic_input();

        let mut consulted = 0_u32;
        let level = physical_key_down_from(PhysicalKeySlot::new(0x27, true), || {
            consulted += 1;
            true
        });
        assert!(level, "the disarmed path must return the caller's OS level");
        assert_eq!(consulted, 1, "the caller's OS source must be the only one");

        // Once armed, the timeline replaces the OS source entirely.
        arm_test_synthetic_input(0x403, egui::ViewportId::ROOT);
        let mut armed_consulted = 0_u32;
        let armed_level = physical_key_down_from(PhysicalKeySlot::new(0x27, true), || {
            armed_consulted += 1;
            true
        });
        assert!(!armed_level, "no synthetic key is held yet");
        assert_eq!(
            armed_consulted, 0,
            "an armed timeline must not consult the OS"
        );
    }

    #[test]
    fn synthetic_level_stays_down_between_edges_and_across_frames() {
        let _serial = TEST_INPUT_LOCK
            .get_or_init(|| std::sync::Mutex::new(()))
            .lock()
            .expect("key input test lock poisoned");
        let _cleanup = ClearSyntheticInput;
        arm_test_synthetic_input(0x402, egui::ViewportId::ROOT);
        let start = Instant::now();
        enqueue_test_synthetic_command(SyntheticKeyCommand::down(
            start,
            SyntheticNavigationKey::Down,
            SyntheticModifiers {
                ctrl: true,
                shift: false,
                alt: false,
            },
        ));
        let down = materialize_test_synthetic_input(start, |_| Some(true));
        assert_eq!(down.events.len(), 1);
        assert!(down.events[0].modifiers.ctrl);
        assert!(down.final_modifiers.ctrl);
        assert!(physical_key_down(PhysicalKeySlot::new(0x28, false)));
        assert!(physical_key_down(PhysicalKeySlot::new(0x11, false)));

        begin_frame();
        let between =
            materialize_test_synthetic_input(start + Duration::from_millis(100), |_| Some(true));
        assert!(between.events.is_empty());
        begin_frame();
        assert!(physical_key_down(PhysicalKeySlot::new(0x28, false)));
        assert!(physical_key_down(PhysicalKeySlot::new(0x11, false)));

        let repeats =
            materialize_test_synthetic_input(start + Duration::from_millis(300), |_| Some(true));
        assert!(repeats.events.iter().all(|event| event.repeat));
        begin_frame();
        assert!(physical_key_down(PhysicalKeySlot::new(0x28, false)));

        enqueue_test_synthetic_command(SyntheticKeyCommand::up(
            start + Duration::from_millis(400),
            SyntheticNavigationKey::Down,
        ));
        materialize_test_synthetic_input(start + Duration::from_millis(400), |_| Some(true));
        assert!(!physical_key_down(PhysicalKeySlot::new(0x28, false)));
        assert!(!physical_key_down(PhysicalKeySlot::new(0x11, false)));
    }

    #[test]
    fn synthetic_materialize_catches_up_all_repeats_after_a_long_sleep() {
        let _serial = TEST_INPUT_LOCK
            .get_or_init(|| std::sync::Mutex::new(()))
            .lock()
            .expect("key input test lock poisoned");
        let _cleanup = ClearSyntheticInput;
        arm_test_synthetic_input(0x403, egui::ViewportId::ROOT);
        let start = Instant::now();
        enqueue_test_synthetic_command(SyntheticKeyCommand::down(
            start,
            SyntheticNavigationKey::PageDown,
            SyntheticModifiers::default(),
        ));
        materialize_test_synthetic_input(start, |_| Some(true));
        let first_repeat =
            materialize_test_synthetic_input(start + Duration::from_millis(250), |_| Some(true));
        assert_eq!(first_repeat.events.len(), 1);

        let caught_up =
            materialize_test_synthetic_input(start + Duration::from_millis(710), |_| Some(true));
        assert_eq!(caught_up.events.len(), 13);
        assert!(caught_up.events.iter().all(|event| event.repeat));
        assert!(
            caught_up
                .events
                .windows(2)
                .all(|pair| pair[0].at < pair[1].at)
        );
    }

    #[test]
    fn synthetic_hold_facts_report_levels_edges_and_accumulated_repeats() {
        let _serial = TEST_INPUT_LOCK
            .get_or_init(|| std::sync::Mutex::new(()))
            .lock()
            .expect("key input test lock poisoned");
        let _cleanup = ClearSyntheticInput;
        let viewport = egui::ViewportId::from_hash_of("synthetic-facts");
        arm_test_synthetic_input(0x409, viewport);
        assert!(set_test_synthetic_repeat(Duration::from_millis(250), 30.0));
        let start = Instant::now();
        enqueue_test_synthetic_command(
            SyntheticKeyCommand::down(
                start,
                SyntheticNavigationKey::Right,
                SyntheticModifiers::default(),
            )
            .with_hold_id(7),
        );

        let down = materialize_test_synthetic_input(start, |_| Some(true));
        assert!(down.facts.iter().any(|fact| matches!(
            fact,
            SyntheticInputFact::HoldBegin {
                hold_id: 7,
                key: SyntheticNavigationKey::Right,
                target,
                repeat_delay,
                repeat_hz,
            } if target.viewport == viewport
                && *repeat_delay == Duration::from_millis(250)
                && (*repeat_hz - 30.0).abs() < 0.000_001
        )));
        assert!(down.facts.iter().any(|fact| matches!(
            fact,
            SyntheticInputFact::FrameInput {
                hold_id: 7,
                held: true,
                edge_count: 1,
                materialized_in_frame: 0,
            }
        )));

        let between =
            materialize_test_synthetic_input(start + Duration::from_millis(100), |_| Some(true));
        assert_eq!(
            between.facts,
            [SyntheticInputFact::FrameInput {
                hold_id: 7,
                held: true,
                edge_count: 0,
                materialized_in_frame: 0,
            }]
        );

        let accumulated =
            materialize_test_synthetic_input(start + Duration::from_millis(710), |_| Some(true));
        assert!(accumulated.facts.iter().any(|fact| matches!(
            fact,
            SyntheticInputFact::FrameInput {
                hold_id: 7,
                held: true,
                edge_count,
                materialized_in_frame,
            } if *edge_count > 1 && *materialized_in_frame > 1
        )));

        enqueue_test_synthetic_command(
            SyntheticKeyCommand::up(
                start + Duration::from_millis(711),
                SyntheticNavigationKey::Right,
            )
            .with_hold_id(7),
        );
        let released =
            materialize_test_synthetic_input(start + Duration::from_millis(711), |_| Some(true));
        assert!(released.facts.iter().any(|fact| matches!(
            fact,
            SyntheticInputFact::HoldEnd {
                hold_id: 7,
                down_count: 1,
                repeat_count,
                up_count: 1,
            } if *repeat_count > 1
        )));
        assert!(released.facts.iter().any(|fact| matches!(
            fact,
            SyntheticInputFact::FrameInput {
                hold_id: 7,
                held: false,
                edge_count: 1,
                ..
            }
        )));
    }

    #[test]
    fn synthetic_unregistered_foreground_is_typed_and_retryable() {
        let _serial = TEST_INPUT_LOCK
            .get_or_init(|| std::sync::Mutex::new(()))
            .lock()
            .expect("key input test lock poisoned");
        let _cleanup = ClearSyntheticInput;
        let hwnd = 0x404;
        let viewport = egui::ViewportId::from_hash_of("late-synthetic-target");
        arm_test_synthetic_input_without_registration(hwnd);
        assert_eq!(
            resolve_synthetic_routing_target(),
            Err(SyntheticRoutingTargetError::UnregisteredForegroundWindow { hwnd })
        );
        let start = Instant::now();
        enqueue_test_synthetic_command(SyntheticKeyCommand::down(
            start,
            SyntheticNavigationKey::Home,
            SyntheticModifiers::default(),
        ));
        let waiting = materialize_test_synthetic_input(start, |_| Some(true));
        assert_eq!(
            waiting.issue,
            Some(SyntheticInputIssue::WaitingForRouting(
                SyntheticRoutingTargetError::UnregisteredForegroundWindow { hwnd }
            ))
        );
        assert!(waiting.events.is_empty());

        register_test_synthetic_target(hwnd, viewport);
        let routed = materialize_test_synthetic_input(start, |_| Some(true));
        assert_eq!(routed.events.len(), 1);
        assert_eq!(routed.events[0].target.viewport, viewport);
    }

    #[test]
    fn synthetic_plugin_reinjects_without_double_materializing_same_time() {
        let _serial = TEST_INPUT_LOCK
            .get_or_init(|| std::sync::Mutex::new(()))
            .lock()
            .expect("key input test lock poisoned");
        let _cleanup = ClearSyntheticInput;
        arm_test_synthetic_input(0x405, egui::ViewportId::ROOT);
        enqueue_test_synthetic_command(SyntheticKeyCommand::down(
            Instant::now() - Duration::from_millis(1),
            SyntheticNavigationKey::Enter,
            SyntheticModifiers {
                ctrl: true,
                shift: false,
                alt: false,
            },
        ));

        let mut plugin = SyntheticInputPlugin::default();
        let mut first = raw_input(egui::ViewportId::ROOT, 7.0, true);
        run_input_hook(&mut plugin, &mut first);
        let pending_after_first = state()
            .lock()
            .expect("key input state poisoned")
            .pending
            .len();
        let mut second = raw_input(egui::ViewportId::ROOT, 7.0, true);
        run_input_hook(&mut plugin, &mut second);
        let pending_after_second = state()
            .lock()
            .expect("key input state poisoned")
            .pending
            .len();

        assert_eq!(pending_after_first, 1);
        assert_eq!(pending_after_second, pending_after_first);
        assert_eq!(first.events, second.events);
        assert_eq!(first.events.len(), 1);
        assert!(first.modifiers.ctrl);
        assert!(matches!(
            first.events.as_slice(),
            [egui::Event::Key { modifiers, .. }] if modifiers.ctrl && modifiers.command && !modifiers.mac_cmd
        ));
        assert!(
            state()
                .lock()
                .expect("key input state poisoned")
                .pending
                .front()
                .is_some_and(|edge| edge.ctrl)
        );
    }

    #[test]
    fn synthetic_pending_cap_preserves_repeat_folding_and_release() {
        let _serial = TEST_INPUT_LOCK
            .get_or_init(|| std::sync::Mutex::new(()))
            .lock()
            .expect("key input test lock poisoned");
        let _cleanup = ClearSyntheticInput;
        let viewport = egui::ViewportId::from_hash_of("synthetic-cap");
        arm_test_synthetic_input(0x406, viewport);
        assert!(set_test_synthetic_repeat(Duration::from_millis(1), 1000.0));
        let start = Instant::now();
        enqueue_test_synthetic_command(SyntheticKeyCommand::down(
            start,
            SyntheticNavigationKey::Right,
            SyntheticModifiers::default(),
        ));
        enqueue_test_synthetic_command(SyntheticKeyCommand::up(
            start + Duration::from_millis(400),
            SyntheticNavigationKey::Right,
        ));
        let materialized =
            materialize_test_synthetic_input(start + Duration::from_millis(400), |_| Some(true));
        assert!(materialized.events.len() > super::MAX_PENDING_EVENTS);
        assert_eq!(
            state()
                .lock()
                .expect("key input state poisoned")
                .pending
                .len(),
            super::MAX_PENDING_EVENTS
        );

        begin_frame();
        let result =
            consume_all_key_down_with_result(viewport, true, |edge| edge.virtual_key == 0x27);
        assert_eq!(result.matched_count, super::MAX_PENDING_EVENTS - 1);
        assert_eq!(result.triggered_count, 1);
        assert_eq!(
            consume_key_edges(viewport, |edge| edge.virtual_key == 0x27),
            (false, true)
        );
    }

    #[test]
    fn synthetic_plugin_records_undrawn_child_and_cancels_on_focus_loss() {
        let _serial = TEST_INPUT_LOCK
            .get_or_init(|| std::sync::Mutex::new(()))
            .lock()
            .expect("key input test lock poisoned");
        let _cleanup = ClearSyntheticInput;
        let viewport = egui::ViewportId::from_hash_of("synthetic-focus-child");
        arm_test_synthetic_input(0x407, viewport);
        enqueue_test_synthetic_command(SyntheticKeyCommand::down(
            Instant::now() - Duration::from_millis(1),
            SyntheticNavigationKey::End,
            SyntheticModifiers::default(),
        ));

        let mut undrawn_plugin = SyntheticInputPlugin::default();
        let mut first_root = raw_input(egui::ViewportId::ROOT, 10.0, true);
        first_root.viewports.entry(viewport).or_default().focused = Some(true);
        run_input_hook(&mut undrawn_plugin, &mut first_root);
        let mut next_root = raw_input(egui::ViewportId::ROOT, 11.0, true);
        next_root.viewports.entry(viewport).or_default().focused = Some(true);
        run_input_hook(&mut undrawn_plugin, &mut next_root);
        assert!(undrawn_plugin.issues.iter().any(|issue| matches!(
            issue,
            SyntheticInputIssue::TargetViewportNotRendered {
                viewport: missed,
                event_count: 1,
                ..
            } if *missed == viewport
        )));

        clear_test_synthetic_input();
        arm_test_synthetic_input(0x408, viewport);
        enqueue_test_synthetic_command(SyntheticKeyCommand::down(
            Instant::now() - Duration::from_millis(1),
            SyntheticNavigationKey::End,
            SyntheticModifiers::default(),
        ));
        let mut focus_plugin = SyntheticInputPlugin::default();
        let mut focused_root = raw_input(egui::ViewportId::ROOT, 20.0, true);
        focused_root.viewports.entry(viewport).or_default().focused = Some(true);
        run_input_hook(&mut focus_plugin, &mut focused_root);
        let mut focused_child = raw_input(viewport, 20.0, true);
        run_input_hook(&mut focus_plugin, &mut focused_child);
        assert!(physical_key_down(PhysicalKeySlot::new(0x23, false)));

        let mut blurred_root = raw_input(egui::ViewportId::ROOT, 21.0, true);
        blurred_root.viewports.entry(viewport).or_default().focused = Some(false);
        run_input_hook(&mut focus_plugin, &mut blurred_root);
        let mut blurred_child = raw_input(viewport, 21.0, false);
        run_input_hook(&mut focus_plugin, &mut blurred_child);
        assert!(!physical_key_down(PhysicalKeySlot::new(0x23, false)));
        assert!(matches!(
            blurred_child.events.as_slice(),
            [egui::Event::Key { pressed: false, .. }]
        ));
        assert!(focus_plugin.issues.iter().any(|issue| matches!(
            issue,
            SyntheticInputIssue::FocusLost { viewport: lost } if *lost == viewport
        )));
    }

    #[cfg(windows)]
    fn pointer_owner(
        viewport: egui::ViewportId,
        context_serial: u64,
        backend_token: u64,
    ) -> SyntheticPointerOwner {
        SyntheticPointerOwner {
            identity: crate::test_script::TestScriptWindowIdentity::Detached {
                window_id: context_serial,
                context_serial,
                viewport_id: viewport,
                host_incarnation: context_serial,
                hwnd: 0x5000 + context_serial,
                backend_token,
            },
            items_generation: 7,
        }
    }

    #[cfg(windows)]
    fn pointer_step(
        owner: SyntheticPointerOwner,
        transaction_id: u64,
        step_id: u64,
        kind: SyntheticPointerStepKind,
    ) -> SyntheticPointerStep {
        let rect = egui::Rect::from_min_max(egui::pos2(10.0, 20.0), egui::pos2(110.0, 40.0));
        SyntheticPointerStep {
            step_id,
            latch: SyntheticPointerLatch {
                transaction_id,
                owner,
                region: SyntheticPointerRegion::StillSeekTrack,
                mode: SyntheticPointerModeSignature {
                    spread_mode: "Single".to_string(),
                    reading_flow: "Paged".to_string(),
                    strip_rtl: false,
                    seek_bar_rtl: false,
                    strip_visible: true,
                    strip_locked: true,
                    bar_locked: true,
                },
                region_geometry_token: 11,
                press_page_index: 2,
                press_item_identity: "page-2".to_string(),
                widget_id: egui::Id::new("pointer-track"),
                press_rect: rect,
                coordinate_frame: rect,
                press_pixels_per_point: 1.0,
                press_point: rect.center(),
            },
            kind,
            completion: None,
        }
    }

    #[cfg(windows)]
    #[test]
    fn cleanup_terminal_failure_accepts_only_its_exact_post_environment_ack() {
        let viewport = egui::ViewportId::from_hash_of("pointer-terminal-owner");
        let step = pointer_step(
            pointer_owner(viewport, 17, 23),
            5,
            5_u64 << 32 | 1,
            SyntheticPointerStepKind::CleanupUp {
                final_point: egui::pos2(80.0, 30.0),
            },
        );
        let handle = step.cancel_handle();
        let mut timeline = SyntheticPointerTransaction::Cancelling(
            SyntheticPointerCancelPhase::DeliveredCleanupAwaitingTail(step.clone()),
        );
        let error = timeline
            .finish_delivered_show(SyntheticPointerShowTail {
                step,
                primary_down: true,
                handler: SyntheticPointerHandlerProof::Missing,
            })
            .expect_err("cleanup with primary still down must fail");
        assert!(error.contains("did not release"));
        assert!(timeline.is_terminal_failure(&handle));

        let mut wrong = handle.clone();
        wrong.transaction_id += 1;
        assert!(!timeline.acknowledge_terminal_failure(&wrong));
        assert!(timeline.fail_delivered(
            &handle,
            super::SyntheticPointerFailure::CleanupDidNotRelease,
            error,
        ));
        assert!(timeline.acknowledge_terminal_failure(&handle));
        assert!(timeline.is_idle());
    }

    #[cfg(windows)]
    #[test]
    fn cancelled_down_transport_is_stale_in_a_different_live_show() {
        let _serial = TEST_INPUT_LOCK
            .get_or_init(|| std::sync::Mutex::new(()))
            .lock()
            .expect("key input test lock poisoned");
        let _cleanup = ClearSyntheticInput;
        let viewport = egui::ViewportId::from_hash_of("pointer-stale-transport");
        let old_step = pointer_step(
            pointer_owner(viewport, 19, 29),
            7,
            7_u64 << 32,
            SyntheticPointerStepKind::Down {
                point: egui::pos2(50.0, 30.0),
            },
        );
        let prepared = PreparedSyntheticPointerStep {
            raw_frame: 3,
            raw_time_bits: 1.0_f64.to_bits(),
            step: old_step.clone(),
        };
        {
            let mut input_state = state().lock().expect("key input state poisoned");
            input_state.synthetic.pointer = SyntheticPointerTransaction::Prepared(old_step);
            input_state.synthetic.pointer.cancel();
            assert!(input_state.synthetic.pointer.is_idle());
        }

        let mut plugin = SyntheticInputPlugin {
            prepared: Some(PreparedSyntheticFrame {
                frame_nr: 3,
                time_key: super::RawInputTimeKey::from(Some(1.0)),
                raw_input_time: Some(1.0),
                batches: Vec::new(),
                hold_attributions: Vec::new(),
                delivered_viewports: Vec::new(),
                final_modifiers: egui::Modifiers::NONE,
                armed: true,
                pointer_delivery: PreparedSyntheticPointerDelivery::Pending(prepared),
            }),
            ..Default::default()
        };
        let context = egui::Context::default();
        let backend = eframe::miv_test_script_window_witness::WindowWitnessFixture::new();
        let _backend_scope = backend.enter(&context, viewport, 0x7000);
        let active_witness = eframe::miv_test_script_window_witness::active()
            .expect("fixture must publish an active backend witness");
        let other_owner = pointer_owner(viewport, 20, active_witness.token());
        let _show = crate::test_script::pointer_input::enter_show(Some(
            crate::test_script::pointer_input::ShowOwner {
                identity: other_owner.identity,
                items_generation: other_owner.items_generation,
            },
        ));
        let mut input = raw_input(viewport, 1.005, true);
        plugin.inject_prepared_pointer(&mut input);

        assert!(input.events.is_empty());
        assert!(plugin.issues.is_empty());
        assert!(matches!(
            state()
                .lock()
                .expect("key input state poisoned")
                .synthetic
                .pointer,
            SyntheticPointerTransaction::Idle
        ));
    }

    #[cfg(windows)]
    #[test]
    fn prepared_delivery_rejects_only_the_current_exact_step() {
        let viewport = egui::ViewportId::from_hash_of("pointer-delivery-disposition");
        let current = pointer_step(
            pointer_owner(viewport, 21, 31),
            9,
            9_u64 << 32,
            SyntheticPointerStepKind::Down {
                point: egui::pos2(50.0, 30.0),
            },
        );
        let stale = pointer_step(
            pointer_owner(viewport, 22, 32),
            10,
            10_u64 << 32,
            SyntheticPointerStepKind::Down {
                point: egui::pos2(50.0, 30.0),
            },
        );
        let mut timeline = SyntheticPointerTransaction::Prepared(current.clone());
        assert_eq!(
            timeline.resolve_prepared_delivery(&stale, true),
            PreparedPointerDeliveryDisposition::Stale
        );
        assert!(matches!(timeline, SyntheticPointerTransaction::Prepared(_)));
        assert_eq!(
            timeline.resolve_prepared_delivery(&current, true),
            PreparedPointerDeliveryDisposition::Rejected(current.cancel_handle())
        );
        assert!(
            timeline.is_idle(),
            "an undelivered Down cannot leave a hold"
        );
    }
}
