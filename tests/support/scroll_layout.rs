//! Geometry assertions shared by headless lib and snapshot tests.
//! A PNG records appearance; it cannot decide whether scrolling was necessary.

#[derive(Clone, Copy, Debug)]
pub(crate) struct ScrollLayout {
    pub inner: egui::Rect,
    pub content: egui::Vec2,
    pub offset: egui::Vec2,
}

#[derive(Clone, Copy, Debug)]
pub(crate) enum HeightExpectation {
    Fits,
    Scrolls,
}

impl ScrollLayout {
    pub fn from_output<R>(output: &egui::scroll_area::ScrollAreaOutput<R>) -> Self {
        Self {
            inner: output.inner_rect,
            content: output.content_size,
            offset: output.state.offset,
        }
    }

    // One physical pixel allows egui's rounding without hiding row spacing errors.
    fn tolerance(pixels_per_point: f32) -> f32 {
        1.0 / pixels_per_point
    }

    pub fn assert_height(
        self,
        expected: HeightExpectation,
        bounds: egui::Rect,
        pixels_per_point: f32,
    ) {
        let tolerance = Self::tolerance(pixels_per_point);
        assert!(self.inner.is_positive(), "empty scroll viewport: {self:?}");
        assert!(
            bounds.expand(tolerance).contains_rect(self.inner),
            "viewport outside panel/screen: {self:?}, bounds={bounds:?}"
        );
        assert!(
            self.content.y.is_finite() && self.offset.y.is_finite(),
            "invalid layout: {self:?}"
        );
        match expected {
            HeightExpectation::Fits => {
                assert!(
                    self.content.y <= self.inner.height() + tolerance,
                    "unnecessary vertical scrolling: {self:?}, excess={}",
                    self.content.y - self.inner.height()
                );
                assert!(
                    self.offset.y.abs() <= tolerance,
                    "fitting content retained a scroll offset: {self:?}"
                );
            }
            HeightExpectation::Scrolls => {
                assert!(
                    self.content.y > self.inner.height() + tolerance,
                    "fixture must overflow the constrained viewport: {self:?}"
                );
            }
        }
    }

    /// Call after actual wheel/drag input, not after setting ScrollArea state directly.
    /// Positive offset and the content's bottom in view prove clipped rows remain reachable.
    pub fn assert_scrolled_to_end(self, before: Self, pixels_per_point: f32) {
        let tolerance = Self::tolerance(pixels_per_point);
        assert!(
            self.offset.y > before.offset.y + tolerance,
            "scroll input did not advance: before={before:?}, after={self:?}"
        );
        let max_offset = (self.content.y - self.inner.height()).max(0.0);
        assert!(
            (self.offset.y - max_offset).abs() <= tolerance,
            "scroll input cannot reach the last row: {self:?}, max_offset={max_offset}"
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Positive/negative controls use real egui allocation, including the spacing
    // that caused the fullscreen HUD regression. No snapshot baseline can mask it.
    fn row_layout(spacing: f32, height: f32) -> ScrollLayout {
        let ctx = egui::Context::default();
        let mut measured = None;
        for _ in 0..3 {
            ctx.run_ui(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(400.0, height),
                    )),
                    ..Default::default()
                },
                |ui| {
                    let ctx = &ui.ctx().clone();
                    egui::CentralPanel::default()
                        .frame(egui::Frame::NONE)
                        .show(ui, |ui| {
                            ui.spacing_mut().item_spacing.y = spacing;
                            let output = egui::ScrollArea::vertical()
                                .auto_shrink([false, false])
                                .show(ui, |ui| {
                                    for _ in 0..5 {
                                        ui.allocate_exact_size(
                                            egui::vec2(300.0, 48.0),
                                            egui::Sense::hover(),
                                        );
                                    }
                                });
                            measured = Some(ScrollLayout::from_output(&output));
                        });
                },
            )
            .drop_without_applying_deltas();
        }
        measured.unwrap()
    }

    #[test]
    fn calculated_scroll_height_accepts_matching_rows() {
        row_layout(0.0, 240.0).assert_height(
            HeightExpectation::Fits,
            egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(400.0, 240.0)),
            1.0,
        );
    }

    #[test]
    #[should_panic(expected = "unnecessary vertical scrolling")]
    fn calculated_scroll_height_rejects_unaccounted_item_spacing() {
        row_layout(3.0, 240.0).assert_height(
            HeightExpectation::Fits,
            egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(400.0, 240.0)),
            1.0,
        );
    }
}
