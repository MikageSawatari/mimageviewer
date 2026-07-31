//! IME composition focus policy for egui text fields.

use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::{Duration, Instant};

const IME_STATE_ID: &str = "miv_ime_focus_state";
const IME_FOCUS_RETRY_ID: &str = "miv_ime_focus_retry";
const IME_TEXT_FOCUS_CONTRACT_ID: &str = "miv_ime_text_focus_contract";
const TEXT_INPUT_KEY_DIAGNOSTIC_ID: &str = "miv_text_input_key_diagnostic";
const TEXT_INPUT_KEY_DIAGNOSTIC_STARTED_ID: &str = "miv_text_input_key_diagnostic_started";
const TEXT_INPUT_KEY_DIAGNOSTIC_ROUTINE_BUDGET_BYTES: usize = 1024 * 1024;
const TEXT_INPUT_KEY_DIAGNOSTIC_LOG_OVERHEAD_BYTES: usize = 64;
const IME_EVENT_GRACE: Duration = Duration::from_millis(300);
static TEXT_INPUT_KEY_DIAGNOSTIC_ROUTINE_BYTES: AtomicUsize = AtomicUsize::new(0);
static TEXT_INPUT_KEY_DIAGNOSTIC_BUDGET_NOTICE_LOGGED: AtomicBool = AtomicBool::new(false);

#[derive(Clone, Copy, Debug, Default)]
struct ImeFocusState {
    composing: bool,
    last_event_at: Option<Instant>,
}

#[derive(Clone, Copy, Debug)]
struct ImeTextFocusContract {
    widget_id: egui::Id,
    focused_pass: u64,
}

impl Default for ImeTextFocusContract {
    fn default() -> Self {
        Self {
            widget_id: egui::Id::NULL,
            focused_pass: 0,
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

#[derive(Clone, Copy, Debug)]
struct DiagnosticKeyPress {
    key: DiagnosticKeyIdentity,
    physical_key: Option<DiagnosticKeyIdentity>,
    modifiers: egui::Modifiers,
    repeat: bool,
}

impl DiagnosticKeyPress {
    fn from_event(
        key: egui::Key,
        physical_key: Option<egui::Key>,
        modifiers: egui::Modifiers,
        repeat: bool,
    ) -> Self {
        Self {
            key: diagnostic_key_identity(key, modifiers),
            physical_key: physical_key.map(|key| diagnostic_key_identity(key, modifiers)),
            modifiers,
            repeat,
        }
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

fn ime_state_id(viewport_id: egui::ViewportId) -> egui::Id {
    egui::Id::new((IME_STATE_ID, viewport_id))
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

fn diagnostic_key_identity(key: egui::Key, modifiers: egui::Modifiers) -> DiagnosticKeyIdentity {
    if modifiers.is_none() && !key_is_non_character(key) {
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
            format!(
                "[text-input-key] viewport={:?} pass={} key={} physical_key={} \
                 modifiers={:?} repeat={} field_id={:?} field_id_after={:?} \
                 field_id_changed={} field_seen_after={} focused_before={:?} \
                 focused_after={:?} owner={:?} phase={} side_panel_close={}",
                diagnostic.viewport,
                diagnostic.pass,
                key.key.log_label(),
                DiagnosticKeyIdentity::optional_log_label(key.physical_key),
                key.modifiers,
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

fn ime_input_active(ctx: &egui::Context) -> bool {
    begin_pass_diagnostics(ctx);
    let ime_events: Vec<egui::ImeEvent> = ctx.input(|input| {
        input
            .events
            .iter()
            .filter_map(|event| match event {
                egui::Event::Ime(event) => Some(event.clone()),
                _ => None,
            })
            .collect()
    });
    let now = Instant::now();
    let viewport_id = ctx.viewport_id();
    ctx.data_mut(|data| {
        let id = ime_state_id(viewport_id);
        let mut state = data.get_temp::<ImeFocusState>(id).unwrap_or_default();
        for event in ime_events {
            state.last_event_at = Some(now);
            match event {
                egui::ImeEvent::Enabled => state.composing = true,
                egui::ImeEvent::Preedit(text) => state.composing = !text.is_empty(),
                egui::ImeEvent::Commit(_) | egui::ImeEvent::Disabled => {
                    state.composing = false;
                }
            }
        }
        data.insert_temp(id, state);
        state.composing
            || state
                .last_event_at
                .is_some_and(|at| now.saturating_duration_since(at) < IME_EVENT_GRACE)
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

fn remember_text_focus(ctx: &egui::Context, widget_id: egui::Id) {
    let contract = ImeTextFocusContract {
        widget_id,
        focused_pass: ctx.cumulative_pass_nr(),
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

/// Return the helper-managed field whose keyboard-driven IME focus loss is
/// recoverable in this pass.
///
/// Ownership is deliberately limited to the pass immediately after the field
/// was observed focused. Pointer input wins, so clicks remain legitimate focus
/// changes.
pub(crate) fn recovering_text_input(ctx: &egui::Context) -> Option<egui::Id> {
    if !keyboard_focus_recovery_input(ctx) || !ime_input_active(ctx) {
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
pub(crate) fn restore_focus_for_ime_key(
    ctx: &egui::Context,
    response: &egui::Response,
    ime_active: bool,
    mut focus_request: Option<&mut bool>,
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
        remember_text_focus(ctx, response.id);
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
    let _ = restore_focus_for_ime_key(ui.ctx(), &output.response, ime_active, focus_request);
    output
}

pub(crate) fn add_singleline<'text>(
    ui: &mut egui::Ui,
    text: &'text mut dyn egui::TextBuffer,
    focus_request: Option<&mut bool>,
    configure: impl FnOnce(egui::TextEdit<'text>) -> egui::TextEdit<'text>,
) -> egui::Response {
    let ime_active = ime_input_active(ui.ctx());
    let response = ui.add(configure(egui::TextEdit::singleline(text)));
    let _ = restore_focus_for_ime_key(ui.ctx(), &response, ime_active, focus_request);
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
    let _ = restore_focus_for_ime_key(ui.ctx(), &response, ime_active, focus_request);
    response
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::{Path, PathBuf};

    fn key_event(key: egui::Key) -> egui::Event {
        egui::Event::Key {
            key,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: egui::Modifiers::NONE,
        }
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
                ),
                DiagnosticKeyPress::from_event(
                    egui::Key::A,
                    Some(egui::Key::A),
                    egui::Modifiers::NONE,
                    false,
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
        assert!(!lines[1].contains("key=A"));
    }

    #[test]
    fn diagnostic_masks_plain_characters_but_keeps_shortcut_and_non_character_keys() {
        for key in [
            egui::Key::A,
            egui::Key::Num7,
            egui::Key::Space,
            egui::Key::Questionmark,
        ] {
            let press =
                DiagnosticKeyPress::from_event(key, Some(key), egui::Modifiers::NONE, false);
            assert_eq!(press.key, DiagnosticKeyIdentity::Char);
            assert_eq!(press.physical_key, Some(DiagnosticKeyIdentity::Char));
        }

        let modified = DiagnosticKeyPress::from_event(
            egui::Key::A,
            Some(egui::Key::A),
            egui::Modifiers::CTRL,
            false,
        );
        assert_eq!(modified.key, DiagnosticKeyIdentity::Named(egui::Key::A));
        assert_eq!(
            modified.physical_key,
            Some(DiagnosticKeyIdentity::Named(egui::Key::A))
        );

        let non_character = DiagnosticKeyPress::from_event(
            egui::Key::Backspace,
            Some(egui::Key::Backspace),
            egui::Modifiers::NONE,
            false,
        );
        assert_eq!(
            non_character.key,
            DiagnosticKeyIdentity::Named(egui::Key::Backspace)
        );
        assert_eq!(
            non_character.physical_key,
            Some(DiagnosticKeyIdentity::Named(egui::Key::Backspace))
        );
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
        let ctx = egui::Context::default();
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
        let ctx = egui::Context::default();
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
        let ctx = egui::Context::default();
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
        let ctx = egui::Context::default();
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
            let ctx = egui::Context::default();
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
                        terminal_ime_event,
                        key_event(key),
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
