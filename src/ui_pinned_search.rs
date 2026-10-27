//! The toolbar owns placement; search transactions stay in App.
use crate::settings::{Settings, ToolbarSectionId};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum PinnedSearchKind {
    Container,
    Local,
    Metadata,
    Everything,
}

impl PinnedSearchKind {
    pub const ALL: [Self; 4] = [
        Self::Container,
        Self::Local,
        Self::Metadata,
        Self::Everything,
    ];
    pub fn section(self) -> ToolbarSectionId {
        match self {
            Self::Container => ToolbarSectionId::SearchContainer,
            Self::Local => ToolbarSectionId::SearchLocal,
            Self::Metadata => ToolbarSectionId::SearchMetadata,
            Self::Everything => ToolbarSectionId::SearchEverything,
        }
    }
    pub fn from_section(section: ToolbarSectionId) -> Option<Self> {
        Self::ALL.into_iter().find(|kind| kind.section() == section)
    }
    pub fn pinned(self, settings: &Settings) -> bool {
        match self {
            Self::Container => settings.show_toolbar_search_container,
            Self::Local => settings.show_toolbar_search_local,
            Self::Metadata => settings.show_toolbar_search_metadata,
            Self::Everything => settings.show_toolbar_search_everything,
        }
    }
    pub fn label(self) -> &'static str {
        match self {
            Self::Container => "コンテナ検索",
            Self::Local => "現在地の絞り込み",
            Self::Metadata => "アイテム検索",
            Self::Everything => "Everything 検索",
        }
    }
    pub fn input_id(self) -> egui::Id {
        egui::Id::unique(("pinned_search_input", self))
    }
    pub fn temporary_id(self) -> egui::Id {
        egui::Id::unique(("temporary_search_input", self))
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PinnedSearchState {
    Inactive,
    Applied,
    Edited,
    Running,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PinnedSearchAction {
    Edited(PinnedSearchKind),
    Submit(PinnedSearchKind),
    Clear(PinnedSearchKind),
    Focused(PinnedSearchKind),
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SearchSectionWidth {
    pub break_before: bool,
    pub stacked: bool,
    pub width: f32,
    pub input: f32,
}

/// `controls` includes the measured label, status, buttons and their intervening gaps.
/// `frame` includes TextEdit margins and the section's accent reservation.
pub fn search_section_width(
    full: f32,
    remaining: f32,
    controls: f32,
    frame: f32,
    gap: f32,
) -> SearchSectionWidth {
    let minimum = controls + gap + 140.0 + frame;
    let break_before = remaining + 0.5 < minimum && remaining < full - 0.5;
    let available = if break_before { full } else { remaining };
    let stacked = full + 0.5 < minimum;
    let input = if stacked {
        (available - frame).clamp(140.0, 220.0)
    } else {
        (available - controls - gap - frame).clamp(140.0, 220.0)
    };
    SearchSectionWidth {
        break_before,
        stacked,
        width: if stacked {
            available
        } else {
            controls + gap + input + frame
        },
        input,
    }
}

pub struct SearchFieldResponse {
    pub label: egui::Response,
    pub input: egui::Response,
    pub clear: egui::Response,
    pub submit: Option<egui::Response>,
    pub actions: Vec<PinnedSearchAction>,
}

/// Pure drawing: edits the sole draft buffer and returns owner-tagged intents.
pub fn draw_pinned_search(
    ui: &mut egui::Ui,
    kind: PinnedSearchKind,
    draft: &mut String,
    state: PinnedSearchState,
    enabled: Result<(), &'static str>,
    focus: bool,
    enter: bool,
    drag: bool,
) -> egui::InnerResponse<SearchFieldResponse> {
    let gap = ui.spacing().item_spacing.x;
    let text_width = |text: &str, style| {
        egui::WidgetText::from(text)
            .into_galley(ui, Some(egui::TextWrapMode::Extend), f32::INFINITY, style)
            .size()
            .x
    };
    let status = match state {
        PinnedSearchState::Edited => "未適用",
        PinnedSearchState::Running => "検索中",
        _ => "",
    };
    let button_width =
        |text| text_width(text, egui::TextStyle::Button) + ui.spacing().button_padding.x * 2.0;
    let controls = text_width(kind.label(), egui::TextStyle::Body)
        + gap
        + button_width("×")
        + if kind == PinnedSearchKind::Everything {
            gap + button_width("検索")
        } else {
            0.0
        }
        + if status.is_empty() {
            0.0
        } else {
            gap + text_width(status, egui::TextStyle::Body)
        };
    const MARGIN: f32 = 2.0;
    const TEXT_MARGIN: f32 = 4.0;
    let width = search_section_width(
        ui.max_rect().width(),
        ui.available_rect_before_wrap().width(),
        controls,
        (MARGIN + TEXT_MARGIN + 1.0) * 2.0,
        gap,
    );
    let previous_bottom = ui.min_rect().bottom();
    if width.break_before {
        ui.end_row();
    }
    let mut origin = ui.available_rect_before_wrap().min;
    if width.break_before {
        origin.y = previous_bottom + ui.spacing().item_spacing.y;
    }
    let rect = egui::Rect::from_min_size(
        origin,
        egui::vec2(width.width, ui.spacing().interact_size.y),
    );
    ui.scope_builder(
        egui::UiBuilder::new()
            .max_rect(rect)
            .layout(egui::Layout::top_down(egui::Align::Min)),
        |ui| {
            ui.set_min_width(width.width);
            let stroke = if state == PinnedSearchState::Applied {
                egui::Stroke::new(1.0, ui.visuals().selection.stroke.color)
            } else {
                egui::Stroke::new(1.0, egui::Color32::TRANSPARENT)
            };
            egui::Frame::new()
                .inner_margin(MARGIN)
                .stroke(stroke)
                .show(ui, |ui| {
                    let mut actions = Vec::new();
                    let mut label = None;
                    let mut input = None;
                    let mut clear = None;
                    let mut submit = None;
                    let mut draw_input =
                        |ui: &mut egui::Ui, actions: &mut Vec<PinnedSearchAction>| {
                            let mut request = focus && enabled.is_ok();
                            let output = ui
                                .add_enabled_ui(enabled.is_ok(), |ui| {
                                    let mut output = crate::ime_focus::show_singleline(
                                        ui,
                                        draft,
                                        Some(&mut request),
                                        |edit| {
                                            edit.id(kind.input_id())
                                                .desired_width(width.input + TEXT_MARGIN * 2.0)
                                                .min_size(egui::vec2(
                                                    width.input + TEXT_MARGIN * 2.0,
                                                    20.0,
                                                ))
                                                .margin(egui::Margin::symmetric(4, 2))
                                        },
                                    );
                                    crate::ui_helpers::set_accessibility_label(
                                        &output.response,
                                        kind.label(),
                                    );
                                    if focus
                                        && kind == PinnedSearchKind::Everything
                                        && !crate::ime_focus::ime_input_active(ui.ctx())
                                    {
                                        let range = egui::text::CCursorRange::two(
                                            egui::text::CCursor::new(0),
                                            egui::text::CCursor::new(draft.chars().count()),
                                        );
                                        output.state.cursor.set_char_range(Some(range));
                                        output.state.clone().store(ui.ctx(), output.response.id);
                                        output.cursor_range = Some(range);
                                    }
                                    let changed =
                                        crate::ui_helpers::singleline_text_edit_context_menu(
                                            ui,
                                            &mut output,
                                            draft,
                                        );
                                    if output.response.changed() || changed {
                                        actions.push(PinnedSearchAction::Edited(kind));
                                    }
                                    if output.response.has_focus() {
                                        actions.push(PinnedSearchAction::Focused(kind));
                                    }
                                    if output.response.lost_focus() && enter {
                                        actions.push(PinnedSearchAction::Submit(kind));
                                    }
                                    output.response.response
                                })
                                .inner;
                            if let Err(reason) = enabled {
                                output.clone().on_disabled_hover_text(reason);
                            }
                            output
                        };
                    let header = |ui: &mut egui::Ui| {
                        // The label owns section gestures, never text selection. Keep the
                        // TextEdit's independent focus and drag selection unchanged.
                        label = Some(
                            ui.add(egui::Label::new(kind.label()).selectable(false).sense(
                                if drag {
                                    egui::Sense::click_and_drag()
                                } else {
                                    egui::Sense::click()
                                },
                            )),
                        );
                        if !width.stacked {
                            input = Some(draw_input(ui, &mut actions));
                        }
                        if !status.is_empty() {
                            ui.label(status);
                        }
                        if kind == PinnedSearchKind::Everything {
                            let response =
                                ui.add_enabled(enabled.is_ok(), egui::Button::new("検索"));
                            if response.clicked() {
                                actions.push(PinnedSearchAction::Submit(kind));
                            }
                            submit = Some(response);
                        }
                        let response = ui
                            .add_enabled(enabled.is_ok(), egui::Button::new("×"))
                            .on_hover_text("検索条件を解除");
                        if response.clicked() {
                            actions.push(PinnedSearchAction::Clear(kind));
                        }
                        clear = Some(response);
                    };
                    if width.stacked {
                        ui.horizontal_wrapped(header);
                    } else {
                        ui.horizontal(header);
                    }
                    if width.stacked {
                        input = Some(draw_input(ui, &mut actions));
                    }
                    SearchFieldResponse {
                        label: label.unwrap(),
                        input: input.unwrap(),
                        clear: clear.unwrap(),
                        submit,
                        actions,
                    }
                })
                .inner
        },
    )
}

#[doc(hidden)]
pub fn draw_pinned_search_snapshot_fixture(ui: &mut egui::Ui) {
    ui.scope(|ui| {
        crate::ui_main::apply_toolbar_style(ui);
        ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Extend);
        ui.horizontal_wrapped(|ui| {
            crate::ui_toolbar_layout::flexible_section(ui, 240.0, true, |ui, _| {
                ui.horizontal(|ui| {
                    ui.label("フォルダ:");
                    ui.label(r"C:\Pictures");
                });
            });
            for (kind, state, text) in [
                (
                    PinnedSearchKind::Container,
                    PinnedSearchState::Inactive,
                    "写真集",
                ),
                (PinnedSearchKind::Local, PinnedSearchState::Applied, "旅行"),
                (
                    PinnedSearchKind::Metadata,
                    PinnedSearchState::Edited,
                    "青空",
                ),
                (
                    PinnedSearchKind::Everything,
                    PinnedSearchState::Inactive,
                    "ext:jpg",
                ),
            ] {
                draw_pinned_search(
                    ui,
                    kind,
                    &mut text.to_owned(),
                    state,
                    Ok(()),
                    false,
                    false,
                    false,
                );
            }
        });
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn section1450_label_drag_does_not_select_text_and_preserves_gestures() {
        use std::{cell::RefCell, rc::Rc};
        for kind in PinnedSearchKind::ALL {
            for drag in [false, true] {
                let observed = Rc::new(RefCell::new((
                    egui::Rect::NOTHING,
                    false,
                    false,
                    false,
                    egui::Rect::NOTHING,
                )));
                let render = observed.clone();
                let mut draft = "search text".to_owned();
                let fonts = std::cell::Cell::new(false);
                let mut harness = crate::test_ui::builder()
                    .with_size(egui::vec2(600.0, 160.0))
                    .build_ui(move |ui| {
                        if !fonts.replace(true) {
                            crate::ui_fonts::configure_fonts(ui.ctx());
                            ui.ctx().request_repaint();
                            return;
                        }
                        let field = draw_pinned_search(
                            ui,
                            kind,
                            &mut draft,
                            PinnedSearchState::Inactive,
                            Ok(()),
                            false,
                            false,
                            drag,
                        )
                        .inner;
                        let mut seen = render.borrow_mut();
                        seen.0 = field.label.rect;
                        seen.1 |= field.label.drag_started();
                        seen.2 |= field.label.clicked();
                        seen.3 |= field.label.secondary_clicked();
                        seen.4 = field.input.rect;
                    });
                harness.run();
                let rect = observed.borrow().0;
                let start = egui::pos2(rect.left() + 2.0, rect.center().y);
                let end = egui::pos2(rect.right() - 2.0, rect.center().y);
                harness.event(egui::Event::PointerMoved(start));
                harness.event(egui::Event::PointerButton {
                    pos: start,
                    button: egui::PointerButton::Primary,
                    pressed: true,
                    modifiers: egui::Modifiers::NONE,
                });
                harness.event(egui::Event::PointerMoved(end));
                harness.event(egui::Event::PointerButton {
                    pos: end,
                    button: egui::PointerButton::Primary,
                    pressed: false,
                    modifiers: egui::Modifiers::NONE,
                });
                harness.run();
                assert!(
                    !harness
                        .ctx
                        .plugin::<egui::text_selection::LabelSelectionState>()
                        .lock()
                        .has_selection(),
                    "{kind:?}, drag={drag}"
                );
                assert_eq!(observed.borrow().1, drag, "{kind:?}");
                for button in [egui::PointerButton::Primary, egui::PointerButton::Secondary] {
                    harness.event(egui::Event::PointerMoved(start));
                    for pressed in [true, false] {
                        harness.event(egui::Event::PointerButton {
                            pos: start,
                            button,
                            pressed,
                            modifiers: egui::Modifiers::NONE,
                        });
                    }
                }
                harness.run();
                {
                    let seen = observed.borrow();
                    assert!(
                        seen.2 && seen.3,
                        "click and context menu gestures: {kind:?}"
                    );
                }
                let input_rect = observed.borrow().4;
                let start = input_rect.left_center() + egui::vec2(5.0, 0.0);
                let end = start + egui::vec2(45.0, 0.0);
                harness.event(egui::Event::PointerMoved(start));
                harness.event(egui::Event::PointerButton {
                    pos: start,
                    button: egui::PointerButton::Primary,
                    pressed: true,
                    modifiers: egui::Modifiers::NONE,
                });
                harness.event(egui::Event::PointerMoved(end));
                harness.event(egui::Event::PointerButton {
                    pos: end,
                    button: egui::PointerButton::Primary,
                    pressed: false,
                    modifiers: egui::Modifiers::NONE,
                });
                harness.run();
                let state =
                    egui::text_edit::TextEditState::load(&harness.ctx, kind.input_id()).unwrap();
                assert!(
                    !state.cursor.char_range().unwrap().is_empty(),
                    "input drag selection: {kind:?}"
                );
                assert!(harness.ctx.memory(|m| m.has_focus(kind.input_id())));
            }
        }
    }
    #[test]
    fn section1409_width_wraps_whole_section_then_stacks_on_full_row() {
        for full in [1120.0, 720.0, 360.0] {
            let width = search_section_width(full, 80.0, 120.0, 14.0, 8.0);
            assert!(width.break_before);
            assert!(!width.stacked);
            assert_eq!(width.input, 220.0_f32.min(full - 142.0));
            assert!(width.width <= full);
        }
        let narrow = search_section_width(250.0, 250.0, 120.0, 14.0, 8.0);
        assert!(!narrow.break_before);
        assert!(narrow.stacked);
        assert_eq!(narrow.input, 220.0);
        let shrink = search_section_width(300.0, 300.0, 120.0, 14.0, 8.0);
        assert_eq!(shrink.input, 158.0);
        assert_eq!(shrink.width, 300.0);
    }
    #[test]
    fn section1409_real_render_fields_buttons_and_folder_fit_1120_720_360_240() {
        use std::{cell::RefCell, rc::Rc};
        for width in [1120.0, 720.0, 360.0, 240.0] {
            let rects = Rc::new(RefCell::new(Vec::new()));
            let render = rects.clone();
            let fonts = std::cell::Cell::new(false);
            let mut harness = crate::test_ui::builder()
                .with_size(egui::vec2(width, 500.0))
                .build_ui(move |root| {
                    if !fonts.replace(true) {
                        crate::ui_fonts::configure_fonts(root.ctx());
                        root.ctx().request_repaint();
                        return;
                    }
                    egui::CentralPanel::default().show(root, |ui| {
                        render.borrow_mut().clear();
                        ui.horizontal_wrapped(|ui| {
                            let folder = crate::ui_toolbar_layout::flexible_section(
                                ui,
                                240.0,
                                true,
                                |ui, _| {
                                    ui.label("フォルダ:");
                                },
                            );
                            render.borrow_mut().push((
                                None,
                                folder.response.rect,
                                folder.response.rect,
                                folder.response.rect,
                                None,
                            ));
                            for kind in PinnedSearchKind::ALL {
                                let response = draw_pinned_search(
                                    ui,
                                    kind,
                                    &mut "下書き".to_owned(),
                                    PinnedSearchState::Edited,
                                    Ok(()),
                                    false,
                                    false,
                                    false,
                                );
                                render.borrow_mut().push((
                                    Some(kind),
                                    response.response.rect,
                                    response.inner.input.rect,
                                    response.inner.clear.rect,
                                    response.inner.submit.map(|r| r.rect),
                                ));
                            }
                        });
                    });
                });
            harness.run();
            let rects = rects.borrow();
            let folder = rects[0].1;
            for (kind, section, input, clear, submit) in rects.iter().skip(1) {
                assert!(input.width() >= 148.0, "{width} {kind:?} {input:?}");
                assert!(
                    section.left() >= 0.0 && section.right() <= width + 0.5,
                    "{width} {kind:?} {section:?}"
                );
                assert!(section.contains_rect(*input));
                assert!(section.contains_rect(*clear));
                assert!(section.top() >= folder.bottom());
                assert!(!input.intersects(*clear));
                if let Some(submit) = submit {
                    assert!(section.contains_rect(*submit));
                    assert!(!input.intersects(*submit));
                }
            }
        }
    }
}
