//! mImageViewer's diagnostics facade over shared egui IME repair.
//!
//! Composition, helper focus contracts and retries belong to egui-ime.
//! Keyboard ownership, diagnostic records and logging remain in this crate.

use egui_ime::TextInputDiagnosticKeyPolicy;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

pub(crate) use egui_ime::{
    focused_text_input, ime_composing_with_pending_events, ime_input_active_in_viewport,
    install_ime_input_policy, recovering_text_input,
};

const TEXT_INPUT_KEY_DIAGNOSTIC_ID: &str = "miv_text_input_key_diagnostic";
const TEXT_INPUT_KEY_DIAGNOSTIC_STARTED_ID: &str = "miv_text_input_key_diagnostic_started";
const TEXT_INPUT_KEY_DIAGNOSTIC_ROUTINE_BUDGET_BYTES: usize = 1024 * 1024;
const TEXT_INPUT_KEY_DIAGNOSTIC_LOG_OVERHEAD_BYTES: usize = 64;
static TEXT_INPUT_KEY_DIAGNOSTIC_ROUTINE_BYTES: AtomicUsize = AtomicUsize::new(0);
static TEXT_INPUT_KEY_DIAGNOSTIC_BUDGET_NOTICE_LOGGED: AtomicBool = AtomicBool::new(false);

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

fn text_input_key_diagnostic_id(viewport_id: egui::ViewportId) -> egui::Id {
    egui::Id::unique((TEXT_INPUT_KEY_DIAGNOSTIC_ID, viewport_id))
}

fn text_input_key_diagnostic_started_id(viewport_id: egui::ViewportId) -> egui::Id {
    egui::Id::unique((TEXT_INPUT_KEY_DIAGNOSTIC_STARTED_ID, viewport_id))
}

fn diagnostic_owner_phase(owner: Option<crate::keyboard_input::KeyboardOwner>) -> &'static str {
    match owner {
        Some(crate::keyboard_input::KeyboardOwner::TextInput { phase, .. }) => match phase {
            crate::keyboard_input::TextInputPhase::PendingFocus => "PendingFocus",
            crate::keyboard_input::TextInputPhase::Focused => "Focused",
            crate::keyboard_input::TextInputPhase::FocusRecovery => "FocusRecovery",
            crate::keyboard_input::TextInputPhase::ImeComposing => "ImeComposing",
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
        .try_update(Ordering::Relaxed, Ordering::Relaxed, |used| {
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
    let contract = egui_ime::text_focus_contract_snapshot(ctx);
    let already_started = ctx.data_mut(|data| {
        let started_id = text_input_key_diagnostic_started_id(viewport);
        if data.get_temp::<u64>(started_id) == Some(pass) {
            return true;
        }
        data.insert_temp(started_id, pass);
        previous =
            data.remove_temp::<TextInputKeyDiagnostic>(text_input_key_diagnostic_id(viewport));
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

/// Read IME activity and start this viewport pass's existing host diagnostics.
pub(crate) fn ime_input_active(ctx: &egui::Context) -> bool {
    begin_pass_diagnostics(ctx);
    egui_ime::ime_input_active(ctx)
}

/// mImageViewer の IME 対応範囲
///
/// コード上 TSF は使用していない。IMM32 を使う経路が 2 つある。
///
/// - main window と fullscreen viewport は winit / egui を通り、`Ime::Preedit` /
///   `Ime::Commit` / `Ime::Disabled` 相当の `egui::ImeEvent` を受ける。
///   IME が処理した `NamedKey::Process` の press / repeat は allow_ime 有効時に egui-winit が
///   Key / Text / clipboard event にしない。物理キーの押下を Hidden（未配送）/ Forwarded（配送済み）で持ち、
///   配送済みは後続の Process repeat でも保持する。release は allow_ime にかかわらず状態を除去し Hidden のみ遮断する。
///   focus 喪失時は全状態を消す。未登録の release と Ime event はそのまま渡す。
/// - winit 管理外の独立 HWND である native video window は
///   `video/native_window.rs` で `WM_IME_STARTCOMPOSITION` /
///   `WM_IME_COMPOSITION` / `WM_IME_ENDCOMPOSITION` / `WM_IME_SETCONTEXT` を直接処理する。
///   `GCS_COMPSTR` と `GCS_RESULTSTR` を読み、`ISC_SHOWUICOMPOSITIONWINDOW` を落として OS の
///   composition window を抑止する。候補位置は `video/native_window_host.rs` が
///   `ImmSetCompositionWindow` / `ImmSetCandidateWindow` で設定する。
///   この経路も `VK_PROCESSKEY` に egui key を割り当てない。共有 plugin は
///   取消時の空 preedit 補完、composition の追跡と理由に基づく focus 復帰を所有する。
///
/// native 経路は `GCS_COMPATTR` / `GCS_COMPCLAUSE` を読まないため変換対象 clause を区別せず、
/// egui へ渡す preedit 文字列にも caret range がない。`IMR_RECONVERTSTRING` による再変換も
/// 実装していない。main window の message loop は winit が所有するため、TSF 対応は
/// winit / egui 上流の責務になる。所有する 1 HWND だけ TSF 化すると同一アプリ内で IME 挙動が
/// 二重化する一方、本アプリの文字入力は folder path、tag、bookmark name 等の補助用途であるため
/// 採用しない。ただし、このような focus / key routing の不具合は本アプリ側の責務であり、
/// IMM32 の範囲で修正する。「完全な IME には TSF が必要」を未修正の理由にしてはならない。
pub(crate) fn show_singleline<'text>(
    ui: &mut egui::Ui,
    text: &'text mut dyn egui::TextBuffer,
    focus_request: Option<&mut bool>,
    configure: impl FnOnce(egui::TextEdit<'text>) -> egui::TextEdit<'text>,
) -> egui::widgets::text_edit::TextEditOutput {
    begin_pass_diagnostics(ui.ctx());
    let output = egui_ime::show_singleline(ui, text, focus_request, configure);
    observe_helper_widget(ui.ctx(), output.response.id);
    output
}

pub(crate) fn add_singleline<'text>(
    ui: &mut egui::Ui,
    text: &'text mut dyn egui::TextBuffer,
    focus_request: Option<&mut bool>,
    configure: impl FnOnce(egui::TextEdit<'text>) -> egui::TextEdit<'text>,
) -> egui::Response {
    begin_pass_diagnostics(ui.ctx());
    let response = egui_ime::add_singleline(ui, text, focus_request, configure);
    observe_helper_widget(ui.ctx(), response.id);
    response
}

/// Keep password-field diagnostic details redacted while recording ownership.
pub(crate) fn add_singleline_sensitive<'text>(
    ui: &mut egui::Ui,
    text: &'text mut dyn egui::TextBuffer,
    focus_request: Option<&mut bool>,
    configure: impl FnOnce(egui::TextEdit<'text>) -> egui::TextEdit<'text>,
) -> egui::Response {
    begin_pass_diagnostics(ui.ctx());
    let response = egui_ime::add_singleline_sensitive(ui, text, focus_request, configure);
    observe_helper_widget(ui.ctx(), response.id);
    response
}

pub(crate) fn add_sized_singleline<'text>(
    ui: &mut egui::Ui,
    size: egui::Vec2,
    text: &'text mut dyn egui::TextBuffer,
    focus_request: Option<&mut bool>,
    configure: impl FnOnce(egui::TextEdit<'text>) -> egui::TextEdit<'text>,
) -> egui::Response {
    begin_pass_diagnostics(ui.ctx());
    let response = egui_ime::add_sized_singleline(ui, size, text, focus_request, configure);
    observe_helper_widget(ui.ctx(), response.id);
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
        let field_id = egui::Id::unique("diagnostic-field");
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
        let field_id = egui::Id::unique("sensitive-diagnostic-field");
        let mut password = String::new();
        let mut request_focus = true;

        ctx.begin_pass(Default::default());
        ctx.root_ui(|root_ui| {
            egui::CentralPanel::default().show(root_ui, |ui| {
                let _ =
                    add_singleline_sensitive(ui, &mut password, Some(&mut request_focus), |edit| {
                        edit.id(field_id).password(true)
                    });
            })
        });
        ctx.end_pass().drop_without_applying_deltas();
        assert_eq!(ctx.memory(|memory| memory.focused()), Some(field_id));

        let modifiers = egui::Modifiers::SHIFT;
        ctx.begin_pass(egui::RawInput {
            events: vec![
                egui::Event::ModifiersChanged(modifiers),
                egui::Event::Key {
                    key: egui::Key::A,
                    physical_key: Some(egui::Key::A),
                    pressed: true,
                    repeat: false,
                    modifiers,
                },
            ],
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
        ctx.end_pass().drop_without_applying_deltas();

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

    fn draw_text_fields(
        ui: &mut egui::Ui,
        first_id: egui::Id,
        second_id: egui::Id,
        first: &mut String,
        second: &mut String,
        first_focus_request: &mut bool,
    ) {
        egui::CentralPanel::default().show(ui, |ui| {
            let _ = show_singleline(ui, first, Some(first_focus_request), |edit| {
                edit.id(first_id)
            });
            let _ = show_singleline(ui, second, None, |edit| edit.id(second_id));
        });
    }

    #[test]
    #[expect(
        deprecated,
        reason = "Exercise retained IME compatibility notifications and cancellation ordering"
    )]
    fn tab_without_ime_keeps_editing_field_focus_and_input_working() {
        let ctx = test_context();
        crate::egui_focus_policy::install_tab_shortcut_focus_policy(&ctx);
        let first_id = egui::Id::unique("plain_tab_first");
        let second_id = egui::Id::unique("plain_tab_second");
        let mut first = String::new();
        let mut second = String::new();
        let mut focus_request = true;
        crate::egui_output::discard_headless_output(ctx.run_ui(Default::default(), |ui| {
            draw_text_fields(
                ui,
                first_id,
                second_id,
                &mut first,
                &mut second,
                &mut focus_request,
            );
        }));
        crate::egui_output::discard_headless_output(ctx.run_ui(Default::default(), |ui| {
            draw_text_fields(
                ui,
                first_id,
                second_id,
                &mut first,
                &mut second,
                &mut focus_request,
            );
        }));
        crate::egui_output::discard_headless_output(ctx.run_ui(
            egui::RawInput {
                events: vec![key_event(egui::Key::Tab)],
                ..Default::default()
            },
            |ui| {
                draw_text_fields(
                    ui,
                    first_id,
                    second_id,
                    &mut first,
                    &mut second,
                    &mut focus_request,
                );
            },
        ));

        assert_eq!(ctx.memory(|memory| memory.focused()), Some(first_id));

        crate::egui_output::discard_headless_output(ctx.run_ui(
            egui::RawInput {
                events: vec![
                    egui::Event::Text("x".to_owned()),
                    egui::Event::Ime(egui::ImeEvent::Enabled),
                    egui::Event::Ime(egui::ImeEvent::Preedit {
                        text: "あ".to_owned(),
                        active_range_chars: None,
                    }),
                    egui::Event::Ime(egui::ImeEvent::Commit("あ".to_owned())),
                ],
                ..Default::default()
            },
            |ui| {
                draw_text_fields(
                    ui,
                    first_id,
                    second_id,
                    &mut first,
                    &mut second,
                    &mut focus_request,
                );
            },
        ));

        assert_eq!(ctx.memory(|memory| memory.focused()), Some(first_id));
        assert_eq!(first, "xあ");
        assert!(second.is_empty());
    }

    fn diagnostic_for_pass(ctx: &egui::Context) -> Option<TextInputKeyDiagnostic> {
        ctx.data(|data| data.get_temp(text_input_key_diagnostic_id(ctx.viewport_id())))
    }

    fn record_text_owner(ctx: &egui::Context, widget_id: egui::Id) {
        record_keyboard_owner(
            ctx,
            crate::keyboard_input::KeyboardOwner::TextInput {
                viewport: ctx.viewport_id(),
                widget_id,
                phase: crate::keyboard_input::TextInputPhase::Focused,
            },
        );
    }

    #[test]
    fn facade_helpers_start_diagnostics_before_drawing_and_observe_the_shared_contract() {
        for variant in 0..4 {
            // Existing diagnostic-only Contexts do not install the input plugin.
            let ctx = egui::Context::default();
            let field_id = egui::Id::unique(("facade-helper", variant));
            let mut text = "abc".to_owned();
            let mut request_focus = true;
            let policy = if variant == 2 {
                TextInputDiagnosticKeyPolicy::Sensitive
            } else {
                TextInputDiagnosticKeyPolicy::Standard
            };
            for keyed_pass in [false, true] {
                ctx.run_ui(
                    viewport_raw_input(
                        egui::ViewportId::ROOT,
                        if keyed_pass {
                            vec![key_event(egui::Key::ArrowRight)]
                        } else {
                            Vec::new()
                        },
                    ),
                    |ui| {
                        let ctx = &ui.ctx().clone();
                        egui::CentralPanel::default().show(ui, |ui| {
                            let assert_before_drawing = || {
                                if keyed_pass {
                                    let diagnostic = diagnostic_for_pass(ctx).unwrap();
                                    assert_eq!(diagnostic.pass, ctx.cumulative_pass_nr());
                                    assert_eq!(diagnostic.field_id_before, field_id);
                                    assert_eq!(diagnostic.focused_before, Some(field_id));
                                    assert!(!diagnostic.field_seen_after);
                                    assert_eq!(diagnostic.field_id_after, None);
                                }
                            };
                            let response = match variant {
                                0 => {
                                    show_singleline(
                                        ui,
                                        &mut text,
                                        Some(&mut request_focus),
                                        |edit| {
                                            assert_before_drawing();
                                            edit.id(field_id)
                                        },
                                    )
                                    .response
                                    .response
                                }
                                1 => add_singleline(
                                    ui,
                                    &mut text,
                                    Some(&mut request_focus),
                                    |edit| {
                                        assert_before_drawing();
                                        edit.id(field_id)
                                    },
                                ),
                                2 => add_singleline_sensitive(
                                    ui,
                                    &mut text,
                                    Some(&mut request_focus),
                                    |edit| {
                                        assert_before_drawing();
                                        edit.id(field_id)
                                    },
                                ),
                                3 => add_sized_singleline(
                                    ui,
                                    egui::vec2(120.0, 24.0),
                                    &mut text,
                                    Some(&mut request_focus),
                                    |edit| {
                                        assert_before_drawing();
                                        edit.id(field_id)
                                    },
                                ),
                                _ => unreachable!(),
                            };
                            assert_eq!(response.id, field_id);
                        });
                        let contract = egui_ime::text_focus_contract_snapshot(ctx).unwrap();
                        assert_eq!(contract.widget_id, field_id);
                        assert_eq!(contract.focused_pass, ctx.cumulative_pass_nr());
                        assert_eq!(contract.diagnostic_key_policy, policy);
                        if keyed_pass {
                            record_text_owner(ctx, field_id);
                            let diagnostic = diagnostic_for_pass(ctx).unwrap();
                            assert!(diagnostic.field_seen_after);
                            assert_eq!(diagnostic.field_id_after, Some(field_id));
                            assert!(!diagnostic_is_anomalous(&diagnostic, Some(field_id)));
                            let before = format_key_diagnostic(&diagnostic, Some(field_id));
                            assert!(!ime_input_active(ctx));
                            assert_eq!(
                                format_key_diagnostic(
                                    &diagnostic_for_pass(ctx).unwrap(),
                                    Some(field_id)
                                ),
                                before
                            );
                            if policy == TextInputDiagnosticKeyPolicy::Sensitive {
                                assert!(before[0].contains("key=Redacted"));
                            }
                        }
                    },
                )
                .drop_without_applying_deltas();
            }
            // Finalize the previous record once, after egui's end-pass validation.
            ctx.begin_pass(Default::default());
            assert_eq!(ctx.memory(|memory| memory.focused()), Some(field_id));
            assert!(!ime_input_active(&ctx));
            assert!(diagnostic_for_pass(&ctx).is_none());
            ctx.end_pass().drop_without_applying_deltas();
        }
    }

    #[test]
    fn facade_observes_the_field_even_after_the_shared_helper_forgets_its_contract() {
        let ctx = test_context();
        let field_id = egui::Id::unique("facade-forget-contract");
        let mut text = "abc".to_owned();
        let mut request_focus = true;
        ctx.run_ui(Default::default(), |ui| {
            egui::CentralPanel::default().show(ui, |ui| {
                add_singleline(ui, &mut text, Some(&mut request_focus), |edit| {
                    edit.id(field_id)
                });
            });
        })
        .drop_without_applying_deltas();
        ctx.run_ui(
            viewport_raw_input(egui::ViewportId::ROOT, vec![key_event(egui::Key::Escape)]),
            |ui| {
                let ctx = &ui.ctx().clone();
                assert_eq!(ctx.memory(|memory| memory.focused()), None);
                egui::CentralPanel::default().show(ui, |ui| {
                    add_singleline(ui, &mut text, None, |edit| edit.id(field_id));
                });
                assert!(egui_ime::text_focus_contract_snapshot(ctx).is_none());
                let diagnostic = diagnostic_for_pass(ctx).unwrap();
                assert!(diagnostic.field_seen_after);
                assert_eq!(diagnostic.field_id_after, Some(field_id));
                assert!(diagnostic_is_anomalous(&diagnostic, None));
            },
        )
        .drop_without_applying_deltas();
        ctx.run_ui(Default::default(), |ui| {
            let ctx = &ui.ctx().clone();
            begin_pass_diagnostics(ctx);
            assert!(diagnostic_for_pass(ctx).is_none());
        })
        .drop_without_applying_deltas();
    }

    #[test]
    #[expect(
        deprecated,
        reason = "Exercise retained IME compatibility notifications and cancellation ordering"
    )]
    fn facade_observes_ime_restoration_after_the_shared_contract_is_refreshed() {
        for external_latch in [false, true] {
            let ctx = test_context();
            let field_id = egui::Id::unique(("facade-restored-contract", external_latch));
            let mut text = String::new();
            let mut request_focus = true;
            ctx.run_ui(Default::default(), |ui| {
                egui::CentralPanel::default().show(ui, |ui| {
                    show_singleline(ui, &mut text, Some(&mut request_focus), |edit| {
                        edit.id(field_id)
                    });
                });
            })
            .drop_without_applying_deltas();
            ctx.run_ui(
                viewport_raw_input(
                    egui::ViewportId::ROOT,
                    vec![
                        egui::Event::Ime(egui::ImeEvent::Enabled),
                        egui::Event::Ime(egui::ImeEvent::Preedit {
                            text: "あ".to_owned(),
                            active_range_chars: None,
                        }),
                        // An ordinary Enter while still composing exercises restoration.
                        // IME-handled Process presses never reach this event queue.
                        key_event(egui::Key::Enter),
                    ],
                ),
                |ui| {
                    let ctx = &ui.ctx().clone();
                    egui::CentralPanel::default().show(ui, |ui| {
                        let focus_request = external_latch.then_some(&mut request_focus);
                        let output =
                            show_singleline(ui, &mut text, focus_request, |edit| edit.id(field_id));
                        // Response reads live egui memory: the helper has already restored focus.
                        assert!(output.response.has_focus());
                    });
                    assert_eq!(ctx.memory(|memory| memory.focused()), Some(field_id));
                    if external_latch {
                        assert!(
                            !request_focus,
                            "helper retries never write the caller's latch"
                        );
                    }
                    assert_eq!(
                        egui_ime::text_focus_contract_snapshot(ctx)
                            .unwrap()
                            .focused_pass,
                        ctx.cumulative_pass_nr()
                    );
                    record_text_owner(ctx, field_id);
                    let diagnostic = diagnostic_for_pass(ctx).unwrap();
                    assert!(diagnostic.field_seen_after);
                    assert_eq!(diagnostic.field_id_after, Some(field_id));
                    assert!(!diagnostic_is_anomalous(&diagnostic, Some(field_id)));
                },
            )
            .drop_without_applying_deltas();
        }
    }

    #[test]
    #[expect(
        deprecated,
        reason = "Exercise retained IME compatibility notifications and cancellation ordering"
    )]
    fn facade_explicit_viewport_and_pending_reads_do_not_start_diagnostics() {
        let ctx = test_context();
        ctx.set_embed_viewports(false);
        let child = egui::ViewportId::from_hash_of("facade-child");
        // RawInput metadata alone does not keep a child registered after root
        // end-pass GC. Declare the native child before its first input pass and
        // on every parent pass, as the real viewport owner does.
        ctx.run_ui(Default::default(), |ui| {
            ui.ctx()
                .show_viewport_deferred(child, Default::default(), |_, _| {});
        })
        .drop_without_applying_deltas();
        ctx.begin_pass(viewport_raw_input(
            child,
            vec![egui::Event::Ime(egui::ImeEvent::Enabled)],
        ));
        ctx.end_pass().drop_without_applying_deltas();
        let field_id = egui::Id::unique("facade-root-field");
        let mut text = "abc".to_owned();
        let mut request_focus = true;
        let mut root_input = viewport_raw_input(egui::ViewportId::ROOT, vec![]);
        root_input.viewports.insert(child, Default::default());
        ctx.run_ui(root_input, |ui| {
            let ctx = &ui.ctx().clone();
            ctx.show_viewport_deferred(child, Default::default(), |_, _| {});
            egui::CentralPanel::default().show(ui, |ui| {
                add_singleline(ui, &mut text, Some(&mut request_focus), |edit| {
                    edit.id(field_id)
                });
            });
        })
        .drop_without_applying_deltas();
        let mut root_input = viewport_raw_input(
            egui::ViewportId::ROOT,
            vec![key_event(egui::Key::ArrowLeft)],
        );
        root_input.viewports.insert(child, Default::default());
        ctx.begin_pass(root_input);
        ctx.show_viewport_deferred(child, Default::default(), |_, _| {});
        let pass = ctx.cumulative_pass_nr();
        let events_before = ctx.input(|input| input.events.clone());
        let contract_before = egui_ime::text_focus_contract_snapshot(&ctx);
        assert!(ime_input_active_in_viewport(&ctx, child));
        assert!(!ime_composing_with_pending_events(
            &ctx,
            child,
            &[egui::Event::Ime(egui::ImeEvent::Disabled)]
        ));
        assert!(ime_composing_with_pending_events(&ctx, child, &[]));
        assert!(!ime_input_active_in_viewport(&ctx, egui::ViewportId::ROOT));
        for viewport in [egui::ViewportId::ROOT, child] {
            assert_ne!(
                ctx.data(
                    |data| data.get_temp::<u64>(text_input_key_diagnostic_started_id(viewport))
                ),
                Some(pass)
            );
        }
        assert!(diagnostic_for_pass(&ctx).is_none());
        assert_eq!(ctx.input(|input| input.events.clone()), events_before);
        assert_eq!(
            egui_ime::text_focus_contract_snapshot(&ctx),
            contract_before
        );
        assert!(!ime_input_active(&ctx));
        assert_eq!(diagnostic_for_pass(&ctx).unwrap().field_id_before, field_id);
        assert_ne!(
            ctx.data(|data| data.get_temp::<u64>(text_input_key_diagnostic_started_id(child))),
            Some(pass)
        );
        ctx.end_pass().drop_without_applying_deltas();

        // Stale metadata must not preserve policy once the actual registration
        // ends. This is a different owner lifetime, not a visibility change.
        let mut retired_input = viewport_raw_input(egui::ViewportId::ROOT, vec![]);
        retired_input.viewports.insert(child, Default::default());
        ctx.run_ui(retired_input, |_| {})
            .drop_without_applying_deltas();
        assert!(!ime_input_active_in_viewport(&ctx, child));
    }

    #[test]
    #[expect(
        deprecated,
        reason = "Exercise retained IME compatibility notifications and cancellation ordering"
    )]
    fn facade_plugin_reregistration_preserves_composition_and_cancellation() {
        let ctx = test_context();
        let field_id = egui::Id::unique("facade-plugin-reregister");
        let mut text = String::new();
        let mut request_focus = true;
        for events in [
            Vec::new(),
            vec![
                egui::Event::Ime(egui::ImeEvent::Enabled),
                egui::Event::Ime(egui::ImeEvent::Preedit {
                    text: "あ".to_owned(),
                    active_range_chars: None,
                }),
            ],
        ] {
            ctx.run_ui(viewport_raw_input(egui::ViewportId::ROOT, events), |ui| {
                egui::CentralPanel::default().show(ui, |ui| {
                    add_singleline(ui, &mut text, Some(&mut request_focus), |edit| {
                        edit.id(field_id)
                    });
                });
            })
            .drop_without_applying_deltas();
        }
        assert_eq!(text, "あ");
        install_ime_input_policy(&ctx);
        assert!(ime_composing_with_pending_events(
            &ctx,
            egui::ViewportId::ROOT,
            &[]
        ));
        ctx.run_ui(
            viewport_raw_input(
                egui::ViewportId::ROOT,
                vec![
                    egui::Event::Ime(egui::ImeEvent::Disabled),
                    egui::Event::Key {
                        key: egui::Key::Escape,
                        physical_key: Some(egui::Key::Escape),
                        pressed: false,
                        repeat: false,
                        modifiers: egui::Modifiers::NONE,
                    },
                ],
            ),
            |ui| {
                egui::CentralPanel::default().show(ui, |ui| {
                    add_singleline(ui, &mut text, None, |edit| edit.id(field_id));
                });
            },
        )
        .drop_without_applying_deltas();
        assert!(text.is_empty());
        assert_eq!(ctx.memory(|memory| memory.focused()), Some(field_id));
        assert!(!ime_composing_with_pending_events(
            &ctx,
            egui::ViewportId::ROOT,
            &[]
        ));
    }

    struct RawTextEditExemption {
        path: &'static str,
        line_anchor: &'static str,
        expected_occurrences: usize,
        reason: &'static str,
    }

    const RAW_TEXT_EDIT_EXEMPTIONS: &[RawTextEditExemption] = &[
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
        RawTextEditExemption {
            path: "src/video/native_presenter/render_core.rs",
            line_anchor: "singleline(&mut text).id(id)",
            expected_occurrences: 4,
            reason: "native_ime_interrupt の2つの低レベル headless fixture。実eguiのcaret移動・focus要求から生成されるIME出力を測り、アプリのfocus復元policyを介在させない",
        },
        RawTextEditExemption {
            path: "src/video/native_window_pump.rs",
            line_anchor: "singleline(&mut text).id(id)",
            expected_occurrences: 2,
            reason: "native_ime_interrupt のheadless fixture。実eguiの旧focus ownerが生成する出力をcontrol配送へ渡す",
        },
        RawTextEditExemption {
            path: "src/video/native_window_pump.rs",
            line_anchor: "singleline(&mut String::new()).id(destination)",
            expected_occurrences: 1,
            reason: "同じnative_ime_interrupt fixtureのfocus移譲先。実eguiのfocus変更によるinterrupt pulseを直接生成する",
        },
        RawTextEditExemption {
            path: "src/video/native_window_pump.rs",
            line_anchor: "singleline(&mut text).id(destination)",
            expected_occurrences: 1,
            reason: "同じnative_ime_interrupt fixtureの配送後pass。実eguiがinterrupt pulseを再発行しないことを直接観測する",
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
