//! Layout shared by the toolbar's flexible folder section and its snapshots.

/// Shared rendering for Known Folder destinations; resolving paths stays outside the UI.
pub fn quick_location_button(
    ui: &mut egui::Ui,
    location: &crate::known_folders::QuickLocation,
) -> egui::Response {
    use crate::ui_helpers::HoverTipExt;
    ui.button(location.label)
        .hover_tip(location.path.to_string_lossy().as_ref())
}

/// Logical points; DPI scaling is applied by egui, not by the layout policy.
pub const ADDRESS_INPUT_MIN_WIDTH: f32 = 160.0;

/// Measure only the currently visible controls, in the style used to draw them.
/// Include one item gap per control and the TextEdit's frame margin.
pub struct FolderBarWidth(f32);

impl FolderBarWidth {
    pub fn with_input() -> Self {
        Self(ADDRESS_INPUT_MIN_WIDTH + 8.0) // TextEdit default horizontal margin: 4 + 4.
    }

    /// Start a measurement of controls without an input field.
    pub fn controls() -> Self {
        Self(0.0)
    }

    pub fn label(&mut self, ui: &egui::Ui, text: impl Into<egui::WidgetText>) {
        self.0 += text
            .into()
            .into_galley(
                ui,
                Some(egui::TextWrapMode::Extend),
                f32::INFINITY,
                egui::TextStyle::Body,
            )
            .size()
            .x
            + ui.spacing().item_spacing.x;
    }

    pub fn button(&mut self, ui: &egui::Ui, text: impl Into<egui::WidgetText>, min_width: f32) {
        self.button_with_frame(ui, text, min_width, ui.visuals().button_frame);
    }

    pub fn button_with_frame(
        &mut self,
        ui: &egui::Ui,
        text: impl Into<egui::WidgetText>,
        min_width: f32,
        frame: bool,
    ) {
        let width = text
            .into()
            .into_galley(
                ui,
                Some(egui::TextWrapMode::Extend),
                f32::INFINITY,
                egui::TextStyle::Button,
            )
            .size()
            .x;
        let padding = if frame {
            ui.spacing().button_padding.x * 2.0
        } else {
            0.0
        };
        self.0 += (width + padding).max(min_width) + ui.spacing().item_spacing.x;
    }

    pub fn width(&self) -> f32 {
        self.0
    }

    pub fn space(&mut self, width: f32) {
        self.0 += width;
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FlexibleSectionLayout {
    pub break_before: bool,
    pub width: f32,
    pub compact: bool,
}

pub fn flexible_section_layout(
    full_width: f32,
    remaining_width: f32,
    minimum_width: f32,
    new_row: bool,
) -> FlexibleSectionLayout {
    let full_width = full_width.max(0.0);
    let break_before =
        remaining_width < full_width - 0.5 && (new_row || remaining_width < minimum_width);
    FlexibleSectionLayout {
        break_before,
        width: if break_before {
            full_width
        } else {
            remaining_width.max(0.0)
        },
        compact: full_width < minimum_width,
    }
}

/// Reserve the rest of one row atomically. Subsequent sections always start a new row.
/// The closure receives `compact` when the controls and minimum input cannot fit
/// even on an otherwise empty row; the caller then wraps the controls internally.
pub fn flexible_section<R>(
    ui: &mut egui::Ui,
    minimum_width: f32,
    new_row: bool,
    draw: impl FnOnce(&mut egui::Ui, bool) -> R,
) -> egui::InnerResponse<R> {
    let layout = flexible_section_layout(
        ui.max_rect().width(),
        ui.available_rect_before_wrap().width(),
        minimum_width,
        new_row,
    );
    let previous_bottom = ui.min_rect().bottom();
    // egui's end_row also advances an empty row, so the policy skips it at row start.
    if layout.break_before {
        ui.end_row();
    }
    // Place the child's origin explicitly at the row top. allocate_ui_with_layout
    // would center its initial height in the toolbar row before the child grows,
    // shifting the folder bar when its normal style has a different row height.
    let mut origin = ui.available_rect_before_wrap().min;
    if layout.break_before {
        // Actual widgets can extend beyond the wrapping row's initial frame height.
        // The next section starts after the painted content, just like a new panel.
        origin.y = previous_bottom + ui.spacing().item_spacing.y;
    }
    let rect = egui::Rect::from_min_size(
        origin,
        egui::vec2(layout.width, ui.spacing().interact_size.y),
    );
    let response = ui.scope_builder(
        egui::UiBuilder::new()
            .max_rect(rect)
            .layout(egui::Layout::top_down(egui::Align::Min)),
        |ui| {
            ui.set_min_width(layout.width);
            draw(ui, layout.compact)
        },
    );
    ui.end_row();
    response
}

/// Keep the normal single row; at very small widths the left controls wrap and
/// the right controls/input get their own row in the same section.
pub fn folder_controls<R>(
    ui: &mut egui::Ui,
    compact: bool,
    draw: impl FnOnce(&mut egui::Ui) -> R,
) -> egui::InnerResponse<R> {
    if compact {
        ui.horizontal_wrapped(draw)
    } else {
        ui.horizontal(draw)
    }
}

/// Reserve a compound menu in the wrapping parent before creating its disabled
/// child UI. A bare add_enabled_ui only sees the remaining fragment of the row,
/// so it can wrap the caption internally instead of moving the whole button.
/// This is the same ownership rule as the toolbar's existing combo slots.
pub fn folder_menu_button(
    ui: &mut egui::Ui,
    enabled: bool,
    text: impl Into<egui::WidgetText>,
    contents: impl FnOnce(&mut egui::Ui),
) -> egui::Response {
    let text = text.into();
    let size = text
        .clone()
        .into_galley(
            ui,
            Some(egui::TextWrapMode::Extend),
            f32::INFINITY,
            egui::TextStyle::Button,
        )
        .size();
    let padding = if ui.visuals().button_frame {
        ui.spacing().button_padding * 2.0
    } else {
        egui::Vec2::ZERO
    };
    let size = (size + padding).max(egui::vec2(0.0, ui.spacing().interact_size.y));
    ui.allocate_ui_with_layout(
        size,
        egui::Layout::left_to_right(egui::Align::Center),
        |ui| {
            ui.add_enabled_ui(enabled, |ui| ui.menu_button(text, contents).response)
                .inner
        },
    )
    .inner
}

#[derive(Clone)]
struct FolderMenuSnapshot<T> {
    popup_id: egui::Id,
    value: T,
}

/// Resolve once per open menu, using egui's actual popup state for both reuse
/// and disposal. `source` lets resolution and rendering borrow the same owner
/// sequentially without cloning its settings on every closed frame.
pub(crate) fn folder_menu_button_with_snapshot<S, T: Clone + Send + Sync + 'static>(
    ui: &mut egui::Ui,
    enabled: bool,
    text: impl Into<egui::WidgetText>,
    source: &mut S,
    resolve: impl FnOnce(&mut S) -> T,
    contents: impl FnOnce(&mut S, &mut egui::Ui, &T),
) -> egui::Response {
    let snapshot_id = ui.next_auto_id().with("folder_menu_snapshot");
    let ctx = ui.ctx().clone();
    // A different popup may have closed this one after it was drawn last pass.
    // Check before the button can reopen it, as well as after its close handling.
    let mut snapshot = ctx
        .data(|data| data.get_temp::<FolderMenuSnapshot<T>>(snapshot_id))
        .filter(|snapshot| egui::Popup::is_id_open(&ctx, snapshot.popup_id))
        .map(|snapshot| snapshot.value);
    let response = folder_menu_button(ui, enabled, text, |ui| {
        let value = snapshot.get_or_insert_with(|| resolve(source));
        contents(source, ui, value);
    });
    let popup_id = egui::Popup::default_response_id(&response);
    let is_open = egui::Popup::is_id_open(&ctx, popup_id);
    ctx.data_mut(|data| {
        data.remove::<FolderMenuSnapshot<T>>(snapshot_id);
        if is_open && let Some(value) = snapshot {
            data.insert_temp(snapshot_id, FolderMenuSnapshot { popup_id, value });
        }
    });
    response
}

/// Visual order is always left controls, input, right controls. Decide whether
/// the tail shares the current row, gets one new row, or needs two rows before
/// allocating any right-to-left widgets. A full row includes the input's margin.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FolderTailLayout {
    Inline,
    NewRow,
    SeparateRows,
}

pub fn folder_tail_layout(full: f32, remaining: f32, input: f32, right: f32) -> FolderTailLayout {
    if remaining >= input + right {
        FolderTailLayout::Inline
    } else if full >= input + right {
        FolderTailLayout::NewRow
    } else {
        FolderTailLayout::SeparateRows
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FolderTailPart {
    Input,
    RightControls,
}

/// Keep the former RTL allocation on a single row (right controls first, then
/// the remaining width for the input). When the tail needs separate rows, draw
/// the input first so RTL wrapping cannot move the controls ahead of it.
pub fn folder_tail(
    ui: &mut egui::Ui,
    input_minimum: f32,
    right_width: f32,
    mut draw: impl FnMut(&mut egui::Ui, FolderTailPart),
) {
    let plan = folder_tail_layout(
        ui.max_rect().width(),
        ui.available_rect_before_wrap().width(),
        input_minimum,
        right_width,
    );
    match plan {
        FolderTailLayout::Inline | FolderTailLayout::NewRow => {
            if plan == FolderTailLayout::NewRow {
                ui.end_row();
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                draw(ui, FolderTailPart::RightControls);
                draw(ui, FolderTailPart::Input);
            });
        }
        FolderTailLayout::SeparateRows => {
            ui.end_row();
            ui.with_layout(egui::Layout::left_to_right(egui::Align::Center), |ui| {
                draw(ui, FolderTailPart::Input);
            });
            ui.end_row();
            ui.with_layout(
                egui::Layout::right_to_left(egui::Align::Center).with_main_wrap(true),
                |ui| {
                    draw(ui, FolderTailPart::RightControls);
                },
            );
        }
    }
}

fn address_label_needs_row(width: f32, label_with_gap: f32) -> bool {
    width < label_with_gap + FolderBarWidth::with_input().width()
}

/// Fill the input region chosen by `folder_tail`. An explanatory label precedes
/// the field, but gets its own wrapping row when sharing would shrink the field
/// below its minimum. Without a label, retain the former single-row layout.
pub fn address_input<R>(
    ui: &mut egui::Ui,
    label: Option<egui::WidgetText>,
    draw: impl FnOnce(&mut egui::Ui) -> R,
) -> egui::InnerResponse<R> {
    let width = ui.available_rect_before_wrap().width();
    let label_above = label.as_ref().is_some_and(|text| {
        let mut measured = FolderBarWidth::controls();
        measured.label(ui, text.clone());
        address_label_needs_row(width, measured.width())
    });
    let layout = if label_above {
        egui::Layout::top_down(egui::Align::Min)
    } else {
        egui::Layout::left_to_right(egui::Align::Center)
    };
    ui.allocate_ui_with_layout(
        egui::vec2(width, ui.spacing().interact_size.y),
        layout,
        |ui| {
            if let Some(label) = label {
                ui.add(egui::Label::new(label).wrap_mode(if label_above {
                    egui::TextWrapMode::Wrap
                } else {
                    egui::TextWrapMode::Extend
                }));
            }
            draw(ui)
        },
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Default)]
    struct LocationMenuFixture {
        resolutions: usize,
        current_value: usize,
        rendered: Vec<usize>,
        close_from_contents: bool,
    }

    fn location_menu_frame(
        ctx: &egui::Context,
        source: &mut LocationMenuFixture,
        events: Vec<egui::Event>,
    ) -> egui::Response {
        let mut response = None;
        ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(500.0, 400.0),
                )),
                events,
                ..Default::default()
            },
            |root_ui| {
                egui::CentralPanel::default().show(root_ui, |ui| {
                    response = Some(folder_menu_button_with_snapshot(
                        ui,
                        true,
                        "場所▼",
                        source,
                        |source| {
                            source.resolutions += 1;
                            source.current_value
                        },
                        |source, ui, value| {
                            source.rendered.push(*value);
                            ui.label(value.to_string());
                            if source.close_from_contents {
                                ui.close();
                            }
                        },
                    ));
                });
            },
        )
        .drop_without_applying_deltas();
        response.unwrap()
    }

    fn location_menu_click(pos: egui::Pos2) -> Vec<egui::Event> {
        let mut events = vec![egui::Event::PointerMoved(pos)];
        for pressed in [true, false] {
            events.push(egui::Event::PointerButton {
                pos,
                button: egui::PointerButton::Primary,
                pressed,
                modifiers: egui::Modifiers::NONE,
            });
        }
        events
    }

    #[test]
    fn location_menu_resolves_once_while_open_and_refreshes_after_contents_close() {
        let ctx = egui::Context::default();
        let mut source = LocationMenuFixture::default();
        let response = location_menu_frame(&ctx, &mut source, vec![]);
        let popup_id = egui::Popup::default_response_id(&response);
        assert_eq!(source.resolutions, 0, "closed menus must not resolve");

        egui::Popup::open_id(&ctx, popup_id);
        location_menu_frame(&ctx, &mut source, vec![]);
        source.current_value = 1; // Settings/OS changes apply on the next open.
        for _ in 0..12 {
            location_menu_frame(&ctx, &mut source, vec![]);
        }
        assert_eq!(source.resolutions, 1);
        assert!(source.rendered.len() >= 13);
        assert!(source.rendered.iter().all(|value| *value == 0));

        source.close_from_contents = true;
        location_menu_frame(&ctx, &mut source, vec![]);
        assert!(!egui::Popup::is_id_open(&ctx, popup_id));
        source.close_from_contents = false;
        source.rendered.clear();
        egui::Popup::open_id(&ctx, popup_id);
        location_menu_frame(&ctx, &mut source, vec![]);
        assert_eq!(source.resolutions, 2);
        assert!(source.rendered.iter().all(|value| *value == 1));
    }

    #[test]
    fn location_menu_discards_snapshot_on_escape_outside_click_and_button_toggle() {
        for close in ["escape", "outside", "toggle", "other_popup"] {
            let ctx = egui::Context::default();
            let mut source = LocationMenuFixture::default();
            let response = location_menu_frame(&ctx, &mut source, vec![]);
            let popup_id = egui::Popup::default_response_id(&response);
            egui::Popup::open_id(&ctx, popup_id);
            location_menu_frame(&ctx, &mut source, vec![]);
            location_menu_frame(&ctx, &mut source, vec![]);
            assert_eq!(source.resolutions, 1);

            match close {
                "escape" => {
                    location_menu_frame(
                        &ctx,
                        &mut source,
                        vec![egui::Event::Key {
                            key: egui::Key::Escape,
                            physical_key: None,
                            pressed: true,
                            repeat: false,
                            modifiers: egui::Modifiers::NONE,
                        }],
                    );
                }
                "outside" | "toggle" => {
                    let pos = if close == "toggle" {
                        response.rect.center()
                    } else {
                        egui::pos2(450.0, 350.0)
                    };
                    location_menu_frame(&ctx, &mut source, location_menu_click(pos));
                }
                _ => {
                    // Closed after this button's last draw: no closed frame of
                    // this helper runs before the user's next opening click.
                    egui::Popup::open_id(&ctx, egui::Id::unique("other_popup"));
                }
            }
            assert!(!egui::Popup::is_id_open(&ctx, popup_id), "{close}");
            source.current_value = 1;
            source.rendered.clear();
            // A real opening click has a press pass before its release. In
            // particular, egui must retire the just-closed popup's Area response
            // before classifying the release as a fresh opening click.
            let mut press = location_menu_click(response.rect.center());
            let release = press.pop().unwrap();
            location_menu_frame(&ctx, &mut source, press);
            location_menu_frame(&ctx, &mut source, vec![release]);
            assert!(egui::Popup::is_id_open(&ctx, popup_id), "{close}");
            assert_eq!(source.resolutions, 2, "{close}");
            assert!(!source.rendered.is_empty(), "{close}");
            assert!(source.rendered.iter().all(|value| *value == 1), "{close}");
        }
    }

    #[test]
    fn address_label_yields_a_row_at_the_input_minimum_boundary() {
        assert!(!address_label_needs_row(268.0, 100.0));
        assert!(address_label_needs_row(267.9, 100.0));
        assert!(address_label_needs_row(360.0, 250.0));
        assert!(!address_label_needs_row(720.0, 250.0));
    }

    #[test]
    fn folder_tail_layout_preserves_order_at_the_one_two_and_three_row_boundaries() {
        assert_eq!(
            folder_tail_layout(1000.0, 568.0, 168.0, 400.0),
            FolderTailLayout::Inline
        );
        assert_eq!(
            folder_tail_layout(1000.0, 567.9, 168.0, 400.0),
            FolderTailLayout::NewRow
        );
        assert_eq!(
            folder_tail_layout(568.0, 100.0, 168.0, 400.0),
            FolderTailLayout::NewRow
        );
        assert_eq!(
            folder_tail_layout(567.9, 100.0, 168.0, 400.0),
            FolderTailLayout::SeparateRows
        );
        assert_eq!(
            folder_tail_layout(168.0, 168.0, 168.0, 0.0),
            FolderTailLayout::Inline
        );
    }

    #[test]
    fn folder_section_layout_reserves_remaining_width_and_wraps_at_minimum() {
        assert_eq!(
            flexible_section_layout(900.0, 640.0, 520.0, false),
            FlexibleSectionLayout {
                break_before: false,
                width: 640.0,
                compact: false
            }
        );
        assert_eq!(
            flexible_section_layout(900.0, 519.0, 520.0, false),
            FlexibleSectionLayout {
                break_before: true,
                width: 900.0,
                compact: false
            }
        );
        assert_eq!(
            flexible_section_layout(900.0, 520.0, 520.0, false).width,
            520.0
        );
        assert_eq!(
            flexible_section_layout(900.0, 640.0, 520.0, true).width,
            900.0
        );
        assert!(flexible_section_layout(400.0, 300.0, 520.0, false).compact);
        assert!(!flexible_section_layout(400.0, 400.0, 520.0, true).break_before);
    }
}
