//! Layout shared by the toolbar's flexible folder section and its snapshots.

/// Logical points; DPI scaling is applied by egui, not by the layout policy.
pub const ADDRESS_INPUT_MIN_WIDTH: f32 = 160.0;

/// Measure only the currently visible controls, in the style used to draw them.
/// Include one item gap per control and the TextEdit's frame margin.
pub struct FolderBarWidth(f32);

impl FolderBarWidth {
    pub fn new(ui: &egui::Ui) -> Self {
        Self(ADDRESS_INPUT_MIN_WIDTH + ui.spacing().button_padding.x * 2.0)
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
        self.0 += (width + ui.spacing().button_padding.x * 2.0).max(min_width)
            + ui.spacing().item_spacing.x;
    }

    pub fn space(&mut self, width: f32) {
        self.0 += width;
    }

    pub fn minimum(&self) -> f32 {
        self.0.ceil()
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

pub fn address_input<R>(
    ui: &mut egui::Ui,
    compact: bool,
    draw: impl FnOnce(&mut egui::Ui) -> R,
) -> egui::InnerResponse<R> {
    if compact && ui.available_rect_before_wrap().width() < ui.max_rect().width() - 0.5 {
        ui.end_row();
    }
    let width = ui.available_rect_before_wrap().width();
    ui.allocate_ui_with_layout(
        egui::vec2(width, ui.spacing().interact_size.y),
        if compact {
            egui::Layout::top_down(egui::Align::Min)
        } else {
            egui::Layout::left_to_right(egui::Align::Center)
        },
        draw,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

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
