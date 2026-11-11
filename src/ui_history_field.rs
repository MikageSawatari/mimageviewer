//! A helper-managed single-line editor with an explicitly opened history list.
//!
//! The caller owns the entries and submission. A `picked` result replaces the
//! caller's ordinary Enter handling and must use its normal submit/claim path.

use egui::widgets::text_edit::TextEditOutput;
use egui::{Context, Id, Key};

/// Width of the history atom, excluding TextEdit's gap before the suffix.
pub const SUFFIX_WIDTH: f32 = 22.0;

/// Additional field width needed to preserve the editor's existing text width.
pub fn reserved_width(ui: &egui::Ui) -> f32 {
    SUFFIX_WIDTH + ui.spacing().icon_spacing
}

#[derive(Clone, Copy)]
pub(crate) struct HistoryFieldOptions {
    pub visible: bool,
    /// Disables only the history affordance, leaving the editor usable.
    pub availability: Result<(), &'static str>,
    pub tooltip: &'static str,
    pub min_popup_width: Option<f32>,
    pub monospace: bool,
    /// Derived from the caller's existing viewport/widget focus reservation.
    pub focus_pending: bool,
}

impl Default for HistoryFieldOptions {
    fn default() -> Self {
        Self {
            visible: true,
            availability: Ok(()),
            tooltip: "入力履歴",
            min_popup_width: None,
            monospace: false,
            focus_pending: false,
        }
    }
}

pub(crate) struct HistoryFieldOutput {
    pub text: TextEditOutput,
    pub picked: Option<usize>,
    pub focus_requested: bool,
    pub submit_requested: bool,
}

#[derive(Clone, Copy)]
struct HistoryPopupState {
    owner: Id,
    highlight: Option<usize>,
    drawn_pass: u64,
}

fn state_id(ctx: &Context) -> Id {
    Id::unique(("history_field_popup", ctx.viewport_id()))
}

fn popup_id(owner: Id) -> Id {
    owner.with("history_field_popup_area")
}

fn arrow_rect_key(viewport: egui::ViewportId, owner: Id) -> Id {
    owner.with(("history_field_arrow_rect", viewport))
}

#[cfg(test)]
pub(crate) fn arrow_rect_for_test(ctx: &Context, owner: Id) -> egui::Rect {
    let key = arrow_rect_key(ctx.viewport_id(), owner);
    ctx.data(|data| data.get_temp(key)).unwrap()
}

fn state(ctx: &Context) -> Option<HistoryPopupState> {
    let id = state_id(ctx);
    ctx.data(|data| data.get_temp(id))
}

fn store(ctx: &Context, state: HistoryPopupState) {
    let id = state_id(ctx);
    ctx.data_mut(|data| {
        data.insert_temp(id, state);
    });
}

pub(crate) fn close(ctx: &Context) {
    let id = state_id(ctx);
    ctx.data_mut(|data| {
        data.remove::<HistoryPopupState>(id);
    });
}

pub(crate) fn is_open_for(ctx: &Context, owner: Id) -> bool {
    state(ctx).is_some_and(|state| {
        state.owner == owner && state.drawn_pass.saturating_add(1) >= ctx.cumulative_pass_nr()
    }) && ctx.input(|input| input.focused)
        && !egui::Popup::is_any_open(ctx)
}

/// The existing App claim protects this history while the helper lands focus.
pub(crate) fn owns_pending_focus(ctx: &Context) -> bool {
    matches!(crate::keyboard_input::cached_keyboard_owner(ctx),
        Some(crate::keyboard_input::KeyboardOwner::TextInput {
            viewport, widget_id, phase: crate::keyboard_input::TextInputPhase::PendingFocus,
        }) if viewport == ctx.viewport_id() && is_open_for(ctx, widget_id))
}

struct SurrenderFocusGuard {
    ctx: Context,
    previous: egui::SurrenderFocusOn,
}

impl Drop for SurrenderFocusGuard {
    fn drop(&mut self) {
        self.ctx.options_mut(|options| {
            options.input_options.surrender_focus_on = self.previous;
        });
    }
}

fn retire(ctx: &Context) {
    if state(ctx).is_some_and(|state| state.drawn_pass.saturating_add(1) < ctx.cumulative_pass_nr())
        || !ctx.input(|input| input.focused)
        || egui::Popup::is_any_open(ctx)
    {
        close(ctx);
    }
}

fn plain_key(ctx: &Context, key: Key) -> bool {
    ctx.input(|input| {
        input.events.iter().any(|event| {
            matches!(event,
        egui::Event::Key { key: pressed, pressed: true, modifiers, .. }
            if *pressed == key && modifiers.is_none())
        })
    })
}

fn consume_key(ctx: &Context, key: Key) {
    ctx.input_mut(|input| {
        input.events.retain(|event| {
            !matches!(event,
            egui::Event::Key { key: pressed, pressed: true, modifiers, .. }
                if *pressed == key && modifiers.is_none())
        });
    });
    #[cfg(windows)]
    {
        let virtual_key = match key {
            Key::ArrowDown => 0x28,
            Key::ArrowUp => 0x26,
            Key::Enter => 0x0D,
            Key::Escape => 0x1B,
            _ => unreachable!("history only handles arrows, Enter and Escape"),
        };
        crate::key_input::consume_key_edges(ctx.viewport_id(), |edge| {
            edge.virtual_key == virtual_key && !edge.ctrl && !edge.shift && !edge.alt
        });
    }
}

/// Editing wins over history navigation in an aggregated backend input batch.
/// This only recognizes intent; TextEdit remains the sole text editor.
fn has_edit_intent(ctx: &Context, id: Id) -> bool {
    ctx.input(|input| {
        input.events.iter().any(|event| match event {
            egui::Event::Text(_)
            | egui::Event::Paste(_)
            | egui::Event::Cut
            | egui::Event::Ime(egui::ImeEvent::Commit(_) | egui::ImeEvent::Preedit { .. }) => true,
            egui::Event::Key {
                key,
                pressed: true,
                modifiers,
                ..
            } => {
                matches!(key, Key::Backspace | Key::Delete)
                    || (modifiers.ctrl && matches!(key, Key::H | Key::K | Key::U | Key::W))
                    || (modifiers.command && matches!(key, Key::Z | Key::Y))
            }
            egui::Event::AccessKitActionRequest(request) => {
                request.target_node == id.accesskit_id()
                    && request.action == egui::accesskit::Action::ReplaceSelectedText
            }
            _ => false,
        })
    })
}

/// Run before the App's search Escape capture. A foreground modal owns Escape.
pub(crate) fn take_escape(ctx: &Context, modal: bool) -> bool {
    if !plain_key(ctx, Key::Escape) || crate::ime_focus::ime_input_active(ctx) {
        return false;
    }
    let Some(current) = state(ctx) else {
        return false;
    };
    close(ctx);
    if current.drawn_pass.saturating_add(1) < ctx.cumulative_pass_nr()
        || !ctx.input(|input| input.focused)
        || modal
        || egui::Popup::is_any_open(ctx)
    {
        return false;
    }
    consume_key(ctx, Key::Escape);
    true
}

fn consume_wheel(ctx: &Context) {
    crate::wheel_input::consume_wheel_input(ctx);
}

pub(crate) fn show_singleline<'text>(
    ui: &mut egui::Ui,
    text: &'text mut String,
    focus_request: Option<&mut bool>,
    id: Id,
    entries: &[String],
    options: HistoryFieldOptions,
    configure: impl FnOnce(egui::TextEdit<'text>) -> egui::TextEdit<'text>,
) -> HistoryFieldOutput {
    let ctx = ui.ctx().clone();
    retire(&ctx);
    let pass = ctx.cumulative_pass_nr();
    let enabled = ui.is_enabled() && options.visible && options.availability.is_ok();
    // A different popup owns its input until it closes. Retirement alone must
    // not let the same pass's Down immediately reopen the history beneath it.
    let may_open = enabled && !egui::Popup::is_any_open(&ctx) && ctx.input(|input| input.focused);
    if !enabled && state(&ctx).is_some_and(|state| state.owner == id) {
        close(&ctx);
    }
    let mut current = state(&ctx).filter(|state| state.owner == id);
    if let Some(current) = &mut current {
        current.drawn_pass = pass;
        current.highlight = current.highlight.and_then(|index| {
            if entries.is_empty() {
                None
            } else {
                Some(index.min(entries.len() - 1))
            }
        });
        store(&ctx, *current);
    }
    let was_open = current.is_some();
    let ime_active = crate::ime_focus::ime_input_active(&ctx);
    let pointer = ctx.pointer_interact_pos();
    let outside_pointer = ctx.input(|input| input.pointer.any_pressed())
        && pointer.is_some_and(|pos| {
            !ctx.read_response(id)
                .is_some_and(|response| response.rect.contains(pos))
                && ctx.layer_id_at(pos)
                    != Some(egui::LayerId::new(egui::Order::Foreground, popup_id(id)))
        });
    let requesting_focus = options.focus_pending
        && enabled
        && !outside_pointer
        && crate::ime_focus::activate_reserved_focus(ui, id);
    let pending_enter = requesting_focus && !ime_active && plain_key(&ctx, Key::Enter);
    let editing = !ime_active && has_edit_intent(&ctx, id);
    if editing && current.is_some() {
        close(&ctx);
        current = None;
    }
    let mut picked = None;
    let mut moved_highlight = false;
    let has_focus = ctx.memory(|memory| memory.has_focus(id));
    let mut request = focus_request.is_some_and(std::mem::take) && !requesting_focus;
    if may_open && has_focus && !ime_active && !editing {
        if plain_key(&ctx, Key::ArrowDown) {
            consume_key(&ctx, Key::ArrowDown);
            moved_highlight = true;
            let highlight = if entries.is_empty() {
                None
            } else {
                Some(
                    current
                        .and_then(|state| state.highlight)
                        .map_or(0, |index| (index + 1).min(entries.len() - 1)),
                )
            };
            current = Some(HistoryPopupState {
                owner: id,
                highlight,
                drawn_pass: pass,
            });
            store(&ctx, current.unwrap());
        }
        if let Some(mut open) = current {
            if plain_key(&ctx, Key::ArrowUp) {
                consume_key(&ctx, Key::ArrowUp);
                moved_highlight = true;
                open.highlight = if entries.is_empty() {
                    None
                } else {
                    Some(open.highlight.unwrap_or(0).saturating_sub(1))
                };
                current = Some(open);
                store(&ctx, open);
            }
            if plain_key(&ctx, Key::Enter) {
                picked = open.highlight;
                if picked.is_some() {
                    consume_key(&ctx, Key::Enter);
                }
                current = None;
                close(&ctx);
            }
        }
    }

    let arrow_rect_id = arrow_rect_key(ctx.viewport_id(), id);
    let pointer_on_arrow = options.visible
        && pointer.is_some_and(|pos| {
            ctx.data(|data| data.get_temp::<egui::Rect>(arrow_rect_id))
                .is_some_and(|rect| rect.contains(pos))
        });
    let pointer_on_popup = current.is_some()
        && pointer.and_then(|pos| ctx.layer_id_at(pos))
            == Some(egui::LayerId::new(egui::Order::Foreground, popup_id(id)));
    let surrender = (pointer_on_arrow || pointer_on_popup).then(|| {
        let previous = ctx.options(|options| options.input_options.surrender_focus_on);
        ctx.options_mut(|options| {
            options.input_options.surrender_focus_on = egui::SurrenderFocusOn::Never
        });
        SurrenderFocusGuard {
            ctx: ctx.clone(),
            previous,
        }
    });
    let atom_id = egui::IdSalt::new("history_field_arrow");
    let arrow_size = egui::vec2(SUFFIX_WIDTH, ui.text_style_height(&egui::TextStyle::Body));
    let output = crate::ime_focus::show_singleline(ui, text, Some(&mut request), |edit| {
        let edit = configure(edit).id(id);
        if options.visible {
            edit.suffix(egui::Atom::custom(atom_id, arrow_size))
        } else {
            edit
        }
    });
    drop(surrender);
    // lost_focus only covers prior passes, not acquisition and submit in one pass.
    let submit_requested = pending_enter && picked.is_none() && !output.response.has_focus();

    let mut arrow_clicked = false;
    let mut focus_requested = false;
    if let Some(rect) = output.response.rect(atom_id).filter(|_| options.visible) {
        ctx.data_mut(|data| data.insert_temp(arrow_rect_id, rect));
        let response = ui
            .scope_builder(egui::UiBuilder::new().max_rect(rect), |ui| {
                ui.spacing_mut().button_padding = egui::Vec2::ZERO;
                ui.add_enabled(
                    enabled,
                    egui::Button::new("▼").frame(false).min_size(rect.size()),
                )
            })
            .inner;
        response.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, enabled, "入力履歴"));
        arrow_clicked = response.clicked() && may_open;
        response
            .on_hover_text(options.tooltip)
            .on_disabled_hover_text(options.availability.err().unwrap_or(options.tooltip));
    } else {
        ctx.data_mut(|data| {
            data.remove::<egui::Rect>(arrow_rect_id);
        });
    }
    if arrow_clicked {
        if current.is_some() {
            current = None;
            close(&ctx);
        } else {
            let open = HistoryPopupState {
                owner: id,
                highlight: None,
                drawn_pass: pass,
            };
            current = Some(open);
            store(&ctx, open);
            if !output.response.has_focus() {
                focus_requested = true;
                ctx.request_repaint();
            }
        }
    }
    if (output.response.changed() && !ime_active)
        || (!output.response.has_focus() && !arrow_clicked && !requesting_focus)
    {
        current = None;
        if state(&ctx).is_some_and(|state| state.owner == id) {
            close(&ctx);
        }
    }

    if let Some(open) = current {
        let popup_width = options
            .min_popup_width
            .unwrap_or(output.response.rect.width())
            .max(output.response.rect.width());
        let pos = output.response.rect.left_bottom();
        let max_height =
            (ctx.content_rect().bottom() - pos.y - 8.0).max(ui.spacing().interact_size.y);
        let popup = egui::Area::new(popup_id(id))
            .order(egui::Order::Foreground)
            .fixed_pos(pos)
            .movable(false)
            .show(&ctx, |ui| {
                crate::os_theme::apply_dark_ui(ui);
                crate::os_theme::dark_popup_frame(&ctx).show(ui, |ui| {
                    ui.set_width((popup_width - ui.spacing().window_margin.sum().x).max(0.0));
                    egui::ScrollArea::vertical()
                        .max_height(max_height)
                        .auto_shrink([false, true])
                        .show(ui, |ui| {
                            if entries.is_empty() {
                                ui.weak("履歴はありません");
                            }
                            for (index, entry) in entries.iter().enumerate() {
                                let text = if options.monospace {
                                    egui::RichText::new(entry).monospace()
                                } else {
                                    egui::RichText::new(entry)
                                };
                                let response = ui
                                    .add_sized(
                                        [ui.available_width(), ui.spacing().interact_size.y],
                                        egui::Button::selectable(
                                            open.highlight == Some(index),
                                            (text, egui::Atom::grow()),
                                        )
                                        .truncate(),
                                    )
                                    .on_hover_text(entry);
                                if response.clicked() {
                                    picked = Some(index);
                                }
                                if open.highlight == Some(index) && moved_highlight {
                                    response.scroll_to_me(None);
                                }
                            }
                        });
                });
            });
        let outside_press = ctx.input(|input| input.pointer.any_pressed())
            && pointer.is_some_and(|pos| {
                !output.response.rect.contains(pos) && !popup.response.rect.contains(pos)
            });
        if picked.is_some() || outside_press {
            close(&ctx);
            current = None;
        }
    }
    if was_open || current.is_some() {
        consume_wheel(&ctx);
    }
    if current.is_some() && output.response.has_focus() {
        ctx.memory_mut(|memory| {
            memory.set_focus_lock_filter(
                id,
                egui::EventFilter {
                    horizontal_arrows: true,
                    vertical_arrows: true,
                    escape: true,
                    ..Default::default()
                },
            )
        });
    }
    HistoryFieldOutput {
        text: output,
        picked,
        focus_requested,
        submit_requested,
    }
}

/// Pure renderer shared by the history affordance's closed/open snapshots.
pub fn draw_history_field_snapshot_fixture(ui: &mut egui::Ui, open: bool) {
    let id = Id::unique("history_field_snapshot");
    if open {
        store(
            ui.ctx(),
            HistoryPopupState {
                owner: id,
                highlight: Some(0),
                drawn_pass: ui.ctx().cumulative_pass_nr(),
            },
        );
    }
    let mut text = "旅行".to_owned();
    let mut focus = open;
    show_singleline(
        ui,
        &mut text,
        Some(&mut focus),
        id,
        &["旅行".to_owned(), "青空".to_owned(), "写真集".to_owned()],
        HistoryFieldOptions::default(),
        |edit| edit.desired_width(320.0).hint_text("検索語"),
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui_kittest::{Harness, kittest::Queryable};

    struct Fixture {
        text: String,
        entries: Vec<String>,
        request_focus: bool,
        show: bool,
        enabled: bool,
        options: HistoryFieldOptions,
        modal: bool,
        picked: Vec<usize>,
        submitted: usize,
        escapes: usize,
        history_escapes: usize,
        focused: bool,
        rect: egui::Rect,
        wheel_after: bool,
        discard_pick: bool,
        popup_click_focus: Vec<bool>,
    }

    fn field_id() -> Id {
        Id::unique("history_test_input")
    }

    fn harness(focused: bool) -> Harness<'static, Fixture> {
        crate::test_ui::builder()
            .with_context_setup(|ctx| {
                crate::ime_focus::install_ime_input_policy(ctx);
                crate::ui_fonts::configure_fonts(ctx);
            })
            .with_size(egui::vec2(600.0, 320.0))
            .build_ui_state(
                |ui, state: &mut Fixture| {
                    let ctx = ui.ctx().clone();
                    let taken = take_escape(&ctx, state.modal);
                    state.history_escapes += usize::from(taken);
                    if !taken
                        && plain_key(&ctx, Key::Escape)
                        && !crate::ime_focus::ime_input_active(&ctx)
                    {
                        state.escapes += 1;
                    }
                    let enter =
                        plain_key(&ctx, Key::Enter) && !crate::ime_focus::ime_input_active(&ctx);
                    let owned = crate::keyboard_input::text_input_owns_submit(&ctx, field_id());
                    if state.show {
                        ui.add_enabled_ui(state.enabled, |ui| {
                            let output = show_singleline(
                                ui,
                                &mut state.text,
                                Some(&mut state.request_focus),
                                field_id(),
                                &state.entries,
                                state.options,
                                |edit| edit.desired_width(280.0).hint_text("Search"),
                            );
                            state.rect = output.text.response.rect;
                            state.request_focus |= output.focus_requested;
                            if let Some(index) = output.picked {
                                state.picked.push(index);
                                crate::keyboard_input::claim_text_input_submit(&ctx, field_id());
                                if state.discard_pick {
                                    ctx.request_discard("exercise same input re-pass");
                                }
                            } else if output.submit_requested
                                || owned && enter && output.text.response.lost_focus()
                            {
                                state.submitted += 1;
                                crate::keyboard_input::claim_text_input_submit(&ctx, field_id());
                            }
                        });
                    }
                    state.focused = ctx.memory(|memory| memory.has_focus(field_id()));
                    if ctx
                        .input(|input| input.pointer.any_pressed() || input.pointer.any_released())
                        && super::state(&ctx).is_some()
                    {
                        state.popup_click_focus.push(state.focused);
                    }
                    state.wheel_after = ctx.input(|input| {
                        input.smooth_scroll_delta != egui::Vec2::ZERO
                            || input
                                .events
                                .iter()
                                .any(|event| matches!(event, egui::Event::MouseWheel { .. }))
                    });
                    ui.add_space(200.0);
                    let _ = ui.button("Outside");
                },
                Fixture {
                    text: "draft".into(),
                    entries: vec!["first".into(), "second".into(), "third".into()],
                    request_focus: focused,
                    show: true,
                    enabled: true,
                    options: HistoryFieldOptions::default(),
                    modal: false,
                    picked: Vec::new(),
                    submitted: 0,
                    escapes: 0,
                    history_escapes: 0,
                    focused: false,
                    rect: egui::Rect::NOTHING,
                    wheel_after: false,
                    discard_pick: false,
                    popup_click_focus: Vec::new(),
                },
            )
    }

    fn key(h: &mut Harness<'_, Fixture>, key: Key) {
        h.key_press(key);
        h.step();
        h.run_steps(2);
    }
    fn popup(h: &Harness<'_, Fixture>) -> Option<HistoryPopupState> {
        state(&h.ctx)
    }

    fn pointer(h: &mut Harness<'_, Fixture>, pos: egui::Pos2, pressed: bool) {
        h.input_mut().events.extend([
            egui::Event::PointerMoved(pos),
            egui::Event::PointerButton {
                pos,
                button: egui::PointerButton::Primary,
                pressed,
                modifiers: egui::Modifiers::NONE,
            },
        ]);
        h.step();
    }

    #[test]
    fn history_field_arrows_pick_and_plain_enter_are_exclusive() {
        #[cfg(windows)]
        let _input = crate::key_input::lock_test_input();
        let mut h = harness(true);
        h.run_steps(3);
        key(&mut h, Key::ArrowUp);
        assert!(popup(&h).is_none());
        key(&mut h, Key::ArrowDown);
        assert_eq!(popup(&h).unwrap().highlight, Some(0));
        key(&mut h, Key::ArrowDown);
        assert_eq!(popup(&h).unwrap().highlight, Some(1));
        key(&mut h, Key::ArrowUp);
        assert_eq!(popup(&h).unwrap().highlight, Some(0));
        key(&mut h, Key::Enter);
        assert_eq!(h.state().picked, [0]);
        assert_eq!(h.state().submitted, 0);
        assert!(popup(&h).is_none());
        assert!(!h.state().focused);
        h.state_mut().request_focus = true;
        h.run_steps(3);
        h.get_by_label("入力履歴").click();
        h.run_steps(3);
        assert_eq!(popup(&h).unwrap().highlight, None);
        key(&mut h, Key::Enter);
        assert_eq!(h.state().submitted, 1);
        assert_eq!(h.state().picked, [0]);
    }

    #[test]
    fn history_field_escape_closes_only_history_and_restores_plain_escape() {
        #[cfg(windows)]
        let _input = crate::key_input::lock_test_input();
        let mut h = harness(true);
        h.run_steps(3);
        key(&mut h, Key::ArrowDown);
        key(&mut h, Key::Escape);
        assert!(popup(&h).is_none());
        assert!(h.state().focused);
        assert_eq!(h.state().history_escapes, 1);
        assert_eq!(h.state().escapes, 0);
        key(&mut h, Key::Escape);
        assert_eq!(h.state().escapes, 1);
    }

    #[test]
    fn history_field_ime_does_not_navigate_pick_or_dismiss() {
        #[cfg(windows)]
        let _input = crate::key_input::lock_test_input();
        for initially_open in [false, true] {
            let mut h = harness(true);
            h.run_steps(3);
            if initially_open {
                key(&mut h, Key::ArrowDown);
            }
            h.input_mut()
                .events
                .extend([egui::Event::Ime(egui::ImeEvent::Preedit {
                    text: "あ".into(),
                    active_range_chars: None,
                })]);
            h.step();
            for key_to_press in [Key::ArrowDown, Key::ArrowUp, Key::Enter, Key::Escape] {
                key(&mut h, key_to_press);
                assert_eq!(popup(&h).is_some(), initially_open);
                assert!(h.state().picked.is_empty());
                assert_eq!(h.state().submitted, 0);
                assert_eq!(h.state().history_escapes, 0);
            }
        }
    }

    #[test]
    fn history_field_suffix_focus_and_press_release_inside_list() {
        #[cfg(windows)]
        let _input = crate::key_input::lock_test_input();
        for initially_focused in [false, true] {
            let mut h = harness(initially_focused);
            h.run_steps(3);
            let rect_key = arrow_rect_key(h.ctx.viewport_id(), field_id());
            let arrow = h
                .ctx
                .data(|data| data.get_temp::<egui::Rect>(rect_key))
                .unwrap()
                .center();
            pointer(&mut h, arrow, true);
            pointer(&mut h, arrow, false);
            h.run_steps(3);
            assert!(popup(&h).is_some());
            assert!(h.state().focused);
            h.state_mut().popup_click_focus.clear();
            pointer(&mut h, arrow, true);
            pointer(&mut h, arrow, false);
            h.run_steps(3);
            assert!(popup(&h).is_none());
            assert!(h.state().focused);
            key(&mut h, Key::ArrowDown);
            let item = h.get_by_label("second").rect().center();
            pointer(&mut h, item, true);
            assert!(h.state().focused);
            assert!(popup(&h).is_some());
            pointer(&mut h, item, false);
            h.run_steps(2);
            assert_eq!(h.state().picked, [1]);
            assert!(h.state().popup_click_focus.iter().all(|focused| *focused));
        }
    }

    #[test]
    fn history_field_edit_outside_disabled_and_hidden_close() {
        #[cfg(windows)]
        let _input = crate::key_input::lock_test_input();
        let mut h = harness(true);
        h.run_steps(3);
        key(&mut h, Key::ArrowDown);
        h.event(egui::Event::Text("x".into()));
        h.step();
        assert!(popup(&h).is_none());
        key(&mut h, Key::ArrowDown);
        h.get_by_label("Outside").click();
        h.run_steps(3);
        assert!(popup(&h).is_none());
        h.state_mut().request_focus = true;
        h.run_steps(3);
        h.state_mut().enabled = false;
        key(&mut h, Key::ArrowDown);
        assert!(popup(&h).is_none());
        h.state_mut().enabled = true;
        h.state_mut().request_focus = true;
        h.run_steps(3);
        key(&mut h, Key::ArrowDown);
        assert!(popup(&h).is_some());
        h.state_mut().options.visible = false;
        h.run_steps(2);
        assert!(popup(&h).is_none());
        assert!(h.query_by_label("入力履歴").is_none());
    }

    #[test]
    fn history_field_retirement_modal_and_other_popup_preserve_foreground_escape() {
        #[cfg(windows)]
        let _input = crate::key_input::lock_test_input();
        for boundary in 0..3 {
            let mut h = harness(true);
            h.run_steps(3);
            key(&mut h, Key::ArrowDown);
            match boundary {
                0 => {
                    h.state_mut().show = false;
                    h.run_steps(2);
                }
                1 => {
                    h.state_mut().modal = true;
                }
                _ => egui::Popup::open_id(&h.ctx, Id::unique("other_popup")),
            }
            key(&mut h, Key::Escape);
            assert!(popup(&h).is_none());
            assert_eq!(h.state().history_escapes, 0);
            assert_eq!(h.state().escapes, 1);
        }
    }

    #[test]
    fn history_field_other_popup_prevents_same_pass_down_reopening() {
        #[cfg(windows)]
        let _input = crate::key_input::lock_test_input();
        let mut h = harness(true);
        h.run_steps(3);
        key(&mut h, Key::ArrowDown);
        assert!(h.state().focused);
        egui::Popup::open_id(&h.ctx, Id::unique("editing_menu"));
        h.key_down(Key::ArrowDown);
        h.step();
        assert!(popup(&h).is_none());
        assert!(h.state().picked.is_empty());
    }

    #[test]
    fn history_field_wheel_is_consumed_even_outside_popup() {
        #[cfg(windows)]
        let _input = crate::key_input::lock_test_input();
        let mut h = harness(true);
        h.run_steps(3);
        key(&mut h, Key::ArrowDown);
        h.input_mut().events.extend([
            egui::Event::PointerMoved(egui::pos2(580.0, 300.0)),
            egui::Event::MouseWheel {
                unit: egui::MouseWheelUnit::Point,
                delta: egui::vec2(0.0, -24.0),
                modifiers: egui::Modifiers::NONE,
                phase: egui::TouchPhase::Move,
                source: egui::MouseWheelSource::Unknown,
            },
        ]);
        h.step();
        assert!(!h.state().wheel_after);
    }

    #[test]
    fn history_field_empty_and_changing_entries_clamp_highlight() {
        #[cfg(windows)]
        let _input = crate::key_input::lock_test_input();
        let mut h = harness(true);
        h.run_steps(3);
        for _ in 0..4 {
            key(&mut h, Key::ArrowDown);
        }
        assert_eq!(popup(&h).unwrap().highlight, Some(2));
        h.state_mut().entries.truncate(1);
        h.run_steps(2);
        assert_eq!(popup(&h).unwrap().highlight, Some(0));
        h.state_mut().entries.clear();
        h.run_steps(2);
        assert_eq!(popup(&h).unwrap().highlight, None);
        assert!(h.query_by_label("履歴はありません").is_some());
        key(&mut h, Key::Enter);
        assert!(h.state().picked.is_empty());
        assert_eq!(h.state().submitted, 1);
        assert!(popup(&h).is_none());
    }

    #[test]
    fn history_field_blank_margin_and_scrollbar_do_not_depart() {
        #[cfg(windows)]
        let _input = crate::key_input::lock_test_input();
        let mut h = harness(true);
        h.run_steps(3);
        h.state_mut().entries = (0..40).map(|i| format!("entry {i}")).collect();
        key(&mut h, Key::ArrowDown);
        let margin = h.state().rect.left_bottom() + egui::vec2(2.0, 2.0);
        for pressed in [true, false] {
            pointer(&mut h, margin, pressed);
            assert!(h.state().focused);
            assert!(popup(&h).is_some());
        }
        let scrollbar = h
            .get_by_role(egui::accesskit::Role::ScrollBar)
            .rect()
            .center();
        for pressed in [true, false] {
            pointer(&mut h, scrollbar, pressed);
            assert!(h.state().focused);
            assert!(popup(&h).is_some());
        }
        assert!(h.state().picked.is_empty());
        assert!(h.state().popup_click_focus.iter().all(|focused| *focused));
    }

    #[test]
    fn history_field_suffix_unavailable_and_focus_loss_retire() {
        #[cfg(windows)]
        let _input = crate::key_input::lock_test_input();
        let mut h = harness(true);
        h.run_steps(3);
        h.state_mut().options.availability = Err("検索中は使えません");
        key(&mut h, Key::ArrowDown);
        assert!(popup(&h).is_none());
        assert!(h.query_by_label("入力履歴").is_some());
        h.state_mut().options.availability = Ok(());
        h.run_steps(2);
        key(&mut h, Key::ArrowDown);
        assert!(popup(&h).is_some());
        h.input_mut().focused = false;
        h.event(egui::Event::WindowFocused(false));
        h.step();
        assert!(popup(&h).is_none());
    }

    #[test]
    fn history_field_same_input_discard_does_not_pick_twice() {
        #[cfg(windows)]
        let _input = crate::key_input::lock_test_input();
        let mut h = harness(true);
        h.run_steps(3);
        key(&mut h, Key::ArrowDown);
        h.state_mut().discard_pick = true;
        key(&mut h, Key::Enter);
        assert_eq!(h.state().picked, [0]);
        assert_eq!(h.state().submitted, 0);
    }

    #[test]
    fn history_field_reserved_width_preserves_actual_text_body_for_atom_gaps() {
        for gap in [0.0f32, 4.0, 9.0] {
            let mut h = crate::test_ui::builder()
                .with_context_setup(|ctx| {
                    crate::ime_focus::install_ime_input_policy(ctx);
                    crate::ui_fonts::configure_fonts(ctx);
                })
                .with_size(egui::vec2(600.0, 160.0))
                .build_ui(move |ui| {
                    ui.spacing_mut().icon_spacing = gap;
                    let mut baseline_text = "draft".to_owned();
                    let baseline = show_singleline(
                        ui,
                        &mut baseline_text,
                        None,
                        Id::unique(("history_width_baseline", gap.to_bits())),
                        &[],
                        HistoryFieldOptions { visible: false, ..Default::default() },
                        |edit| edit.desired_width(168.0).clip_text(true).hint_text("Baseline"),
                    );
                    let extra = reserved_width(ui);
                    let mut history_text = "draft".to_owned();
                    let history = show_singleline(
                        ui,
                        &mut history_text,
                        None,
                        Id::unique(("history_width_suffix", gap.to_bits())),
                        &[],
                        HistoryFieldOptions::default(),
                        |edit| edit.desired_width(168.0 + extra).clip_text(true).hint_text("History"),
                    );
                    // Preserve the released whole-editor reservation. The actual
                    // text padding belongs to egui's TextEdit style, not a fixed
                    // 4pt margin inferred from the old toolbar measurement.
                    assert!((baseline.text.response.rect.width() - 168.0).abs() < 0.01);
                    assert!(baseline.text.text_clip_rect.width() > 0.0);
                    assert!(
                        (history.text.text_clip_rect.width() - baseline.text.text_clip_rect.width()).abs() < 0.01,
                        "suffix must preserve the actual text body with gap {gap}: baseline={:?}, history={:?}",
                        baseline.text.text_clip_rect, history.text.text_clip_rect,
                    );
                    assert!(
                        (history.text.response.rect.width() - baseline.text.response.rect.width() - extra).abs() < 0.01,
                        "the outer field must grow by the atom and its gap",
                    );
                });
            h.run_steps(3);
        }
    }

    #[test]
    fn history_field_surrender_focus_is_restored_when_configure_panics() {
        let ctx = Context::default();
        crate::ui_fonts::configure_fonts(&ctx);
        let draw = |ui: &mut egui::Ui| {
            let mut text = String::new();
            show_singleline(
                ui,
                &mut text,
                None,
                field_id(),
                &[],
                Default::default(),
                |edit| edit.desired_width(280.0).hint_text("Search"),
            );
        };
        ctx.run_ui(Default::default(), draw)
            .drop_without_applying_deltas();
        let rect = arrow_rect_for_test(&ctx, field_id());
        for previous in [
            egui::SurrenderFocusOn::Clicks,
            egui::SurrenderFocusOn::Never,
        ] {
            ctx.options_mut(|options| options.input_options.surrender_focus_on = previous);
            ctx.run_ui(
                egui::RawInput {
                    events: vec![egui::Event::PointerMoved(rect.center())],
                    ..Default::default()
                },
                |ui| {
                    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                        let mut text = String::new();
                        show_singleline(
                            ui,
                            &mut text,
                            None,
                            field_id(),
                            &[],
                            Default::default(),
                            |_| {
                                assert_eq!(
                                    ctx.options(|options| options.input_options.surrender_focus_on),
                                    egui::SurrenderFocusOn::Never
                                );
                                panic!("exercise configure unwinding");
                            },
                        );
                    }));
                    assert!(result.is_err());
                    assert_eq!(
                        ctx.options(|options| options.input_options.surrender_focus_on),
                        previous
                    );
                },
            )
            .drop_without_applying_deltas();
        }
    }

    #[test]
    fn history_field_pending_focus_is_not_granted_after_retirement() {
        for cause in ["disabled", "hidden", "unavailable"] {
            let ctx = Context::default();
            crate::ui_fonts::configure_fonts(&ctx);
            ctx.run_ui(Default::default(), |ui| {
                store(
                    &ctx,
                    HistoryPopupState {
                        owner: field_id(),
                        highlight: None,
                        drawn_pass: ctx.cumulative_pass_nr(),
                    },
                );
                if cause == "disabled" {
                    ui.disable();
                }
                let mut text = String::new();
                show_singleline(
                    ui,
                    &mut text,
                    None,
                    field_id(),
                    &[],
                    HistoryFieldOptions {
                        visible: cause != "hidden",
                        availability: if cause == "unavailable" {
                            Err("unavailable")
                        } else {
                            Ok(())
                        },
                        focus_pending: true,
                        ..Default::default()
                    },
                    |edit| edit.hint_text("Search"),
                );
                assert!(state(&ctx).is_none());
                assert!(
                    !ctx.memory(|m| m.has_focus(field_id())),
                    "retired reservation: {cause}"
                );
            })
            .drop_without_applying_deltas();
        }
    }

    #[test]
    fn history_field_text_or_paste_with_enter_submits_edit_instead_of_history() {
        #[cfg(windows)]
        let _input = crate::key_input::lock_test_input();
        for edit in [
            egui::Event::Text("x".into()),
            egui::Event::Paste("x".into()),
        ] {
            let mut h = harness(true);
            h.run_steps(3);
            key(&mut h, Key::ArrowDown);
            h.input_mut().events.extend([
                edit,
                egui::Event::Key {
                    key: Key::Enter,
                    physical_key: None,
                    pressed: true,
                    repeat: false,
                    modifiers: egui::Modifiers::NONE,
                },
            ]);
            h.step();
            assert_eq!(h.state().text, "draftx");
            assert_eq!(h.state().submitted, 1);
            assert!(h.state().picked.is_empty());
            assert!(popup(&h).is_none());
        }
    }

    #[test]
    fn history_field_popup_and_close_are_viewport_owned_even_with_same_widget_id() {
        let ctx = Context::default();
        crate::ime_focus::install_ime_input_policy(&ctx);
        crate::ui_fonts::configure_fonts(&ctx);
        let sibling = egui::ViewportId::from_hash_of("history_field_viewport");
        let input = |viewport_id| {
            let mut input = egui::RawInput {
                viewport_id,
                ..Default::default()
            };
            input.viewports.insert(
                sibling,
                egui::ViewportInfo {
                    parent: Some(egui::ViewportId::ROOT),
                    ..Default::default()
                },
            );
            input
        };
        let mut root_arrow_rect = None;
        for viewport in [egui::ViewportId::ROOT, sibling] {
            ctx.run_ui(input(viewport), |ui| {
                let mut focus = true;
                let mut text = String::new();
                store(
                    &ctx,
                    HistoryPopupState {
                        owner: field_id(),
                        highlight: Some(0),
                        drawn_pass: ctx.cumulative_pass_nr(),
                    },
                );
                show_singleline(
                    ui,
                    &mut text,
                    Some(&mut focus),
                    field_id(),
                    &["entry".into()],
                    HistoryFieldOptions::default(),
                    |edit| {
                        edit.desired_width(if viewport == egui::ViewportId::ROOT {
                            180.0
                        } else {
                            280.0
                        })
                    },
                );
                assert!(state(&ctx).is_some());
                let rect_key = arrow_rect_key(viewport, field_id());
                let rect = ctx
                    .data(|data| data.get_temp::<egui::Rect>(rect_key))
                    .unwrap();
                if viewport == egui::ViewportId::ROOT {
                    root_arrow_rect = Some(rect);
                } else {
                    assert_ne!(Some(rect), root_arrow_rect);
                    let root_key = arrow_rect_key(egui::ViewportId::ROOT, field_id());
                    assert_eq!(
                        ctx.data(|data| data.get_temp::<egui::Rect>(root_key)),
                        root_arrow_rect
                    );
                }
            })
            .drop_without_applying_deltas();
        }
        ctx.run_ui(input(sibling), |_ui| {
            close(&ctx);
            assert!(state(&ctx).is_none());
        })
        .drop_without_applying_deltas();
        ctx.run_ui(input(egui::ViewportId::ROOT), |_ui| {
            assert_eq!(state(&ctx).unwrap().owner, field_id());

            let rect_key = arrow_rect_key(egui::ViewportId::ROOT, field_id());
            assert_eq!(
                ctx.data(|data| data.get_temp::<egui::Rect>(rect_key)),
                root_arrow_rect
            );
        })
        .drop_without_applying_deltas();
    }

    #[test]
    #[cfg(windows)]
    fn history_field_native_arrows_enter_numpad_enter_and_escape_stay_in_viewport() {
        let _input = crate::key_input::lock_test_input();
        for (key_to_press, virtual_key, extended) in [
            (Key::ArrowDown, 0x28, false),
            (Key::ArrowUp, 0x26, false),
            (Key::Enter, 0x0D, false),
            (Key::Enter, 0x0D, true),
            (Key::Escape, 0x1B, false),
        ] {
            let mut h = harness(true);
            h.run_steps(3);
            if key_to_press != Key::ArrowDown {
                key(&mut h, Key::ArrowDown);
            }
            let sibling = egui::ViewportId::from_hash_of("history_sibling");
            let edge = |source_viewport| crate::key_input::KeyEdge {
                operation_id: crate::key_input::KeyOperationId::unassigned(),
                source_hwnd: 0,
                source_viewport,
                virtual_key,
                scan_code: 0,
                extended,
                pressed: true,
                repeat: false,
                ctrl: false,
                shift: false,
                alt: false,
            };
            crate::key_input::set_test_routed_frame(vec![
                edge(egui::ViewportId::ROOT),
                edge(sibling),
            ]);
            key(&mut h, key_to_press);
            assert!(!crate::key_input::pressed_key_down(
                egui::ViewportId::ROOT,
                |edge| edge.virtual_key == virtual_key
            ));
            assert!(crate::key_input::pressed_key_down(sibling, |edge| edge
                .virtual_key
                == virtual_key));
            if key_to_press == Key::Enter {
                assert_eq!(h.state().picked, [0]);
            }
            crate::key_input::clear_test_frame();
        }
    }
}
