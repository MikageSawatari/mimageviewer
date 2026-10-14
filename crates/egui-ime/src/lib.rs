//! Host-independent IME input repair and composition focus policy for egui.
//!
//! Install the input policy before the first pass of each Context. Hosts retain
//! their own shortcut ownership, diagnostics and application-wide focus policy.

mod tab_focus;

pub use tab_focus::{cancel_tab_focus_traversal, install_tab_shortcut_focus_policy};

use std::collections::HashMap;

const IME_FOCUS_RETRY_ID: &str = "miv_ime_focus_retry";
const IME_TEXT_FOCUS_CONTRACT_ID: &str = "miv_ime_text_focus_contract";

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct ImeFocusState {
    composing: bool,
}

/// Pre-`Memory::begin_pass` owner of IME composition state and input repair.
///
/// The host input bridge excludes key presses handled by the IME (winit's
/// `NamedKey::Process`); their releases and IME events still arrive here. Windows
/// can finish cancellation with `Disabled` but no empty preedit. In that case
/// this plugin inserts the `Preedit("")` expected by `TextEdit` while its
/// composition selection is still intact. Ordinary key events are preserved.
#[derive(Default)]
struct ImeInputPlugin {
    viewports: HashMap<egui::ViewportId, ImeFocusState>,
}

impl ImeInputPlugin {
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
        // RawInput lists all live viewports. A destroyed viewport must not leave
        // composition behind when its id is reused without a terminal event.
        self.viewports
            .retain(|id, _| input.viewports.contains_key(id));
        let state = self.viewports.entry(input.viewport_id).or_default();
        normalize_ime_input(&mut input.events, state);
        if !input.focused {
            state.composing = false;
        }
    }
}

/// Install the IME input policy before the first pass of an egui context.
///
/// `Context::add_plugin` is idempotent by plugin type, so test and production
/// setup may safely call this more than once.
pub fn install_ime_input_policy(ctx: &egui::Context) {
    ctx.add_plugin(ImeInputPlugin::default());
}

fn normalize_ime_input(events: &mut Vec<egui::Event>, state: &mut ImeFocusState) {
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
    let mut normalized = Vec::with_capacity(events.len().saturating_add(1));

    for event in std::mem::take(events) {
        if matches!(event, egui::Event::WindowFocused(false)) {
            composing = false;
        }
        if let egui::Event::Ime(ime) = &event {
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
/// Host diagnostic policy recorded with the focus contract; no keys are logged here.
pub enum TextInputDiagnosticKeyPolicy {
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

fn focus_retry_id(viewport_id: egui::ViewportId, widget_id: egui::Id) -> egui::Id {
    egui::Id::new((IME_FOCUS_RETRY_ID, viewport_id, widget_id))
}

fn text_focus_contract_id(viewport_id: egui::ViewportId) -> egui::Id {
    egui::Id::new((IME_TEXT_FOCUS_CONTRACT_ID, viewport_id))
}

/// A read-only copy of the helper-managed focus contract for host diagnostics.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TextFocusContractSnapshot {
    pub widget_id: egui::Id,
    pub focused_pass: u64,
    pub diagnostic_key_policy: TextInputDiagnosticKeyPolicy,
}

/// Read the current viewport's contract, including a recently unfocused field.
///
/// The host chooses the allowed pass age. Reading this snapshot neither updates
/// the contract nor starts diagnostics or examines another viewport's events.
pub fn text_focus_contract_snapshot(ctx: &egui::Context) -> Option<TextFocusContractSnapshot> {
    let id = text_focus_contract_id(ctx.viewport_id());
    ctx.data(|data| {
        data.get_temp::<ImeTextFocusContract>(id)
            .map(|contract| TextFocusContractSnapshot {
                widget_id: contract.widget_id,
                focused_pass: contract.focused_pass,
                diagnostic_key_policy: contract.diagnostic_key_policy,
            })
    })
}

/// Return whether the current viewport is composing after this pass's IME events.
///
/// The pre-input plugin owns viewport-local composition state. This function is
/// a read-only projection for App shortcut gates and TextEdit helpers; it does not
/// start host diagnostics or consume input events.
pub fn ime_input_active(ctx: &egui::Context) -> bool {
    let viewport_id = ctx.viewport_id();
    ctx.with_plugin(|plugin: &mut ImeInputPlugin| plugin.composing_in_viewport(viewport_id))
        .unwrap_or(false)
}

/// Read IME activity for an explicitly selected viewport without consuming its
/// event queue or changing its composition state.
///
/// The plugin updates this shared state before the selected viewport's pass.
/// Cross-viewport consumers may then inspect the same Context-owned snapshot.
pub fn ime_input_active_in_viewport(ctx: &egui::Context, viewport_id: egui::ViewportId) -> bool {
    ctx.with_plugin(|plugin: &mut ImeInputPlugin| plugin.composing_in_viewport(viewport_id))
        .unwrap_or(false)
}

/// Project unprocessed IME events onto the plugin-owned composition snapshot.
///
/// Native presenter events can be queued before its next egui pass. The queue
/// remains the event owner; this read-only projection avoids a second presenter
/// composition flag while still making pre-render clipboard routing exact.
pub fn ime_composing_with_pending_events(
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

/// Return the helper-managed TextEdit that currently owns egui focus.
///
/// The contract is recorded while the widget is actually drawn. Reading it in
/// the next pass lets input handlers that run before panel drawing distinguish
/// a TextEdit from a focused slider without inspecting feature-specific draft
/// state.
pub fn focused_text_input(ctx: &egui::Context) -> Option<egui::Id> {
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
pub fn recovering_text_input(ctx: &egui::Context) -> Option<egui::Id> {
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
/// helper は呼び出し側の latch を書き換えず、viewport / widget ID ごとの次 pass 限りの
/// retry を所有する。変換終了後の通常 Enter / Escape は focus を手放す。pointer event を
/// 伴う正当なフォーカス移動は復帰しない。IME が処理したキーの press は入力 bridge が
/// 渡さず、アプリ全体の Tab traversal 無効化はホストの別責務である。
/// この helper は通常のカーソル矢印を field 内に保持し、keyboard focus loss を復帰する。
/// `ime_active` は必ず `TextEdit` 構築前にこのモジュールで採取する。egui は focus 遷移と
/// 同じ pass の Ime event を `TextEdit` 内部で queue から除去することがあり、描画後に読むと
/// fullscreen viewport や native overlay を含む全 context で composition 開始を取り逃がすため。
///
fn restore_focus_for_ime_key(
    ctx: &egui::Context,
    response: &egui::Response,
    ime_active: bool,
    focus_request: Option<&mut bool>,
    diagnostic_key_policy: TextInputDiagnosticKeyPolicy,
) -> bool {
    let viewport_id = ctx.viewport_id();
    let retry_id = focus_retry_id(viewport_id, response.id);
    let pass = ctx.cumulative_pass_nr();
    let retry_requested = ctx.data_mut(|data| data.remove_temp::<u64>(retry_id));
    let may_restore = keyboard_focus_recovery_input(ctx)
        && ctx.memory(|memory| memory.focused()).is_none()
        && (ime_active
            || !ctx.input(|input| {
                input.events.iter().any(|event| {
                    matches!(
                        event,
                        egui::Event::Key {
                            key: egui::Key::Enter | egui::Key::Escape,
                            pressed: true,
                            ..
                        }
                    )
                })
            }));
    if focus_request.is_some_and(std::mem::take)
        || (retry_requested
            .is_some_and(|requested_pass| requested_pass.checked_add(1) == Some(pass))
            && may_restore)
    {
        response.request_focus();
    }

    if ctx.memory(|memory| memory.has_focus(response.id)) {
        ctx.memory_mut(|memory| {
            memory.set_focus_lock_filter(
                response.id,
                egui::EventFilter {
                    horizontal_arrows: true,
                    vertical_arrows: true,
                    ..Default::default()
                },
            );
        });
    }

    let restored = response.lost_focus() && may_restore;
    if restored {
        response.request_focus();
        ctx.data_mut(|data| data.insert_temp(retry_id, pass));
        ctx.request_repaint();
    }

    if ctx.memory(|memory| memory.has_focus(response.id)) || restored {
        remember_text_focus(ctx, response.id, diagnostic_key_policy);
    } else if response.lost_focus() {
        forget_text_focus(ctx, response.id);
    }

    restored
}

pub fn show_singleline<'text>(
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

pub fn add_singleline<'text>(
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

/// Add a single-line field whose focus contract carries the Sensitive policy.
/// Hosts may read the contract snapshot to redact diagnostic key details while
/// recording their own focus and keyboard-ownership transitions.
pub fn add_singleline_sensitive<'text>(
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

pub fn add_sized_singleline<'text>(
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
mod tests;
