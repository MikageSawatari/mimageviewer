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
        }
    }

    pub const fn up(at: Instant, key: SyntheticNavigationKey) -> Self {
        Self {
            at,
            kind: SyntheticKeyCommandKind::Up { key },
        }
    }

    pub const fn cancel_all(at: Instant) -> Self {
        Self {
            at,
            kind: SyntheticKeyCommandKind::CancelAll,
        }
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
    next_repeat_at: Instant,
    repeat_interval: Duration,
    order: u64,
}

#[derive(Clone, Copy, Debug)]
struct MaterializedSyntheticEvent {
    at: Instant,
    key: SyntheticNavigationKey,
    target: SyntheticRoutingTarget,
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

#[derive(Debug)]
struct SyntheticMaterialization {
    armed: bool,
    events: Vec<MaterializedSyntheticEvent>,
    final_modifiers: SyntheticModifiers,
    issue: Option<SyntheticInputIssue>,
}

impl SyntheticMaterialization {
    fn disarmed() -> Self {
        Self {
            armed: false,
            events: Vec::new(),
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
        }
    }
}

impl SyntheticTimeline {
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
            events.push(MaterializedSyntheticEvent {
                at,
                key: held.key,
                target: held.target,
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
        at: Instant,
        events: &mut Vec<MaterializedSyntheticEvent>,
    ) {
        let Some(index) = self.held.iter().position(|held| held.key == key) else {
            return;
        };
        let modifiers = self.modifiers();
        let held = self.held.remove(index);
        events.push(MaterializedSyntheticEvent {
            at,
            key: held.key,
            target: held.target,
            pressed: false,
            repeat: false,
            modifiers,
        });
    }

    fn cancel_all_at(&mut self, at: Instant, events: &mut Vec<MaterializedSyntheticEvent>) {
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
                pressed: false,
                repeat: false,
                modifiers,
            });
        }
    }

    fn materialize<F, R>(
        &mut self,
        now: Instant,
        mut resolve: R,
        mut focused: F,
    ) -> SyntheticMaterialization
    where
        F: FnMut(egui::ViewportId) -> Option<bool>,
        R: FnMut() -> Result<SyntheticRoutingTarget, SyntheticRoutingTargetError>,
    {
        if !self.armed {
            return SyntheticMaterialization::disarmed();
        }

        let mut events = Vec::new();
        if let Some(lost) = self
            .held
            .iter()
            .find(|held| focused(held.target.viewport) == Some(false))
            .map(|held| held.target.viewport)
        {
            self.cancel_all_at(now, &mut events);
            self.commands.clear();
            return SyntheticMaterialization {
                armed: true,
                events,
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
                        next_repeat_at: queued.command.at + self.repeat_delay,
                        repeat_interval: self.repeat_interval,
                        order: queued.sequence,
                    });
                    events.push(MaterializedSyntheticEvent {
                        at: queued.command.at,
                        key,
                        target,
                        pressed: true,
                        repeat: false,
                        modifiers: self.modifiers(),
                    });
                }
                SyntheticKeyCommandKind::Up { key } => {
                    self.commands.pop_front();
                    self.release_key(key, queued.command.at, &mut events);
                }
                SyntheticKeyCommandKind::CancelAll => {
                    self.commands.pop_front();
                    self.cancel_all_at(queued.command.at, &mut events);
                }
            }
        }
        if issue.is_none() {
            self.emit_repeats_through(now, &mut events);
        }

        SyntheticMaterialization {
            armed: true,
            events,
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

#[derive(Debug)]
struct PreparedSyntheticFrame {
    time_key: RawInputTimeKey,
    raw_input_time: Option<f64>,
    batches: Vec<SyntheticViewportBatch>,
    delivered_viewports: Vec<egui::ViewportId>,
    final_modifiers: egui::Modifiers,
    armed: bool,
}

#[derive(Default)]
struct SyntheticInputPlugin {
    prepared: Option<PreparedSyntheticFrame>,
    issues: VecDeque<SyntheticInputIssue>,
}

impl SyntheticInputPlugin {
    fn record_issue(&mut self, issue: SyntheticInputIssue) {
        while self.issues.len() >= MAX_SYNTHETIC_INPUT_ISSUES {
            self.issues.pop_front();
        }
        self.issues.push_back(issue);
    }

    fn record_undelivered_previous_frame(&mut self) {
        let Some(prepared) = self.prepared.as_ref() else {
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
        let materialized = state()
            .lock()
            .map(|mut guard| {
                guard.materialize_synthetic(Instant::now(), |viewport| {
                    if viewport == input.viewport_id {
                        Some(input.focused)
                    } else {
                        input.viewports.get(&viewport).and_then(|info| info.focused)
                    }
                })
            })
            .unwrap_or_else(|_| SyntheticMaterialization::disarmed());
        debug_assert!(
            materialized
                .events
                .windows(2)
                .all(|pair| pair[0].at <= pair[1].at),
            "synthetic input materialization must stay chronological"
        );

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
            time_key,
            raw_input_time: input.time,
            batches,
            delivered_viewports: Vec::new(),
            final_modifiers: materialized.final_modifiers.to_egui(),
            armed: materialized.armed,
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
}

impl egui::Plugin for SyntheticInputPlugin {
    fn debug_name(&self) -> &'static str {
        "miv_synthetic_input"
    }

    fn input_hook(&mut self, input: &mut egui::RawInput) {
        if input.viewport_id == egui::ViewportId::ROOT {
            self.prepare_root(input);
        }
        self.inject_prepared(input);
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
fn set_test_routed_frame(edges: Vec<KeyEdge>) {
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
        guard.synthetic.armed = true;
        guard.test_foreground_hwnd = Some(foreground_hwnd);
        guard.register_hwnd(foreground_hwnd, viewport);
    }
}

#[cfg(test)]
pub fn arm_test_synthetic_input_without_registration(foreground_hwnd: u64) {
    if let Ok(mut guard) = state().lock() {
        *guard = KeyInputState::default();
        guard.synthetic.armed = true;
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
    if !hz.is_finite() || hz <= 0.0 {
        return false;
    }
    let interval = Duration::from_secs_f64(1.0 / hz);
    if interval.is_zero() {
        return false;
    }
    if let Ok(mut guard) = state().lock() {
        guard.synthetic.repeat_delay = delay;
        guard.synthetic.repeat_interval = interval;
        true
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
        SyntheticInputIssue, SyntheticInputPlugin, SyntheticKeyCommand, SyntheticModifiers,
        SyntheticNavigationKey, SyntheticRoutingTargetError, TEST_INPUT_LOCK,
        arm_test_synthetic_input, arm_test_synthetic_input_without_registration, begin_frame,
        clear_test_synthetic_input, consume_all_key_down_with_result, consume_key_down,
        consume_key_edges, enqueue_test_synthetic_command, materialize_test_synthetic_input,
        physical_key_down, physical_key_down_from, pressed_key_down,
        register_test_synthetic_target, resolve_synthetic_routing_target, set_test_frame,
        set_test_routed_frame, set_test_synthetic_repeat, state,
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
}
