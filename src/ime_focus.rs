//! IME composition focus policy for egui text fields.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::{Duration, Instant};

const IME_FOCUS_RETRY_ID: &str = "miv_ime_focus_retry";
const IME_TEXT_FOCUS_CONTRACT_ID: &str = "miv_ime_text_focus_contract";
const TEXT_INPUT_KEY_DIAGNOSTIC_ID: &str = "miv_text_input_key_diagnostic";
const TEXT_INPUT_KEY_DIAGNOSTIC_STARTED_ID: &str = "miv_text_input_key_diagnostic_started";
const TEXT_INPUT_KEY_DIAGNOSTIC_ROUTINE_BUDGET_BYTES: usize = 1024 * 1024;
const TEXT_INPUT_KEY_DIAGNOSTIC_LOG_OVERHEAD_BYTES: usize = 64;
const IME_EVENT_GRACE: Duration = Duration::from_millis(300);
static TEXT_INPUT_KEY_DIAGNOSTIC_ROUTINE_BYTES: AtomicUsize = AtomicUsize::new(0);
static TEXT_INPUT_KEY_DIAGNOSTIC_BUDGET_NOTICE_LOGGED: AtomicBool = AtomicBool::new(false);

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct ImeFocusState {
    composing: bool,
    last_event_at: Option<Instant>,
}

impl ImeFocusState {
    fn input_active_at(self, now: Instant) -> bool {
        self.composing
            || self
                .last_event_at
                .is_some_and(|at| now.saturating_duration_since(at) < IME_EVENT_GRACE)
    }
}

/// Pre-`Memory::begin_pass` owner of IME composition state and input repair.
///
/// egui clears keyboard focus when it sees an Escape press in
/// `Focus::begin_pass`, before any application UI runs. During an active IME
/// composition that Escape belongs to the IME, so this plugin removes only the
/// press event (the harmless release is preserved). Windows can then finish the
/// cancellation with `Disabled` but no empty preedit; in that case the plugin
/// inserts the `Preedit("")` expected by `TextEdit` while its composition
/// selection is still intact.
#[derive(Default)]
struct ImeInputPlugin {
    viewports: HashMap<egui::ViewportId, ImeFocusState>,
}

impl ImeInputPlugin {
    fn input_active_in_viewport(&self, viewport_id: egui::ViewportId, now: Instant) -> bool {
        self.viewports
            .get(&viewport_id)
            .copied()
            .unwrap_or_default()
            .input_active_at(now)
    }

    fn composing_in_viewport(&self, viewport_id: egui::ViewportId) -> bool {
        self.viewports
            .get(&viewport_id)
            .is_some_and(|state| state.composing)
    }
}

impl egui::Plugin for ImeInputPlugin {
    fn debug_name(&self) -> &'static str {
        "miv_ime_input_policy"
    }

    fn input_hook(&mut self, input: &mut egui::RawInput) {
        let state = self.viewports.entry(input.viewport_id).or_default();
        normalize_ime_input(&mut input.events, state, Instant::now());
    }
}

/// Install the IME input policy before the first pass of an egui context.
///
/// `Context::add_plugin` is idempotent by plugin type, so test and production
/// setup may safely call this more than once.
pub(crate) fn install_ime_input_policy(ctx: &egui::Context) {
    ctx.add_plugin(ImeInputPlugin::default());
}

fn normalize_ime_input(events: &mut Vec<egui::Event>, state: &mut ImeFocusState, now: Instant) {
    // Only a *non-empty* commit confirms text. Windows ends a cancelled composition
    // with an empty `Commit("")` (observed sequence on Esc:
    // `Disabled, Disabled, Commit(""), Disabled, Disabled`), and egui's commit handler
    // skips `delete_selected` when the prediction is empty, so the preedit stays in the
    // buffer as if it had been confirmed. Treating that as a real commit would suppress
    // the cancel below, which is exactly the bug this normalizer exists to fix.
    let commit_in_input = events.iter().any(
        |event| matches!(event, egui::Event::Ime(egui::ImeEvent::Commit(text)) if !text.is_empty()),
    );
    let mut composing = state.composing;
    let mut saw_ime_event = false;
    let mut normalized = Vec::with_capacity(events.len().saturating_add(1));

    for event in std::mem::take(events) {
        if composing
            && matches!(
                event,
                egui::Event::Key {
                    key: egui::Key::Escape,
                    pressed: true,
                    ..
                }
            )
        {
            continue;
        }

        if let egui::Event::Ime(ime) = &event {
            saw_ime_event = true;
            // A composition ends without text either as `Disabled` or as an empty
            // `Commit("")`, depending on the IME. Whichever arrives first is the point
            // where the preedit is still selected, so that is where the cancel belongs.
            let ends_composition_without_text =
                matches!(ime, egui::ImeEvent::Disabled | egui::ImeEvent::Commit(_))
                    && !matches!(ime, egui::ImeEvent::Commit(text) if !text.is_empty());
            if ends_composition_without_text && composing && !commit_in_input {
                normalized.push(egui::Event::Ime(egui::ImeEvent::Preedit(String::new())));
            }
            apply_ime_event(&mut composing, ime);
        }
        normalized.push(event);
    }

    state.composing = composing;
    if saw_ime_event {
        state.last_event_at = Some(now);
    }
    *events = normalized;
}

fn apply_ime_event(composing: &mut bool, event: &egui::ImeEvent) {
    match event {
        egui::ImeEvent::Enabled => *composing = true,
        egui::ImeEvent::Preedit(text) => *composing = !text.is_empty(),
        egui::ImeEvent::Commit(_) | egui::ImeEvent::Disabled => *composing = false,
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum TextInputDiagnosticKeyPolicy {
    #[default]
    Standard,
    Sensitive,
}

#[derive(Clone, Copy, Debug)]
struct ImeTextFocusContract {
    widget_id: egui::Id,
    focused_pass: u64,
    diagnostic_key_policy: TextInputDiagnosticKeyPolicy,
}

impl Default for ImeTextFocusContract {
    fn default() -> Self {
        Self {
            widget_id: egui::Id::NULL,
            focused_pass: 0,
            diagnostic_key_policy: TextInputDiagnosticKeyPolicy::Standard,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum DiagnosticKeyIdentity {
    Char,
    Named(egui::Key),
}

impl DiagnosticKeyIdentity {
    fn log_label(self) -> String {
        match self {
            Self::Char => "Char".to_owned(),
            Self::Named(key) => format!("{key:?}"),
        }
    }

    fn optional_log_label(key: Option<Self>) -> String {
        match key {
            Some(key) => format!("Some({})", key.log_label()),
            None => "None".to_owned(),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum DiagnosticKeyDetails {
    Standard {
        key: DiagnosticKeyIdentity,
        physical_key: Option<DiagnosticKeyIdentity>,
        modifiers: egui::Modifiers,
    },
    Redacted,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct DiagnosticKeyPress {
    details: DiagnosticKeyDetails,
    repeat: bool,
}

impl DiagnosticKeyPress {
    fn from_event(
        key: egui::Key,
        physical_key: Option<egui::Key>,
        modifiers: egui::Modifiers,
        repeat: bool,
        policy: TextInputDiagnosticKeyPolicy,
    ) -> Self {
        let details = match policy {
            TextInputDiagnosticKeyPolicy::Standard => DiagnosticKeyDetails::Standard {
                key: diagnostic_key_identity(key),
                physical_key: physical_key.map(diagnostic_key_identity),
                modifiers,
            },
            TextInputDiagnosticKeyPolicy::Sensitive => DiagnosticKeyDetails::Redacted,
        };
        Self { details, repeat }
    }
}

#[derive(Clone, Debug)]
struct TextInputKeyDiagnostic {
    pass: u64,
    viewport: egui::ViewportId,
    field_id_before: egui::Id,
    field_id_after: Option<egui::Id>,
    field_seen_after: bool,
    focused_before: Option<egui::Id>,
    owner: Option<crate::keyboard_input::KeyboardOwner>,
    keys: Vec<DiagnosticKeyPress>,
    side_panel_close_sites: Vec<&'static str>,
}

impl Default for TextInputKeyDiagnostic {
    fn default() -> Self {
        Self {
            pass: 0,
            viewport: egui::ViewportId::ROOT,
            field_id_before: egui::Id::NULL,
            field_id_after: None,
            field_seen_after: false,
            focused_before: None,
            owner: None,
            keys: Vec::new(),
            side_panel_close_sites: Vec::new(),
        }
    }
}

fn focus_retry_id(viewport_id: egui::ViewportId, widget_id: egui::Id) -> egui::Id {
    egui::Id::new((IME_FOCUS_RETRY_ID, viewport_id, widget_id))
}

fn text_focus_contract_id(viewport_id: egui::ViewportId) -> egui::Id {
    egui::Id::new((IME_TEXT_FOCUS_CONTRACT_ID, viewport_id))
}

fn text_input_key_diagnostic_id(viewport_id: egui::ViewportId) -> egui::Id {
    egui::Id::new((TEXT_INPUT_KEY_DIAGNOSTIC_ID, viewport_id))
}

fn text_input_key_diagnostic_started_id(viewport_id: egui::ViewportId) -> egui::Id {
    egui::Id::new((TEXT_INPUT_KEY_DIAGNOSTIC_STARTED_ID, viewport_id))
}

fn diagnostic_owner_phase(owner: Option<crate::keyboard_input::KeyboardOwner>) -> &'static str {
    match owner {
        Some(crate::keyboard_input::KeyboardOwner::TextInput { phase, .. }) => match phase {
            crate::keyboard_input::TextInputPhase::PendingFocus => "PendingFocus",
            crate::keyboard_input::TextInputPhase::Focused => "Focused",
            crate::keyboard_input::TextInputPhase::FocusRecovery => "FocusRecovery",
            crate::keyboard_input::TextInputPhase::ImeGrace => "ImeGrace",
        },
        Some(crate::keyboard_input::KeyboardOwner::Modal) => "Modal",
        Some(crate::keyboard_input::KeyboardOwner::FocusedUi { .. }) => "FocusedUi",
        Some(crate::keyboard_input::KeyboardOwner::ApplicationShortcut { .. }) => {
            "ApplicationShortcut"
        }
        Some(crate::keyboard_input::KeyboardOwner::Unclaimed) => "Unclaimed",
        None => "Unresolved",
    }
}

fn key_is_non_character(key: egui::Key) -> bool {
    use egui::Key;

    // This is deliberately a non-character allowlist: any future key variant
    // is masked by default until it is explicitly classified as non-text.
    matches!(
        key,
        Key::ArrowDown
            | Key::ArrowLeft
            | Key::ArrowRight
            | Key::ArrowUp
            | Key::Escape
            | Key::Tab
            | Key::Backspace
            | Key::Enter
            | Key::Insert
            | Key::Delete
            | Key::Home
            | Key::End
            | Key::PageUp
            | Key::PageDown
            | Key::Copy
            | Key::Cut
            | Key::Paste
            | Key::F1
            | Key::F2
            | Key::F3
            | Key::F4
            | Key::F5
            | Key::F6
            | Key::F7
            | Key::F8
            | Key::F9
            | Key::F10
            | Key::F11
            | Key::F12
            | Key::F13
            | Key::F14
            | Key::F15
            | Key::F16
            | Key::F17
            | Key::F18
            | Key::F19
            | Key::F20
            | Key::F21
            | Key::F22
            | Key::F23
            | Key::F24
            | Key::F25
            | Key::F26
            | Key::F27
            | Key::F28
            | Key::F29
            | Key::F30
            | Key::F31
            | Key::F32
            | Key::F33
            | Key::F34
            | Key::F35
            | Key::BrowserBack
    )
}

fn diagnostic_key_identity(key: egui::Key) -> DiagnosticKeyIdentity {
    if !key_is_non_character(key) {
        DiagnosticKeyIdentity::Char
    } else {
        DiagnosticKeyIdentity::Named(key)
    }
}

fn diagnostic_is_anomalous(
    diagnostic: &TextInputKeyDiagnostic,
    focused_after: Option<egui::Id>,
) -> bool {
    diagnostic.field_id_after != Some(diagnostic.field_id_before)
        || !diagnostic.field_seen_after
        || focused_after != Some(diagnostic.field_id_before)
        || diagnostic.owner.is_none()
        || matches!(
            diagnostic.owner,
            Some(
                crate::keyboard_input::KeyboardOwner::ApplicationShortcut { .. }
                    | crate::keyboard_input::KeyboardOwner::Unclaimed
            )
        )
        || !diagnostic.side_panel_close_sites.is_empty()
}

fn reserve_routine_diagnostic_bytes(line_bytes: usize) -> bool {
    TEXT_INPUT_KEY_DIAGNOSTIC_ROUTINE_BYTES
        .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |used| {
            routine_diagnostic_bytes_after(used, line_bytes)
        })
        .is_ok()
}

fn routine_diagnostic_bytes_after(used: usize, line_bytes: usize) -> Option<usize> {
    used.checked_add(line_bytes)
        .and_then(|used| used.checked_add(TEXT_INPUT_KEY_DIAGNOSTIC_LOG_OVERHEAD_BYTES))
        .filter(|next| *next <= TEXT_INPUT_KEY_DIAGNOSTIC_ROUTINE_BUDGET_BYTES)
}

fn log_key_diagnostic(diagnostic: &TextInputKeyDiagnostic, focused_after: Option<egui::Id>) {
    let anomalous = diagnostic_is_anomalous(diagnostic, focused_after);
    for line in format_key_diagnostic(diagnostic, focused_after) {
        if anomalous || reserve_routine_diagnostic_bytes(line.len()) {
            crate::logger::log(line);
        } else if !TEXT_INPUT_KEY_DIAGNOSTIC_BUDGET_NOTICE_LOGGED.swap(true, Ordering::Relaxed) {
            crate::logger::log(
                "[text-input-key] routine diagnostic budget exhausted; further routine records \
                 are suppressed, anomaly records remain enabled",
            );
        }
    }
}

fn format_key_diagnostic(
    diagnostic: &TextInputKeyDiagnostic,
    focused_after: Option<egui::Id>,
) -> Vec<String> {
    let field_id_changed = diagnostic.field_id_after != Some(diagnostic.field_id_before);
    let close_site = if diagnostic.side_panel_close_sites.is_empty() {
        "none".to_owned()
    } else {
        diagnostic.side_panel_close_sites.join("|")
    };
    diagnostic
        .keys
        .iter()
        .map(|key| {
            let (key_label, physical_key_label, modifiers_label) = match key.details {
                DiagnosticKeyDetails::Standard {
                    key,
                    physical_key,
                    modifiers,
                } => (
                    key.log_label(),
                    DiagnosticKeyIdentity::optional_log_label(physical_key),
                    format!("{modifiers:?}"),
                ),
                DiagnosticKeyDetails::Redacted => (
                    "Redacted".to_owned(),
                    "Redacted".to_owned(),
                    "Redacted".to_owned(),
                ),
            };
            format!(
                "[text-input-key] viewport={:?} pass={} key={} physical_key={} \
                 modifiers={} repeat={} field_id={:?} field_id_after={:?} \
                 field_id_changed={} field_seen_after={} focused_before={:?} \
                 focused_after={:?} owner={:?} phase={} side_panel_close={}",
                diagnostic.viewport,
                diagnostic.pass,
                key_label,
                physical_key_label,
                modifiers_label,
                key.repeat,
                diagnostic.field_id_before,
                diagnostic.field_id_after,
                field_id_changed,
                diagnostic.field_seen_after,
                diagnostic.focused_before,
                focused_after,
                diagnostic.owner,
                diagnostic_owner_phase(diagnostic.owner),
                close_site,
            )
        })
        .collect()
}

/// Start the helper-managed text-input diagnostic for this pass.
///
/// A record is created only for key presses when the helper focus contract says
/// that the field is focused now or was focused in the immediately preceding
/// pass. The next pass supplies the post-`end_pass` focus state, after egui's
/// dead-man switch has removed focus from widgets that disappeared.
pub(crate) fn begin_pass_diagnostics(ctx: &egui::Context) {
    let viewport = ctx.viewport_id();
    let pass = ctx.cumulative_pass_nr();
    let mut previous = None;
    let mut contract = None;
    let already_started = ctx.data_mut(|data| {
        let started_id = text_input_key_diagnostic_started_id(viewport);
        if data.get_temp::<u64>(started_id) == Some(pass) {
            return true;
        }
        data.insert_temp(started_id, pass);
        previous =
            data.remove_temp::<TextInputKeyDiagnostic>(text_input_key_diagnostic_id(viewport));
        contract = data.get_temp::<ImeTextFocusContract>(text_focus_contract_id(viewport));
        false
    });
    if already_started {
        return;
    }

    let previous_focused_after = previous
        .as_ref()
        .map(|_| ctx.memory(|memory| memory.focused()));
    if let Some(previous) = previous {
        log_key_diagnostic(&previous, previous_focused_after.flatten());
    }

    let Some(contract) = contract else {
        return;
    };
    if pass.saturating_sub(contract.focused_pass) > 1 {
        return;
    }

    // Do not inspect or allocate the event list unless a helper-managed field
    // is focused now or was focused in the immediately preceding pass.
    let keys = ctx.input(|input| {
        input
            .events
            .iter()
            .filter_map(|event| match event {
                egui::Event::Key {
                    key,
                    physical_key,
                    pressed: true,
                    repeat,
                    modifiers,
                } => Some(DiagnosticKeyPress::from_event(
                    *key,
                    *physical_key,
                    *modifiers,
                    *repeat,
                    contract.diagnostic_key_policy,
                )),
                _ => None,
            })
            .collect::<Vec<_>>()
    });
    if keys.is_empty() {
        return;
    }
    let focused_before =
        previous_focused_after.unwrap_or_else(|| ctx.memory(|memory| memory.focused()));

    ctx.data_mut(|data| {
        data.insert_temp(
            text_input_key_diagnostic_id(viewport),
            TextInputKeyDiagnostic {
                pass,
                viewport,
                field_id_before: contract.widget_id,
                field_id_after: None,
                field_seen_after: false,
                focused_before,
                owner: None,
                keys,
                side_panel_close_sites: Vec::new(),
            },
        );
    });
    // The next pass finalizes `focused_after` after egui's end-pass focus
    // validation, even if this key did not otherwise request another repaint.
    ctx.request_repaint();
}

fn observe_helper_widget(ctx: &egui::Context, widget_id: egui::Id) {
    let viewport = ctx.viewport_id();
    let pass = ctx.cumulative_pass_nr();
    let has_focus = ctx.memory(|memory| memory.has_focus(widget_id));
    ctx.data_mut(|data| {
        let id = text_input_key_diagnostic_id(viewport);
        let Some(mut diagnostic) = data.get_temp::<TextInputKeyDiagnostic>(id) else {
            return;
        };
        if diagnostic.pass != pass {
            return;
        }
        if widget_id == diagnostic.field_id_before {
            diagnostic.field_seen_after = true;
            diagnostic.field_id_after = Some(widget_id);
        } else if has_focus {
            diagnostic.field_id_after = Some(widget_id);
        }
        data.insert_temp(id, diagnostic);
    });
}

pub(crate) fn record_keyboard_owner(
    ctx: &egui::Context,
    owner: crate::keyboard_input::KeyboardOwner,
) {
    let viewport = ctx.viewport_id();
    let pass = ctx.cumulative_pass_nr();
    ctx.data_mut(|data| {
        let id = text_input_key_diagnostic_id(viewport);
        let Some(mut diagnostic) = data.get_temp::<TextInputKeyDiagnostic>(id) else {
            return;
        };
        if diagnostic.pass != pass {
            return;
        }
        diagnostic.owner = Some(owner);
        data.insert_temp(id, diagnostic);
    });
}

pub(crate) fn record_side_panel_close(ctx: &egui::Context, call_site: &'static str) {
    let viewport = ctx.viewport_id();
    let pass = ctx.cumulative_pass_nr();
    ctx.data_mut(|data| {
        let id = text_input_key_diagnostic_id(viewport);
        let Some(mut diagnostic) = data.get_temp::<TextInputKeyDiagnostic>(id) else {
            return;
        };
        if diagnostic.pass != pass || diagnostic.side_panel_close_sites.contains(&call_site) {
            return;
        }
        diagnostic.side_panel_close_sites.push(call_site);
        data.insert_temp(id, diagnostic);
    });
}

/// Sample and return IME activity for the current viewport.
///
/// The pre-input plugin owns viewport-local composition state. This function is
/// a read-only projection for App shortcut gates and TextEdit helpers; it also
/// starts the existing helper-field diagnostics for the current pass.
pub(crate) fn ime_input_active(ctx: &egui::Context) -> bool {
    begin_pass_diagnostics(ctx);
    let viewport_id = ctx.viewport_id();
    let now = Instant::now();
    ctx.with_plugin(|plugin: &mut ImeInputPlugin| plugin.input_active_in_viewport(viewport_id, now))
        .unwrap_or(false)
}

/// Read IME activity for an explicitly selected viewport without consuming its
/// event queue or changing its composition state.
///
/// The plugin updates this shared state before the selected viewport's pass.
/// Cross-viewport consumers may then inspect the same Context-owned snapshot.
pub(crate) fn ime_input_active_in_viewport(
    ctx: &egui::Context,
    viewport_id: egui::ViewportId,
) -> bool {
    let now = Instant::now();
    ctx.with_plugin(|plugin: &mut ImeInputPlugin| plugin.input_active_in_viewport(viewport_id, now))
        .unwrap_or(false)
}

/// Project unprocessed IME events onto the plugin-owned composition snapshot.
///
/// Native presenter events can be queued before its next egui pass. The queue
/// remains the event owner; this read-only projection avoids a second presenter
/// composition flag while still making pre-render clipboard routing exact.
pub(crate) fn ime_composing_with_pending_events(
    ctx: &egui::Context,
    viewport_id: egui::ViewportId,
    pending_events: &[egui::Event],
) -> bool {
    let mut composing = ctx
        .with_plugin(|plugin: &mut ImeInputPlugin| plugin.composing_in_viewport(viewport_id))
        .unwrap_or(false);
    for event in pending_events {
        if let egui::Event::Ime(ime) = event {
            apply_ime_event(&mut composing, ime);
        }
    }
    composing
}

/// Return IME activity including native events queued before the next egui pass.
pub(crate) fn ime_input_active_with_pending_events(
    ctx: &egui::Context,
    viewport_id: egui::ViewportId,
    pending_events: &[egui::Event],
) -> bool {
    pending_events
        .iter()
        .any(|event| matches!(event, egui::Event::Ime(_)))
        || ime_input_active_in_viewport(ctx, viewport_id)
}

/// Return the helper-managed TextEdit that currently owns egui focus.
///
/// The contract is recorded while the widget is actually drawn. Reading it in
/// the next pass lets input handlers that run before panel drawing distinguish
/// a TextEdit from a focused slider without inspecting feature-specific draft
/// state.
pub(crate) fn focused_text_input(ctx: &egui::Context) -> Option<egui::Id> {
    let focused = ctx.memory(|memory| memory.focused())?;
    let pass = ctx.cumulative_pass_nr();
    let id = text_focus_contract_id(ctx.viewport_id());
    ctx.data(|data| {
        data.get_temp::<ImeTextFocusContract>(id)
            .filter(|contract| {
                contract.widget_id == focused && pass.saturating_sub(contract.focused_pass) <= 1
            })
            .map(|contract| contract.widget_id)
    })
}

fn keyboard_focus_recovery_input(ctx: &egui::Context) -> bool {
    ctx.input(|input| {
        let key_pressed = input
            .events
            .iter()
            .any(|event| matches!(event, egui::Event::Key { pressed: true, .. }));
        let pointer_pressed = input.events.iter().any(|event| {
            matches!(
                event,
                egui::Event::PointerButton { pressed: true, .. } | egui::Event::Touch { .. }
            )
        });
        key_pressed && !pointer_pressed
    })
}

fn remember_text_focus(
    ctx: &egui::Context,
    widget_id: egui::Id,
    diagnostic_key_policy: TextInputDiagnosticKeyPolicy,
) {
    let contract = ImeTextFocusContract {
        widget_id,
        focused_pass: ctx.cumulative_pass_nr(),
        diagnostic_key_policy,
    };
    let id = text_focus_contract_id(ctx.viewport_id());
    ctx.data_mut(|data| data.insert_temp(id, contract));
}

fn forget_text_focus(ctx: &egui::Context, widget_id: egui::Id) {
    let id = text_focus_contract_id(ctx.viewport_id());
    ctx.data_mut(|data| {
        if data
            .get_temp::<ImeTextFocusContract>(id)
            .is_some_and(|contract| contract.widget_id == widget_id)
        {
            data.remove_temp::<ImeTextFocusContract>(id);
        }
    });
}

/// Return the helper-managed field whose keyboard-driven focus loss must retain
/// text-input ownership in this pass.
///
/// Ownership is deliberately limited to the pass immediately after the field
/// was observed focused. This covers both IME processing and egui's begin-pass
/// Escape handling, which can clear focus before the fullscreen handler runs.
/// Pointer input and focus already moved to another widget both win.
pub(crate) fn recovering_text_input(ctx: &egui::Context) -> Option<egui::Id> {
    if !keyboard_focus_recovery_input(ctx) || ctx.memory(|memory| memory.focused()).is_some() {
        return None;
    }
    let pass = ctx.cumulative_pass_nr();
    let id = text_focus_contract_id(ctx.viewport_id());
    ctx.data(|data| {
        data.get_temp::<ImeTextFocusContract>(id)
            .filter(|contract| pass.saturating_sub(contract.focused_pass) <= 1)
            .map(|contract| contract.widget_id)
    })
}

/// IME 中のキー入力による `TextEdit` の一時的なフォーカス離脱を防止・復帰する。
///
/// `focus_request` は、呼び出し側が初回フォーカス等に使う request latch があれば渡す。
/// 復帰時は同 latch を次 frame 用に立て直す。latch を持たない欄では widget ID ごとの
/// 一時 latch をこのモジュールが所有する。IME 中でもマウスクリック等、pointer event を
/// 伴う正当なフォーカス移動は復帰しない。アプリ全体の Tab traversal 無効化は
/// `crate::egui_focus_policy` の別責務であり、この field-level lock は IME 候補選択中の
/// focus 保持を独立して保証する。
/// `ime_active` は必ず `TextEdit` 構築前にこのモジュールで採取する。egui は focus 遷移と
/// 同じ pass の Ime event を `TextEdit` 内部で queue から除去することがあり、描画後に読むと
/// fullscreen viewport や native overlay を含む全 context で composition 開始を取り逃がすため。
///
/// # mImageViewer の IME 対応範囲
///
/// コード上 TSF は使用していない。IMM32 を使う経路が 2 つある。
///
/// - main window と fullscreen viewport は winit / egui を通り、`Ime::Preedit` /
///   `Ime::Commit` / `Ime::Disabled` 相当の `egui::ImeEvent` を受ける。
/// - winit 管理外の独立 HWND である native video window は
///   `video/native_window.rs` で `WM_IME_STARTCOMPOSITION` /
///   `WM_IME_COMPOSITION` / `WM_IME_ENDCOMPOSITION` / `WM_IME_SETCONTEXT` を直接処理する。
///   `GCS_COMPSTR` と `GCS_RESULTSTR` を読み、`ISC_SHOWUICOMPOSITIONWINDOW` を落として OS の
///   composition window を抑止する。候補位置は `video/native_window_host.rs` が
///   `ImmSetCompositionWindow` / `ImmSetCandidateWindow` で設定する。
///
/// native 経路は `GCS_COMPATTR` / `GCS_COMPCLAUSE` を読まないため変換対象 clause を区別せず、
/// egui へ渡す preedit 文字列にも caret range がない。`IMR_RECONVERTSTRING` による再変換も
/// 実装していない。main window の message loop は winit が所有するため、TSF 対応は
/// winit / egui 上流の責務になる。所有する 1 HWND だけ TSF 化すると同一アプリ内で IME 挙動が
/// 二重化する一方、本アプリの文字入力は folder path、tag、bookmark name 等の補助用途であるため
/// 採用しない。ただし、このような focus / key routing の不具合は本アプリ側の責務であり、
/// IMM32 の範囲で修正する。「完全な IME には TSF が必要」を未修正の理由にしてはならない。
fn restore_focus_for_ime_key(
    ctx: &egui::Context,
    response: &egui::Response,
    ime_active: bool,
    mut focus_request: Option<&mut bool>,
    diagnostic_key_policy: TextInputDiagnosticKeyPolicy,
) -> bool {
    let viewport_id = ctx.viewport_id();
    let retry_id = focus_retry_id(viewport_id, response.id);
    let retry_requested = match focus_request.as_deref_mut() {
        Some(request) => std::mem::take(request),
        None => ctx
            .data_mut(|data| data.remove_temp::<bool>(retry_id))
            .unwrap_or(false),
    };
    if retry_requested {
        response.request_focus();
    }

    if ctx.memory(|memory| memory.has_focus(response.id)) {
        ctx.memory_mut(|memory| {
            memory.set_focus_lock_filter(
                response.id,
                egui::EventFilter {
                    tab: ime_active,
                    horizontal_arrows: true,
                    vertical_arrows: true,
                    ..Default::default()
                },
            );
        });
    }

    let restored = response.lost_focus() && ime_active && keyboard_focus_recovery_input(ctx);
    if restored {
        response.request_focus();
        if let Some(request) = focus_request {
            *request = true;
        } else {
            ctx.data_mut(|data| data.insert_temp(retry_id, true));
        }
        ctx.request_repaint();
    }

    observe_helper_widget(ctx, response.id);
    if ctx.memory(|memory| memory.has_focus(response.id)) || restored {
        remember_text_focus(ctx, response.id, diagnostic_key_policy);
    } else if response.lost_focus() {
        forget_text_focus(ctx, response.id);
    }

    restored
}

pub(crate) fn show_singleline<'text>(
    ui: &mut egui::Ui,
    text: &'text mut dyn egui::TextBuffer,
    focus_request: Option<&mut bool>,
    configure: impl FnOnce(egui::TextEdit<'text>) -> egui::TextEdit<'text>,
) -> egui::widgets::text_edit::TextEditOutput {
    // `TextEdit` は focus 遷移と同じ pass の Ime event を内部で除去することがある。
    // 必ず widget を描く前に、この viewport の event queue を共有 state へ取り込む。
    let ime_active = ime_input_active(ui.ctx());
    let output = configure(egui::TextEdit::singleline(text)).show(ui);
    let _ = restore_focus_for_ime_key(
        ui.ctx(),
        &output.response,
        ime_active,
        focus_request,
        TextInputDiagnosticKeyPolicy::Standard,
    );
    output
}

pub(crate) fn add_singleline<'text>(
    ui: &mut egui::Ui,
    text: &'text mut dyn egui::TextBuffer,
    focus_request: Option<&mut bool>,
    configure: impl FnOnce(egui::TextEdit<'text>) -> egui::TextEdit<'text>,
) -> egui::Response {
    add_singleline_with_policy(
        ui,
        text,
        focus_request,
        TextInputDiagnosticKeyPolicy::Standard,
        configure,
    )
}

/// Add a helper-managed single-line field whose per-key diagnostic details are
/// fully redacted while focus and keyboard-ownership transitions remain logged.
pub(crate) fn add_singleline_sensitive<'text>(
    ui: &mut egui::Ui,
    text: &'text mut dyn egui::TextBuffer,
    focus_request: Option<&mut bool>,
    configure: impl FnOnce(egui::TextEdit<'text>) -> egui::TextEdit<'text>,
) -> egui::Response {
    add_singleline_with_policy(
        ui,
        text,
        focus_request,
        TextInputDiagnosticKeyPolicy::Sensitive,
        configure,
    )
}

fn add_singleline_with_policy<'text>(
    ui: &mut egui::Ui,
    text: &'text mut dyn egui::TextBuffer,
    focus_request: Option<&mut bool>,
    diagnostic_key_policy: TextInputDiagnosticKeyPolicy,
    configure: impl FnOnce(egui::TextEdit<'text>) -> egui::TextEdit<'text>,
) -> egui::Response {
    let ime_active = ime_input_active(ui.ctx());
    let response = ui.add(configure(egui::TextEdit::singleline(text)));
    let _ = restore_focus_for_ime_key(
        ui.ctx(),
        &response,
        ime_active,
        focus_request,
        diagnostic_key_policy,
    );
    response
}

pub(crate) fn add_sized_singleline<'text>(
    ui: &mut egui::Ui,
    size: egui::Vec2,
    text: &'text mut dyn egui::TextBuffer,
    focus_request: Option<&mut bool>,
    configure: impl FnOnce(egui::TextEdit<'text>) -> egui::TextEdit<'text>,
) -> egui::Response {
    let ime_active = ime_input_active(ui.ctx());
    let response = ui.add_sized(size, configure(egui::TextEdit::singleline(text)));
    let _ = restore_focus_for_ime_key(
        ui.ctx(),
        &response,
        ime_active,
        focus_request,
        TextInputDiagnosticKeyPolicy::Standard,
    );
    response
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::{Path, PathBuf};

    fn test_context() -> egui::Context {
        let ctx = egui::Context::default();
        install_ime_input_policy(&ctx);
        ctx
    }

    fn key_event(key: egui::Key) -> egui::Event {
        key_event_state(key, true)
    }

    fn key_event_state(key: egui::Key, pressed: bool) -> egui::Event {
        egui::Event::Key {
            key,
            physical_key: None,
            pressed,
            repeat: false,
            modifiers: egui::Modifiers::NONE,
        }
    }

    fn viewport_raw_input(
        viewport_id: egui::ViewportId,
        events: Vec<egui::Event>,
    ) -> egui::RawInput {
        let mut input = egui::RawInput {
            viewport_id,
            events,
            ..Default::default()
        };
        input.viewports.insert(
            viewport_id,
            egui::ViewportInfo {
                parent: (viewport_id != egui::ViewportId::ROOT).then_some(egui::ViewportId::ROOT),
                ..Default::default()
            },
        );
        input
    }

    #[test]
    fn key_diagnostic_reports_identity_focus_owner_and_close_site_per_press() {
        let field_id = egui::Id::new("diagnostic-field");
        let diagnostic = TextInputKeyDiagnostic {
            pass: 7,
            viewport: egui::ViewportId::ROOT,
            field_id_before: field_id,
            field_id_after: Some(field_id),
            field_seen_after: true,
            focused_before: Some(field_id),
            owner: Some(crate::keyboard_input::KeyboardOwner::TextInput {
                viewport: egui::ViewportId::ROOT,
                widget_id: field_id,
                phase: crate::keyboard_input::TextInputPhase::Focused,
            }),
            keys: vec![
                DiagnosticKeyPress::from_event(
                    egui::Key::Backspace,
                    Some(egui::Key::Backspace),
                    egui::Modifiers::NONE,
                    false,
                    TextInputDiagnosticKeyPolicy::Standard,
                ),
                DiagnosticKeyPress::from_event(
                    egui::Key::A,
                    Some(egui::Key::A),
                    egui::Modifiers::SHIFT,
                    false,
                    TextInputDiagnosticKeyPolicy::Standard,
                ),
            ],
            side_panel_close_sites: vec![
                "ui_fullscreen::handle_fs_wheel_and_click:hover_auto_dismiss",
            ],
        };

        let lines = format_key_diagnostic(&diagnostic, Some(field_id));
        assert_eq!(lines.len(), 2);
        for line in &lines {
            assert!(line.contains("field_id_changed=false"));
            assert!(line.contains("field_seen_after=true"));
            assert!(line.contains("phase=Focused"));
            assert!(line.contains("focused_before=Some("));
            assert!(line.contains("focused_after=Some("));
            assert!(line.contains(
                "side_panel_close=ui_fullscreen::handle_fs_wheel_and_click:hover_auto_dismiss"
            ));
        }
        assert!(lines[0].contains("key=Backspace physical_key=Some(Backspace)"));
        assert!(lines[1].contains("key=Char physical_key=Some(Char)"));
        let expected_modifiers = format!("modifiers={:?}", egui::Modifiers::SHIFT);
        assert!(lines[1].contains(&expected_modifiers));
        assert!(!lines[1].contains("key=A"));
    }

    #[test]
    fn diagnostic_masks_characters_with_or_without_modifiers() {
        for key in [
            egui::Key::A,
            egui::Key::Num7,
            egui::Key::Space,
            egui::Key::Questionmark,
        ] {
            for modifiers in [
                egui::Modifiers::NONE,
                egui::Modifiers::SHIFT,
                egui::Modifiers {
                    alt: true,
                    ctrl: true,
                    ..Default::default()
                },
            ] {
                let press = DiagnosticKeyPress::from_event(
                    key,
                    Some(key),
                    modifiers,
                    false,
                    TextInputDiagnosticKeyPolicy::Standard,
                );
                assert_eq!(
                    press.details,
                    DiagnosticKeyDetails::Standard {
                        key: DiagnosticKeyIdentity::Char,
                        physical_key: Some(DiagnosticKeyIdentity::Char),
                        modifiers,
                    }
                );
                let diagnostic = TextInputKeyDiagnostic {
                    keys: vec![press],
                    ..Default::default()
                };
                let line = format_key_diagnostic(&diagnostic, None).remove(0);
                assert!(line.contains("key=Char physical_key=Some(Char)"));
            }
        }
    }

    #[test]
    fn diagnostic_keeps_non_character_key_identity_for_standard_fields() {
        let non_character = DiagnosticKeyPress::from_event(
            egui::Key::Backspace,
            Some(egui::Key::Backspace),
            egui::Modifiers::NONE,
            false,
            TextInputDiagnosticKeyPolicy::Standard,
        );
        assert_eq!(
            non_character.details,
            DiagnosticKeyDetails::Standard {
                key: DiagnosticKeyIdentity::Named(egui::Key::Backspace),
                physical_key: Some(DiagnosticKeyIdentity::Named(egui::Key::Backspace)),
                modifiers: egui::Modifiers::NONE,
            }
        );
    }

    #[test]
    fn sensitive_field_redacts_key_details_even_on_anomalous_focus_loss() {
        let ctx = egui::Context::default();
        let field_id = egui::Id::new("sensitive-diagnostic-field");
        let mut password = String::new();
        let mut request_focus = true;

        ctx.begin_pass(Default::default());
        egui::CentralPanel::default().show(&ctx, |ui| {
            let _ = add_singleline_sensitive(ui, &mut password, Some(&mut request_focus), |edit| {
                edit.id(field_id).password(true)
            });
        });
        let _ = ctx.end_pass();
        assert_eq!(ctx.memory(|memory| memory.focused()), Some(field_id));

        let modifiers = egui::Modifiers::SHIFT;
        ctx.begin_pass(egui::RawInput {
            modifiers,
            events: vec![egui::Event::Key {
                key: egui::Key::A,
                physical_key: Some(egui::Key::A),
                pressed: true,
                repeat: false,
                modifiers,
            }],
            ..Default::default()
        });
        begin_pass_diagnostics(&ctx);
        ctx.memory_mut(|memory| memory.surrender_focus(field_id));
        record_keyboard_owner(
            &ctx,
            crate::keyboard_input::KeyboardOwner::TextInput {
                viewport: egui::ViewportId::ROOT,
                widget_id: field_id,
                phase: crate::keyboard_input::TextInputPhase::FocusRecovery,
            },
        );
        record_side_panel_close(&ctx, "sensitive-test-focus-loss");
        let diagnostic_id = text_input_key_diagnostic_id(egui::ViewportId::ROOT);
        let diagnostic: TextInputKeyDiagnostic =
            ctx.data(|data| data.get_temp(diagnostic_id).unwrap_or_default());
        assert!(diagnostic_is_anomalous(&diagnostic, None));
        let lines = format_key_diagnostic(&diagnostic, None);
        let _ = ctx.end_pass();

        assert_eq!(lines.len(), 1);
        let line = &lines[0];
        assert!(line.contains("key=Redacted"));
        assert!(line.contains("physical_key=Redacted"));
        assert!(line.contains("modifiers=Redacted"));
        assert!(!line.contains("key=A"));
        assert!(!line.contains("physical_key=Some(A)"));
        assert!(!line.contains("SHIFT"));
        assert!(line.contains("field_seen_after=false"));
        assert!(line.contains("phase=FocusRecovery"));
        assert!(line.contains("side_panel_close=sensitive-test-focus-loss"));
    }

    #[test]
    fn routine_diagnostic_budget_accepts_the_boundary_and_rejects_overflow() {
        let line_bytes = 400;
        let charge = line_bytes + TEXT_INPUT_KEY_DIAGNOSTIC_LOG_OVERHEAD_BYTES;
        let used_at_boundary = TEXT_INPUT_KEY_DIAGNOSTIC_ROUTINE_BUDGET_BYTES - charge;
        assert_eq!(
            routine_diagnostic_bytes_after(used_at_boundary, line_bytes),
            Some(TEXT_INPUT_KEY_DIAGNOSTIC_ROUTINE_BUDGET_BYTES)
        );
        assert_eq!(
            routine_diagnostic_bytes_after(used_at_boundary + 1, line_bytes),
            None
        );
        assert_eq!(routine_diagnostic_bytes_after(usize::MAX, line_bytes), None);
    }

    #[test]
    fn explicit_viewport_ime_read_is_read_only_and_viewport_isolated() {
        let ctx = test_context();
        let viewport_id = egui::ViewportId::from_hash_of("gamepad-ime-fullscreen");
        ctx.begin_pass(viewport_raw_input(
            viewport_id,
            vec![
                egui::Event::Ime(egui::ImeEvent::Enabled),
                egui::Event::Ime(egui::ImeEvent::Preedit("あ".to_owned())),
            ],
        ));
        assert!(ime_input_active(&ctx));
        let _ = ctx.end_pass();

        let before = ctx
            .with_plugin(|plugin: &mut ImeInputPlugin| {
                plugin.viewports.get(&viewport_id).copied().unwrap()
            })
            .unwrap();
        ctx.begin_pass(viewport_raw_input(egui::ViewportId::ROOT, Vec::new()));
        assert!(ime_input_active_in_viewport(&ctx, viewport_id));
        let after_read = ctx
            .with_plugin(|plugin: &mut ImeInputPlugin| {
                plugin.viewports.get(&viewport_id).copied().unwrap()
            })
            .unwrap();
        assert_eq!(after_read, before);
        assert!(!ime_input_active_in_viewport(&ctx, egui::ViewportId::ROOT));
        assert!(!ime_input_active(&ctx));
        let after_root_update = ctx
            .with_plugin(|plugin: &mut ImeInputPlugin| {
                plugin.viewports.get(&viewport_id).copied().unwrap()
            })
            .unwrap();
        assert_eq!(after_root_update, before);
        let _ = ctx.end_pass();
    }

    #[test]
    fn current_viewport_ime_update_still_tracks_root_composition() {
        let ctx = test_context();
        ctx.begin_pass(viewport_raw_input(
            egui::ViewportId::ROOT,
            vec![
                egui::Event::Ime(egui::ImeEvent::Enabled),
                egui::Event::Ime(egui::ImeEvent::Preedit("あ".to_owned())),
            ],
        ));
        assert!(ime_input_active(&ctx));
        assert!(ime_input_active_in_viewport(&ctx, egui::ViewportId::ROOT));
        let _ = ctx.end_pass();
    }

    #[test]
    fn pending_native_events_project_without_mutating_plugin_state() {
        let ctx = test_context();
        establish_composition(&ctx, egui::ViewportId::ROOT);
        let before = ctx
            .with_plugin(|plugin: &mut ImeInputPlugin| {
                plugin
                    .viewports
                    .get(&egui::ViewportId::ROOT)
                    .copied()
                    .unwrap()
            })
            .unwrap();
        let pending = vec![egui::Event::Ime(egui::ImeEvent::Commit("あ".to_owned()))];

        assert!(!ime_composing_with_pending_events(
            &ctx,
            egui::ViewportId::ROOT,
            &pending,
        ));
        assert!(ime_input_active_with_pending_events(
            &ctx,
            egui::ViewportId::ROOT,
            &pending,
        ));
        let after = ctx
            .with_plugin(|plugin: &mut ImeInputPlugin| {
                plugin
                    .viewports
                    .get(&egui::ViewportId::ROOT)
                    .copied()
                    .unwrap()
            })
            .unwrap();
        assert_eq!(after, before);
    }

    fn establish_composition(ctx: &egui::Context, viewport_id: egui::ViewportId) {
        ctx.begin_pass(viewport_raw_input(
            viewport_id,
            vec![
                egui::Event::Ime(egui::ImeEvent::Enabled),
                egui::Event::Ime(egui::ImeEvent::Preedit("あ".to_owned())),
            ],
        ));
        assert!(ime_input_active(ctx));
        let _ = ctx.end_pass();
    }

    fn empty_preedit_count(ctx: &egui::Context) -> usize {
        ctx.input(|input| {
            input
                .events
                .iter()
                .filter(|event| {
                    matches!(event, egui::Event::Ime(egui::ImeEvent::Preedit(text)) if text.is_empty())
                })
                .count()
        })
    }

    #[test]
    fn composing_escape_press_is_removed_but_release_is_preserved() {
        let ctx = test_context();
        let viewport_id = egui::ViewportId::from_hash_of("ime-escape-filter");
        establish_composition(&ctx, viewport_id);

        ctx.begin_pass(viewport_raw_input(
            viewport_id,
            vec![
                key_event_state(egui::Key::Escape, true),
                key_event_state(egui::Key::Escape, false),
            ],
        ));
        ctx.input(|input| {
            assert_eq!(
                input.events,
                vec![key_event_state(egui::Key::Escape, false)]
            );
        });
        assert_eq!(empty_preedit_count(&ctx), 0);
        let _ = ctx.end_pass();
    }

    #[test]
    fn noncomposing_and_ime_grace_escape_presses_are_preserved() {
        let ctx = test_context();
        let viewport_id = egui::ViewportId::from_hash_of("ime-escape-grace");

        ctx.begin_pass(viewport_raw_input(
            viewport_id,
            vec![key_event(egui::Key::Escape)],
        ));
        assert!(ctx.input(|input| input.events == vec![key_event(egui::Key::Escape)]));
        assert_eq!(empty_preedit_count(&ctx), 0);
        let _ = ctx.end_pass();

        establish_composition(&ctx, viewport_id);
        ctx.begin_pass(viewport_raw_input(
            viewport_id,
            vec![egui::Event::Ime(egui::ImeEvent::Disabled)],
        ));
        assert_eq!(empty_preedit_count(&ctx), 1);
        assert!(ime_input_active(&ctx), "Disabled starts the shortcut grace");
        assert!(
            !ctx.with_plugin(|plugin: &mut ImeInputPlugin| {
                plugin.viewports.get(&viewport_id).unwrap().composing
            })
            .unwrap()
        );
        let mut grace_state = ctx
            .with_plugin(|plugin: &mut ImeInputPlugin| *plugin.viewports.get(&viewport_id).unwrap())
            .unwrap();
        let _ = ctx.end_pass();

        let mut events = vec![key_event(egui::Key::Escape)];
        assert!(
            grace_state.input_active_at(Instant::now()),
            "the 300ms grace is still active"
        );
        normalize_ime_input(&mut events, &mut grace_state, Instant::now());
        assert_eq!(events, vec![key_event(egui::Key::Escape)]);
        assert!(!events.iter().any(
            |event| matches!(event, egui::Event::Ime(egui::ImeEvent::Preedit(text)) if text.is_empty())
        ));
    }

    #[test]
    fn composing_disabled_inserts_one_empty_preedit_before_disabled() {
        let ctx = test_context();
        let viewport_id = egui::ViewportId::from_hash_of("ime-disabled-cancel");
        establish_composition(&ctx, viewport_id);

        ctx.begin_pass(viewport_raw_input(
            viewport_id,
            vec![egui::Event::Ime(egui::ImeEvent::Disabled)],
        ));
        let _ = ime_input_active(&ctx);
        assert_eq!(empty_preedit_count(&ctx), 1);
        ctx.input(|input| {
            assert!(matches!(
                input.events.as_slice(),
                [
                    egui::Event::Ime(egui::ImeEvent::Preedit(text)),
                    egui::Event::Ime(egui::ImeEvent::Disabled),
                ] if text.is_empty()
            ));
        });

        let _ = ime_input_active(&ctx);
        assert_eq!(empty_preedit_count(&ctx), 1);
        let _ = ctx.end_pass();
    }

    /// The sequence Windows actually delivers when Esc cancels a composition,
    /// captured from `[ime-raw]` tracing on 2026-08-01:
    /// `EscPress, Disabled, Disabled, Commit(""), Disabled, Disabled`.
    ///
    /// The empty commit is a cancellation, not a confirmation: egui's commit
    /// handler skips `delete_selected` for an empty prediction, so without the
    /// inserted cancel the preedit stays in the buffer as confirmed text.
    #[test]
    fn windows_escape_cancel_sequence_clears_the_preedit() {
        let ctx = test_context();
        let viewport_id = egui::ViewportId::from_hash_of("ime-windows-escape-cancel");
        establish_composition(&ctx, viewport_id);

        ctx.begin_pass(viewport_raw_input(
            viewport_id,
            vec![
                key_event(egui::Key::Escape),
                egui::Event::Ime(egui::ImeEvent::Disabled),
                egui::Event::Ime(egui::ImeEvent::Disabled),
                egui::Event::Ime(egui::ImeEvent::Commit(String::new())),
                egui::Event::Ime(egui::ImeEvent::Disabled),
                egui::Event::Ime(egui::ImeEvent::Disabled),
            ],
        ));
        let _ = ime_input_active(&ctx);

        assert_eq!(
            empty_preedit_count(&ctx),
            1,
            "the empty commit must not be mistaken for a confirmation"
        );
        ctx.input(|input| {
            assert!(
                !input.events.iter().any(|event| matches!(
                    event,
                    egui::Event::Key {
                        key: egui::Key::Escape,
                        pressed: true,
                        ..
                    }
                )),
                "the cancelling Esc must not reach egui focus handling"
            );
            let first_ime = input
                .events
                .iter()
                .position(|event| matches!(event, egui::Event::Ime(_)))
                .expect("the batch carries IME events");
            assert!(
                matches!(
                    &input.events[first_ime],
                    egui::Event::Ime(egui::ImeEvent::Preedit(text)) if text.is_empty()
                ),
                "the cancel must land while the preedit is still selected"
            );
        });
        let _ = ctx.end_pass();
    }

    /// A non-empty commit is a real confirmation and must be left alone.
    #[test]
    fn non_empty_commit_with_disabled_does_not_insert_cancel_preedit() {
        let ctx = test_context();
        let viewport_id = egui::ViewportId::from_hash_of("ime-real-commit");
        establish_composition(&ctx, viewport_id);

        ctx.begin_pass(viewport_raw_input(
            viewport_id,
            vec![
                egui::Event::Ime(egui::ImeEvent::Commit("確定".to_owned())),
                egui::Event::Ime(egui::ImeEvent::Disabled),
            ],
        ));
        let _ = ime_input_active(&ctx);
        assert_eq!(empty_preedit_count(&ctx), 0);
        let _ = ctx.end_pass();
    }

    #[test]
    fn commit_and_disabled_does_not_insert_cancel_preedit() {
        let ctx = test_context();
        let viewport_id = egui::ViewportId::from_hash_of("ime-commit-disabled");
        establish_composition(&ctx, viewport_id);

        ctx.begin_pass(viewport_raw_input(
            viewport_id,
            vec![
                egui::Event::Ime(egui::ImeEvent::Commit("あ".to_owned())),
                egui::Event::Ime(egui::ImeEvent::Disabled),
            ],
        ));
        let _ = ime_input_active(&ctx);
        assert_eq!(empty_preedit_count(&ctx), 0);
        let _ = ctx.end_pass();
    }

    #[test]
    fn disabled_without_local_composition_preserves_events_and_other_viewport() {
        let ctx = test_context();
        let composing_viewport = egui::ViewportId::from_hash_of("ime-isolated-composing");
        establish_composition(&ctx, composing_viewport);

        ctx.begin_pass(viewport_raw_input(
            egui::ViewportId::ROOT,
            vec![
                key_event(egui::Key::Escape),
                egui::Event::Ime(egui::ImeEvent::Disabled),
            ],
        ));
        let _ = ime_input_active(&ctx);
        assert_eq!(empty_preedit_count(&ctx), 0);
        assert!(ctx.input(|input| input.events.contains(&key_event(egui::Key::Escape))));
        assert!(crate::ime_focus::ime_input_active_in_viewport(
            &ctx,
            composing_viewport,
        ));
        let _ = ctx.end_pass();
    }

    #[test]
    fn raw_multiline_text_edit_keeps_focus_and_cancels_preedit_across_delayed_disabled() {
        let ctx = test_context();
        let field_id = egui::Id::new("ime-escape-raw-multiline");
        let mut body = String::new();
        let mut request_focus = true;
        let draw = |ctx: &egui::Context, body: &mut String, request_focus: &mut bool| {
            egui::CentralPanel::default().show(ctx, |ui| {
                let response = ui.add(egui::TextEdit::multiline(body).id(field_id));
                if std::mem::take(request_focus) {
                    response.request_focus();
                }
            });
        };

        let _ = ctx.run(Default::default(), |ctx| {
            draw(ctx, &mut body, &mut request_focus);
        });
        assert_eq!(ctx.memory(|memory| memory.focused()), Some(field_id));

        let _ = ctx.run(
            viewport_raw_input(
                egui::ViewportId::ROOT,
                vec![
                    egui::Event::Ime(egui::ImeEvent::Enabled),
                    egui::Event::Ime(egui::ImeEvent::Preedit("あ".to_owned())),
                ],
            ),
            |ctx| draw(ctx, &mut body, &mut request_focus),
        );
        assert_eq!(body, "あ");
        assert_eq!(ctx.memory(|memory| memory.focused()), Some(field_id));

        let mut escape_press_seen = true;
        let _ = ctx.run(
            viewport_raw_input(
                egui::ViewportId::ROOT,
                vec![
                    key_event_state(egui::Key::Escape, true),
                    key_event_state(egui::Key::Escape, false),
                ],
            ),
            |ctx| {
                escape_press_seen = ctx.input(|input| {
                    input.events.iter().any(|event| {
                        matches!(
                            event,
                            egui::Event::Key {
                                key: egui::Key::Escape,
                                pressed: true,
                                ..
                            }
                        )
                    })
                });
                draw(ctx, &mut body, &mut request_focus);
            },
        );
        assert!(!escape_press_seen);
        assert_eq!(ctx.memory(|memory| memory.focused()), Some(field_id));

        let mut cancel_preedit_count = 0;
        let _ = ctx.run(
            viewport_raw_input(
                egui::ViewportId::ROOT,
                vec![egui::Event::Ime(egui::ImeEvent::Disabled)],
            ),
            |ctx| {
                cancel_preedit_count = empty_preedit_count(ctx);
                draw(ctx, &mut body, &mut request_focus);
            },
        );
        assert_eq!(cancel_preedit_count, 1);
        assert!(body.is_empty());
        assert_eq!(ctx.memory(|memory| memory.focused()), Some(field_id));
    }

    fn draw_text_fields(
        ctx: &egui::Context,
        first_id: egui::Id,
        second_id: egui::Id,
        first: &mut String,
        second: &mut String,
        first_focus_request: &mut bool,
    ) {
        egui::CentralPanel::default().show(ctx, |ui| {
            let _ = show_singleline(ui, first, Some(first_focus_request), |edit| {
                edit.id(first_id)
            });
            let _ = show_singleline(ui, second, None, |edit| edit.id(second_id));
        });
    }

    #[test]
    fn ime_tab_restores_the_editing_field_focus() {
        let ctx = test_context();
        let first_id = egui::Id::new("ime_tab_first");
        let second_id = egui::Id::new("ime_tab_second");
        let mut first = String::new();
        let mut second = String::new();
        let mut focus_request = true;
        let _ = ctx.run(Default::default(), |ctx| {
            draw_text_fields(
                ctx,
                first_id,
                second_id,
                &mut first,
                &mut second,
                &mut focus_request,
            );
        });

        let _ = ctx.run(
            egui::RawInput {
                events: vec![
                    egui::Event::Ime(egui::ImeEvent::Enabled),
                    egui::Event::Ime(egui::ImeEvent::Preedit("あ".to_owned())),
                ],
                ..Default::default()
            },
            |ctx| {
                draw_text_fields(
                    ctx,
                    first_id,
                    second_id,
                    &mut first,
                    &mut second,
                    &mut focus_request,
                );
            },
        );
        let _ = ctx.run(
            egui::RawInput {
                events: vec![key_event(egui::Key::Tab)],
                ..Default::default()
            },
            |ctx| {
                draw_text_fields(
                    ctx,
                    first_id,
                    second_id,
                    &mut first,
                    &mut second,
                    &mut focus_request,
                );
            },
        );

        assert_eq!(ctx.memory(|memory| memory.focused()), Some(first_id));
        assert!(ctx.wants_keyboard_input());
    }

    #[test]
    fn ime_start_is_sampled_before_text_edit_consumes_events() {
        let ctx = test_context();
        let viewport_id = egui::ViewportId::from_hash_of("ime_focus_fullscreen_viewport");
        let first_id = egui::Id::new("ime_start_focus_gain_first");
        let second_id = egui::Id::new("ime_start_focus_gain_second");
        let mut first = String::new();
        let mut second = String::new();
        let mut focus_request = true;
        let raw_input = |events| {
            let mut input = egui::RawInput {
                viewport_id,
                events,
                ..Default::default()
            };
            input.viewports.insert(
                viewport_id,
                egui::ViewportInfo {
                    parent: Some(egui::ViewportId::ROOT),
                    ..Default::default()
                },
            );
            input
        };

        let _ = ctx.run(raw_input(Vec::new()), |ctx| {
            draw_text_fields(
                ctx,
                first_id,
                second_id,
                &mut first,
                &mut second,
                &mut focus_request,
            );
        });
        let mut ime_events_remained_after_text_edit = true;
        let _ = ctx.run(
            raw_input(vec![
                egui::Event::Ime(egui::ImeEvent::Enabled),
                egui::Event::Ime(egui::ImeEvent::Preedit("あ".to_owned())),
            ]),
            |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    let event_queue = ctx.clone();
                    let _ = show_singleline(ui, &mut first, Some(&mut focus_request), |edit| {
                        // egui TextEdit removes Ime events on a focus transition. Keep that
                        // behavior explicit in this fixture so the helper's sampling order is
                        // tested independently of egui's private focus-history timing.
                        event_queue.input_mut(|input| {
                            input
                                .events
                                .retain(|event| !matches!(event, egui::Event::Ime(_)));
                        });
                        edit.id(first_id)
                    });
                    let _ = show_singleline(ui, &mut second, None, |edit| edit.id(second_id));
                });
                ime_events_remained_after_text_edit = ctx.input(|input| {
                    input
                        .events
                        .iter()
                        .any(|event| matches!(event, egui::Event::Ime(_)))
                });
            },
        );

        assert!(
            !ime_events_remained_after_text_edit,
            "fixture must reproduce TextEdit consuming the IME-start events"
        );
        assert_eq!(ctx.memory(|memory| memory.focused()), Some(first_id));

        let _ = ctx.run(raw_input(vec![key_event(egui::Key::Tab)]), |ctx| {
            draw_text_fields(
                ctx,
                first_id,
                second_id,
                &mut first,
                &mut second,
                &mut focus_request,
            );
        });

        assert_eq!(ctx.memory(|memory| memory.focused()), Some(first_id));
    }

    #[test]
    fn tab_without_ime_keeps_editing_field_focus_and_input_working() {
        let ctx = test_context();
        crate::egui_focus_policy::install_tab_shortcut_focus_policy(&ctx);
        let first_id = egui::Id::new("plain_tab_first");
        let second_id = egui::Id::new("plain_tab_second");
        let mut first = String::new();
        let mut second = String::new();
        let mut focus_request = true;
        let _ = ctx.run(Default::default(), |ctx| {
            draw_text_fields(
                ctx,
                first_id,
                second_id,
                &mut first,
                &mut second,
                &mut focus_request,
            );
        });
        let _ = ctx.run(Default::default(), |ctx| {
            draw_text_fields(
                ctx,
                first_id,
                second_id,
                &mut first,
                &mut second,
                &mut focus_request,
            );
        });
        let _ = ctx.run(
            egui::RawInput {
                events: vec![key_event(egui::Key::Tab)],
                ..Default::default()
            },
            |ctx| {
                draw_text_fields(
                    ctx,
                    first_id,
                    second_id,
                    &mut first,
                    &mut second,
                    &mut focus_request,
                );
            },
        );

        assert_eq!(ctx.memory(|memory| memory.focused()), Some(first_id));

        let _ = ctx.run(
            egui::RawInput {
                events: vec![
                    egui::Event::Text("x".to_owned()),
                    egui::Event::Ime(egui::ImeEvent::Enabled),
                    egui::Event::Ime(egui::ImeEvent::Preedit("あ".to_owned())),
                    egui::Event::Ime(egui::ImeEvent::Commit("あ".to_owned())),
                ],
                ..Default::default()
            },
            |ctx| {
                draw_text_fields(
                    ctx,
                    first_id,
                    second_id,
                    &mut first,
                    &mut second,
                    &mut focus_request,
                );
            },
        );

        assert_eq!(ctx.memory(|memory| memory.focused()), Some(first_id));
        assert_eq!(first, "xあ");
        assert!(second.is_empty());
    }

    #[test]
    fn text_cursor_arrow_keys_keep_the_editing_field_focus() {
        let ctx = egui::Context::default();
        let first_id = egui::Id::new("arrow_first");
        let second_id = egui::Id::new("arrow_second");
        let mut first = String::from("abc");
        let mut second = String::new();
        let mut focus_request = true;
        let _ = ctx.run(Default::default(), |ctx| {
            draw_text_fields(
                ctx,
                first_id,
                second_id,
                &mut first,
                &mut second,
                &mut focus_request,
            );
        });
        let _ = ctx.run(Default::default(), |ctx| {
            draw_text_fields(
                ctx,
                first_id,
                second_id,
                &mut first,
                &mut second,
                &mut focus_request,
            );
        });
        let _ = ctx.run(
            egui::RawInput {
                events: vec![key_event(egui::Key::ArrowDown)],
                ..Default::default()
            },
            |ctx| {
                draw_text_fields(
                    ctx,
                    first_id,
                    second_id,
                    &mut first,
                    &mut second,
                    &mut focus_request,
                );
            },
        );

        assert_eq!(ctx.memory(|memory| memory.focused()), Some(first_id));
    }

    #[test]
    fn ime_activity_without_a_focus_key_does_not_undo_focus_change() {
        let ctx = test_context();
        let first_id = egui::Id::new("ime_click_first");
        let second_id = egui::Id::new("ime_click_second");
        let mut first = String::new();
        let mut second = String::new();
        let mut focus_request = true;
        let _ = ctx.run(Default::default(), |ctx| {
            draw_text_fields(
                ctx,
                first_id,
                second_id,
                &mut first,
                &mut second,
                &mut focus_request,
            );
        });
        ctx.memory_mut(|memory| memory.request_focus(second_id));
        let _ = ctx.run(
            egui::RawInput {
                events: vec![
                    egui::Event::Ime(egui::ImeEvent::Enabled),
                    egui::Event::Ime(egui::ImeEvent::Preedit("あ".to_owned())),
                ],
                ..Default::default()
            },
            |ctx| {
                draw_text_fields(
                    ctx,
                    first_id,
                    second_id,
                    &mut first,
                    &mut second,
                    &mut focus_request,
                );
            },
        );

        assert_eq!(ctx.memory(|memory| memory.focused()), Some(second_id));
    }

    #[test]
    fn ime_enter_and_escape_restore_the_editing_field_focus() {
        for key in [egui::Key::Enter, egui::Key::Escape] {
            let ctx = test_context();
            let first_id = egui::Id::new(("ime_focus_key_first", key));
            let second_id = egui::Id::new(("ime_focus_key_second", key));
            let mut first = String::new();
            let mut second = String::new();
            let mut focus_request = true;
            let _ = ctx.run(Default::default(), |ctx| {
                draw_text_fields(
                    ctx,
                    first_id,
                    second_id,
                    &mut first,
                    &mut second,
                    &mut focus_request,
                );
            });
            let terminal_ime_event = if key == egui::Key::Enter {
                egui::Event::Ime(egui::ImeEvent::Commit("あ".to_owned()))
            } else {
                egui::Event::Ime(egui::ImeEvent::Disabled)
            };
            let _ = ctx.run(
                egui::RawInput {
                    events: vec![
                        egui::Event::Ime(egui::ImeEvent::Enabled),
                        egui::Event::Ime(egui::ImeEvent::Preedit("あ".to_owned())),
                        key_event(key),
                        terminal_ime_event,
                    ],
                    ..Default::default()
                },
                |ctx| {
                    draw_text_fields(
                        ctx,
                        first_id,
                        second_id,
                        &mut first,
                        &mut second,
                        &mut focus_request,
                    );
                },
            );

            assert_eq!(ctx.memory(|memory| memory.focused()), Some(first_id));
        }
    }

    struct RawTextEditExemption {
        path: &'static str,
        line_anchor: &'static str,
        expected_occurrences: usize,
        reason: &'static str,
    }

    const RAW_TEXT_EDIT_EXEMPTIONS: &[RawTextEditExemption] = &[
        RawTextEditExemption {
            path: "src/ime_focus.rs",
            line_anchor: "let output = configure(",
            expected_occurrences: 1,
            reason: "共有 helper 自身が raw TextEdit を構築する境界",
        },
        RawTextEditExemption {
            path: "src/ime_focus.rs",
            line_anchor: "let response = ui.add(configure(",
            expected_occurrences: 1,
            reason: "共有 helper 自身が raw TextEdit を構築する境界",
        },
        RawTextEditExemption {
            path: "src/ime_focus.rs",
            line_anchor: "let response = ui.add_sized(size, configure(",
            expected_occurrences: 1,
            reason: "共有 helper 自身が raw TextEdit を構築する境界",
        },
        RawTextEditExemption {
            path: "src/ime_focus.rs",
            line_anchor: "multiline(body).id(field_id)",
            expected_occurrences: 1,
            reason: "pass 前 IME plugin を raw 複数行 editor で直接検証する test fixture",
        },
        RawTextEditExemption {
            path: "src/egui_focus_policy.rs",
            line_anchor: "singleline(first)",
            expected_occurrences: 1,
            reason: "アプリ全体の Tab focus policy を直接検証する低レベル test fixture",
        },
        RawTextEditExemption {
            path: "src/egui_focus_policy.rs",
            line_anchor: "singleline(second)",
            expected_occurrences: 1,
            reason: "アプリ全体の Tab focus policy を直接検証する低レベル test fixture",
        },
        RawTextEditExemption {
            path: "src/keymap.rs",
            line_anchor: "singleline(&mut text)",
            expected_occurrences: 4,
            reason: "keymap の TextEdit 入力抑止を直接検証する低レベル test fixture",
        },
        RawTextEditExemption {
            path: "src/keymap.rs",
            line_anchor: "singleline(&mut first)",
            expected_occurrences: 2,
            reason: "keymap とアプリ全体の Tab focus policy の境界を直接検証する test fixture",
        },
        RawTextEditExemption {
            path: "src/keymap.rs",
            line_anchor: "singleline(&mut second)",
            expected_occurrences: 2,
            reason: "keymap とアプリ全体の Tab focus policy の境界を直接検証する test fixture",
        },
        RawTextEditExemption {
            path: "src/ui_main.rs",
            line_anchor: "singleline(&mut self.color_filter.hex_input)",
            expected_occurrences: 1,
            reason: "RRGGBB 専用の ASCII 制約入力で日本語自由入力ではない",
        },
        RawTextEditExemption {
            path: "src/ui_main.rs",
            line_anchor: "singleline(&mut self.address)",
            expected_occurrences: 1,
            reason: "snapshot 表示中だけ描く disabled 読み取り専用欄",
        },
        RawTextEditExemption {
            path: "src/ui_text.rs",
            line_anchor: "multiline(&mut t.text)",
            expected_occurrences: 1,
            reason: "Enter 改行と caret 操作を所有する注釈本文の複数行 editor",
        },
        RawTextEditExemption {
            path: "src/ui_dialogs/cache_manager.rs",
            line_anchor: "singleline(&mut days_str)",
            expected_occurrences: 1,
            reason: "u32 日数だけを受け付ける数値入力",
        },
        RawTextEditExemption {
            path: "src/ui_dialogs/preferences/pages.rs",
            line_anchor: "singleline(&mut state.command_key_filter)",
            expected_occurrences: 1,
            reason: "F11 等のキー記法だけを検索する ASCII 入力",
        },
        RawTextEditExemption {
            path: "src/ui_dialogs/preferences/pages.rs",
            line_anchor: "singleline(input).hint_text(if idx == 0",
            expected_occurrences: 1,
            reason: "Ctrl+F 等の key chord grammar 専用入力",
        },
        RawTextEditExemption {
            path: "src/ui_dialogs/preferences/pages.rs",
            line_anchor: concat!(".text_edit_", "singleline(&mut state.exif_add_tag_input)"),
            expected_occurrences: 1,
            reason: "EXIF の ASCII 内部 tag 名専用入力",
        },
        RawTextEditExemption {
            path: "src/video/native_presenter/overlay_draw.rs",
            line_anchor: "multiline(&mut state.textarea)",
            expected_occurrences: 1,
            reason: "時刻付き行の paste と独自 focus lifecycle を持つ複数行 editor",
        },
    ];

    fn collect_rust_sources(dir: &Path, files: &mut Vec<PathBuf>) {
        for entry in std::fs::read_dir(dir).expect("read src directory") {
            let entry = entry.expect("read src entry");
            let path = entry.path();
            if path.is_dir() {
                collect_rust_sources(&path, files);
            } else if path.extension().and_then(|ext| ext.to_str()) == Some("rs") {
                files.push(path);
            }
        }
    }

    #[test]
    fn raw_text_edits_are_ime_aware_or_explicitly_exempt() {
        let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        let src_dir = manifest_dir.join("src");
        let mut files = Vec::new();
        collect_rust_sources(&src_dir, &mut files);
        files.sort();

        let raw_patterns = [
            ["TextEdit::", "singleline("].concat(),
            ["TextEdit::", "multiline("].concat(),
            [".text_edit_", "singleline("].concat(),
            [".text_edit_", "multiline("].concat(),
        ];
        let mut exemption_matches = vec![0usize; RAW_TEXT_EDIT_EXEMPTIONS.len()];
        let mut violations = Vec::new();

        for path in files {
            let relative = path
                .strip_prefix(&manifest_dir)
                .expect("source under manifest directory")
                .to_string_lossy()
                .replace('\\', "/");
            let source = std::fs::read_to_string(&path).expect("read Rust source as UTF-8");
            let lines: Vec<&str> = source.lines().collect();
            for (line_idx, line) in lines.iter().enumerate() {
                if !raw_patterns.iter().any(|pattern| line.contains(pattern)) {
                    continue;
                }
                let matches: Vec<usize> = RAW_TEXT_EDIT_EXEMPTIONS
                    .iter()
                    .enumerate()
                    .filter_map(|(idx, exemption)| {
                        (exemption.path == relative
                            && line.contains(exemption.line_anchor)
                            && !exemption.reason.trim().is_empty())
                        .then_some(idx)
                    })
                    .collect();
                match matches.as_slice() {
                    [idx] => exemption_matches[*idx] += 1,
                    [] => violations.push(format!("{relative}:{}: {}", line_idx + 1, line.trim())),
                    _ => violations.push(format!(
                        "{relative}:{}: 複数の exemption に一致: {}",
                        line_idx + 1,
                        line.trim()
                    )),
                }
            }
        }

        for (idx, exemption) in RAW_TEXT_EDIT_EXEMPTIONS.iter().enumerate() {
            assert_eq!(
                exemption_matches[idx], exemption.expected_occurrences,
                "IME raw TextEdit exemption が現ソースと一致しません: {} / {} ({})",
                exemption.path, exemption.line_anchor, exemption.reason
            );
        }
        assert!(
            violations.is_empty(),
            "IME focus policy を通らない raw TextEdit があります:\n{}\n自由入力欄は \
             crate::ime_focus の helper を使ってください。意図的に対象外とする場合は、\
             ime_focus.rs の RAW_TEXT_EDIT_EXEMPTIONS に短い理由付きで追加してください。",
            violations.join("\n")
        );
    }
}
