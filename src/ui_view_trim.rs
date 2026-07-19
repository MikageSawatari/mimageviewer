use eframe::egui;

use crate::app::App;
use crate::settings::FullscreenFitMode;
use crate::ui_fullscreen::SpreadPair;
use crate::ui_fullscreen::draw_icons::draw_close_icon;
use crate::view_trim::{
    MAX_VIEW_TRIM_MARGIN, ViewTrimApplyMode, ViewTrimBookSettings, ViewTrimLinkedMargins,
    ViewTrimMargins, ViewTrimPageOverride, ViewTrimSpreadSide,
};

const PANEL_W: f32 = 260.0;
const PANEL_MARGIN: f32 = 14.0;
const PANEL_TOP: f32 = 72.0;
const PANEL_MIN_H: f32 = 250.0;
const PANEL_MAX_H: f32 = 560.0;

fn panel_outer_height(full_rect: egui::Rect, panel_pos: egui::Pos2) -> f32 {
    (full_rect.bottom() - panel_pos.y - PANEL_MARGIN).clamp(PANEL_MIN_H, PANEL_MAX_H)
}

fn linked_from_detected(
    left: Option<ViewTrimMargins>,
    right: Option<ViewTrimMargins>,
) -> Option<ViewTrimLinkedMargins> {
    if left.is_none() && right.is_none() {
        return None;
    }
    let l = left.unwrap_or_default().clamped();
    let r = right.unwrap_or_default().clamped();
    let linked = ViewTrimLinkedMargins {
        top: l.top.min(r.top),
        bottom: l.bottom.min(r.bottom),
        inner: l.right.min(r.left),
        outer: l.left.min(r.right),
    }
    .clamped();
    (!linked.is_zero()).then_some(linked)
}

fn page_override_from_display_bbox(bbox: Option<egui::Rect>) -> ViewTrimPageOverride {
    ViewTrimPageOverride::from_margins(bbox.map(ViewTrimMargins::from_bbox).unwrap_or_default())
}

fn page_overrides_from_auto_spread_bboxes(
    left: Option<egui::Rect>,
    right: Option<egui::Rect>,
) -> (ViewTrimPageOverride, ViewTrimPageOverride) {
    let (left, right) = crate::view_trim::harmonize_spread_auto_bboxes(left, right);
    (
        page_override_from_display_bbox(left),
        page_override_from_display_bbox(right),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn assert_close(actual: f32, expected: f32) {
        assert!(
            (actual - expected).abs() < 1e-6,
            "actual={actual}, expected={expected}"
        );
    }

    #[test]
    fn page_linked_margins_map_inner_and_outer_to_spread_sides() {
        let mut left = ViewTrimPageOverride::default();
        let mut right = ViewTrimPageOverride::default();
        let linked = ViewTrimLinkedMargins {
            top: 0.01,
            bottom: 0.02,
            inner: 0.08,
            outer: 0.03,
        };

        set_page_spread_linked_margins(&mut left, &mut right, linked);

        assert!(left.enabled);
        assert!(right.enabled);
        assert_close(left.margins.left, 0.03);
        assert_close(left.margins.right, 0.08);
        assert_close(right.margins.left, 0.08);
        assert_close(right.margins.right, 0.03);
        assert_close(left.margins.top, 0.01);
        assert_close(right.margins.bottom, 0.02);
        assert_eq!(left.spread_side, Some(ViewTrimSpreadSide::Left));
        assert_eq!(right.spread_side, Some(ViewTrimSpreadSide::Right));
    }

    #[test]
    fn page_switch_to_linked_preserves_inner_and_outer_semantics() {
        let mut left = ViewTrimPageOverride::from_margins(ViewTrimMargins {
            left: 0.02,
            top: 0.04,
            right: 0.08,
            bottom: 0.10,
        });
        let mut right = ViewTrimPageOverride::from_margins(ViewTrimMargins {
            left: 0.12,
            top: 0.06,
            right: 0.16,
            bottom: 0.14,
        });

        set_page_spread_separate(&mut left, &mut right, false);

        assert_close(left.margins.left, 0.09);
        assert_close(left.margins.right, 0.10);
        assert_close(right.margins.left, 0.10);
        assert_close(right.margins.right, 0.09);
        assert_close(left.margins.top, 0.05);
        assert_close(right.margins.top, 0.05);
        assert_close(left.margins.bottom, 0.12);
        assert_close(right.margins.bottom, 0.12);
        assert_eq!(left.spread_side, Some(ViewTrimSpreadSide::Left));
        assert_eq!(right.spread_side, Some(ViewTrimSpreadSide::Right));
    }

    #[test]
    fn page_switch_to_separate_clears_spread_side_semantics() {
        let mut left = ViewTrimPageOverride::from_spread_margins(
            ViewTrimMargins {
                left: 0.02,
                top: 0.04,
                right: 0.08,
                bottom: 0.10,
            },
            ViewTrimSpreadSide::Left,
        );
        let mut right = ViewTrimPageOverride::from_spread_margins(
            ViewTrimMargins {
                left: 0.12,
                top: 0.06,
                right: 0.16,
                bottom: 0.14,
            },
            ViewTrimSpreadSide::Right,
        );

        set_page_spread_separate(&mut left, &mut right, true);

        assert_eq!(left.spread_side, None);
        assert_eq!(right.spread_side, None);
    }

    #[test]
    fn linked_auto_detect_uses_less_aggressive_shared_margins() {
        let linked = linked_from_detected(
            Some(ViewTrimMargins {
                left: 0.02,
                top: 0.12,
                right: 0.08,
                bottom: 0.07,
            }),
            Some(ViewTrimMargins {
                left: 0.04,
                top: 0.03,
                right: 0.10,
                bottom: 0.15,
            }),
        )
        .unwrap();

        assert_close(linked.top, 0.03);
        assert_close(linked.bottom, 0.07);
        assert_close(linked.inner, 0.04);
        assert_close(linked.outer, 0.02);
    }

    #[test]
    fn linked_auto_detect_requires_safe_margins_for_both_pages() {
        let linked = linked_from_detected(
            Some(ViewTrimMargins {
                left: 0.02,
                top: 0.12,
                right: 0.08,
                bottom: 0.07,
            }),
            None,
        );

        assert!(linked.is_none());
    }

    #[test]
    fn page_apply_seed_from_auto_spread_freezes_displayed_auto_trim() {
        let left_bbox = egui::Rect::from_min_max(egui::pos2(0.02, 0.12), egui::pos2(0.92, 0.93));
        let right_bbox = egui::Rect::from_min_max(egui::pos2(0.06, 0.03), egui::pos2(0.98, 0.84));

        let (left, right) =
            page_overrides_from_auto_spread_bboxes(Some(left_bbox), Some(right_bbox));

        assert!(left.enabled);
        assert!(right.enabled);
        assert_close(left.margins.left, 0.02);
        assert_close(left.margins.right, 0.08);
        assert_close(right.margins.left, 0.06);
        assert_close(right.margins.right, 0.02);
        assert_close(left.margins.top, 0.03);
        assert_close(right.margins.top, 0.03);
        assert_close(left.margins.bottom, 0.07);
        assert_close(right.margins.bottom, 0.07);
    }
}

fn set_book_spread_separate(book: &mut ViewTrimBookSettings, separate: bool) {
    if book.spread_separate == separate {
        return;
    }
    if separate {
        let (left, right) = book.spread_linked.to_separate();
        book.spread_left = left;
        book.spread_right = right;
    } else {
        book.spread_linked =
            ViewTrimLinkedMargins::average_from_separate(book.spread_left, book.spread_right);
    }
    book.spread_separate = separate;
}

fn set_page_spread_separate(
    left: &mut ViewTrimPageOverride,
    right: &mut ViewTrimPageOverride,
    separate: bool,
) {
    if separate {
        left.spread_side = None;
        right.spread_side = None;
    } else {
        let linked = ViewTrimLinkedMargins::average_from_separate(left.margins, right.margins);
        let enabled = left.enabled || right.enabled;
        *left = ViewTrimPageOverride::from_spread_margins(
            linked.margins_for(ViewTrimSpreadSide::Left),
            ViewTrimSpreadSide::Left,
        );
        *right = ViewTrimPageOverride::from_spread_margins(
            linked.margins_for(ViewTrimSpreadSide::Right),
            ViewTrimSpreadSide::Right,
        );
        left.enabled = enabled;
        right.enabled = enabled;
    }
}

fn set_page_spread_linked_margins(
    left: &mut ViewTrimPageOverride,
    right: &mut ViewTrimPageOverride,
    linked: ViewTrimLinkedMargins,
) {
    *left = ViewTrimPageOverride::from_spread_margins(
        linked.margins_for(ViewTrimSpreadSide::Left),
        ViewTrimSpreadSide::Left,
    );
    *right = ViewTrimPageOverride::from_spread_margins(
        linked.margins_for(ViewTrimSpreadSide::Right),
        ViewTrimSpreadSide::Right,
    );
}

fn margin_slider(ui: &mut egui::Ui, label: &str, value: &mut f32) -> bool {
    let mut pct = (*value * 100.0).clamp(0.0, MAX_VIEW_TRIM_MARGIN * 100.0);
    let changed = ui
        .add(
            egui::Slider::new(&mut pct, 0.0..=MAX_VIEW_TRIM_MARGIN * 100.0)
                .text(label)
                .suffix(" %"),
        )
        .changed();
    if changed {
        *value = (pct / 100.0).clamp(0.0, MAX_VIEW_TRIM_MARGIN);
    }
    changed
}

fn margins_controls(ui: &mut egui::Ui, title: &str, margins: &mut ViewTrimMargins) -> bool {
    let mut changed = false;
    ui.label(egui::RichText::new(title).color(egui::Color32::from_gray(210)));
    changed |= margin_slider(ui, "上", &mut margins.top);
    changed |= margin_slider(ui, "下", &mut margins.bottom);
    changed |= margin_slider(ui, "左", &mut margins.left);
    changed |= margin_slider(ui, "右", &mut margins.right);
    *margins = margins.clamped();
    changed
}

fn linked_controls(ui: &mut egui::Ui, margins: &mut ViewTrimLinkedMargins) -> bool {
    let mut changed = false;
    ui.label(egui::RichText::new("見開き連動").color(egui::Color32::from_gray(210)));
    changed |= margin_slider(ui, "上", &mut margins.top);
    changed |= margin_slider(ui, "下", &mut margins.bottom);
    changed |= margin_slider(ui, "中央側", &mut margins.inner);
    changed |= margin_slider(ui, "外側", &mut margins.outer);
    *margins = margins.clamped();
    changed
}

fn close_button(ui: &mut egui::Ui) -> bool {
    let (close_rect, close_resp) =
        ui.allocate_exact_size(egui::vec2(26.0, 22.0), egui::Sense::click());
    let close_bg = if close_resp.hovered() {
        egui::Color32::from_rgba_unmultiplied(220, 80, 80, 200)
    } else {
        egui::Color32::from_rgba_unmultiplied(80, 80, 80, 120)
    };
    ui.painter().rect_filled(close_rect, 4.0, close_bg);
    draw_close_icon(ui.painter(), close_rect.center(), 8.0);
    let clicked = close_resp.clicked();
    close_resp.on_hover_text("閉じる");
    clicked
}

impl App {
    pub(crate) fn view_trim_panel_rect(&self, full_rect: egui::Rect) -> egui::Rect {
        let pos = egui::pos2(full_rect.left() + PANEL_MARGIN, full_rect.top() + PANEL_TOP);
        let h = panel_outer_height(full_rect, pos);
        egui::Rect::from_min_size(pos, egui::vec2(PANEL_W, h))
    }

    fn legacy_margin_fit_active_for_view_trim(&self) -> bool {
        matches!(
            self.settings.fullscreen_fit_mode,
            FullscreenFitMode::MarginFit
        ) || self.settings.margin_fit_enabled
    }

    fn view_trim_base_apply_mode(&self) -> ViewTrimApplyMode {
        match self.view_trim_apply_mode {
            ViewTrimApplyMode::Page => ViewTrimApplyMode::None,
            mode => mode,
        }
    }

    fn effective_view_trim_base_apply_mode(&self) -> ViewTrimApplyMode {
        let mode = self.view_trim_base_apply_mode();
        if matches!(mode, ViewTrimApplyMode::None) && self.legacy_margin_fit_active_for_view_trim()
        {
            ViewTrimApplyMode::Auto
        } else {
            mode
        }
    }

    fn view_trim_page_apply_active_for_current_root(&self) -> bool {
        matches!(
            (self.view_trim_page_apply_root_idx, self.fullscreen_idx),
            (Some(apply_idx), Some(root_idx)) if apply_idx == root_idx
        )
    }

    fn clear_stale_view_trim_page_apply(&mut self) {
        if self.view_trim_page_apply_root_idx.is_some()
            && self.view_trim_page_apply_root_idx != self.fullscreen_idx
        {
            self.view_trim_page_apply_root_idx = None;
            self.view_trim_page_spread_separate = self.view_trim_book_settings.spread_separate;
        }
    }

    pub(crate) fn effective_view_trim_apply_mode(&self) -> ViewTrimApplyMode {
        if self.view_trim_page_apply_active_for_current_root() {
            ViewTrimApplyMode::Page
        } else {
            self.effective_view_trim_base_apply_mode()
        }
    }

    pub(crate) fn view_trim_single_bbox(&self, idx: usize) -> Option<egui::Rect> {
        match self.effective_view_trim_apply_mode() {
            ViewTrimApplyMode::Book => self.view_trim_book_settings.single_bbox(),
            ViewTrimApplyMode::Page => self
                .view_trim_page_overrides
                .get(&idx)
                .copied()
                .and_then(ViewTrimPageOverride::bbox),
            ViewTrimApplyMode::None | ViewTrimApplyMode::Auto => None,
        }
    }

    pub(crate) fn view_trim_spread_bbox(
        &self,
        idx: usize,
        side: ViewTrimSpreadSide,
    ) -> Option<egui::Rect> {
        match self.effective_view_trim_apply_mode() {
            ViewTrimApplyMode::Book => self.view_trim_book_settings.spread_bbox(side),
            ViewTrimApplyMode::Page => self
                .view_trim_page_overrides
                .get(&idx)
                .copied()
                .and_then(|p| p.spread_bbox(side)),
            ViewTrimApplyMode::None | ViewTrimApplyMode::Auto => None,
        }
    }

    pub(crate) fn view_trim_single_content_bbox(&mut self, idx: usize) -> Option<egui::Rect> {
        self.clear_stale_view_trim_page_apply();
        match self.effective_view_trim_apply_mode() {
            ViewTrimApplyMode::None => None,
            ViewTrimApplyMode::Auto => self.view_trim_auto_content_bbox(idx, "single"),
            ViewTrimApplyMode::Book | ViewTrimApplyMode::Page => self.view_trim_single_bbox(idx),
        }
    }

    pub(crate) fn view_trim_spread_content_bbox(
        &mut self,
        idx: usize,
        side: ViewTrimSpreadSide,
    ) -> Option<egui::Rect> {
        self.clear_stale_view_trim_page_apply();
        match self.effective_view_trim_apply_mode() {
            ViewTrimApplyMode::None => None,
            ViewTrimApplyMode::Auto => self.view_trim_auto_content_bbox(idx, "spread"),
            ViewTrimApplyMode::Book | ViewTrimApplyMode::Page => {
                self.view_trim_spread_bbox(idx, side)
            }
        }
    }

    pub(crate) fn view_trim_spread_content_bboxes(
        &mut self,
        left_idx: usize,
        right_idx: usize,
    ) -> (Option<egui::Rect>, Option<egui::Rect>) {
        self.clear_stale_view_trim_page_apply();
        match self.effective_view_trim_apply_mode() {
            ViewTrimApplyMode::None => (None, None),
            ViewTrimApplyMode::Auto => crate::view_trim::harmonize_spread_auto_bboxes(
                self.view_trim_auto_content_bbox(left_idx, "spread_pair_left"),
                self.view_trim_auto_content_bbox(right_idx, "spread_pair_right"),
            ),
            ViewTrimApplyMode::Book | ViewTrimApplyMode::Page => (
                self.view_trim_spread_bbox(left_idx, ViewTrimSpreadSide::Left),
                self.view_trim_spread_bbox(right_idx, ViewTrimSpreadSide::Right),
            ),
        }
    }

    fn view_trim_auto_content_bbox(
        &mut self,
        idx: usize,
        reason: &'static str,
    ) -> Option<egui::Rect> {
        #[cfg(windows)]
        if let Some(window_id) = self.detached_view_trim_runtime_window_id() {
            return self.detached_view_trim_auto_content_bbox(window_id, idx, reason);
        }
        self.cached_margin_bbox(idx)
    }

    #[cfg(windows)]
    fn detached_view_trim_runtime_window_id(&self) -> Option<u64> {
        self.viewer_session_is_detached()
            .then_some(self.detached_viewer_window_id)
            .flatten()
    }

    #[cfg(windows)]
    fn detached_view_trim_auto_content_bbox(
        &mut self,
        window_id: u64,
        idx: usize,
        reason: &'static str,
    ) -> Option<egui::Rect> {
        if let Some(baked) = self.detached_window_runtime_trim_bbox(window_id, idx) {
            return baked;
        }

        self.log_detached_image_window_debug(format!(
            "detached_trim_bbox_fallback window_id={window_id} idx={idx} reason={reason}"
        ));

        let has_static_pixels = matches!(
            self.fs_cache.get(&idx),
            Some(crate::fs_animation::FsCacheEntry::Static { .. })
        );
        let bbox = self.cached_margin_bbox(idx);
        if has_static_pixels {
            self.set_detached_window_runtime_trim_bbox(window_id, idx, bbox, reason);
        } else {
            self.log_detached_image_window_debug(format!(
                "detached_trim_bbox_no_bake window_id={window_id} idx={idx} \
                 reason={reason} cause=no_static_pixels"
            ));
        }
        bbox
    }

    pub(crate) fn view_trim_active_for_display(
        &self,
        fs_idx: usize,
        spread_pair: SpreadPair,
    ) -> bool {
        let _ = (fs_idx, spread_pair);
        if self.view_trim_page_apply_root_idx == Some(fs_idx) {
            return true;
        }
        !matches!(
            self.effective_view_trim_base_apply_mode(),
            ViewTrimApplyMode::None
        )
    }

    fn view_trim_single_margins_for_ui(&self, idx: usize) -> ViewTrimMargins {
        self.view_trim_page_overrides
            .get(&idx)
            .map(|p| p.margins)
            .unwrap_or(self.view_trim_book_settings.single)
            .clamped()
    }

    fn view_trim_spread_margins_for_ui(
        &self,
        idx: usize,
        side: ViewTrimSpreadSide,
    ) -> ViewTrimMargins {
        self.view_trim_page_overrides
            .get(&idx)
            .map(|p| p.margins_for_spread_side(side))
            .unwrap_or_else(|| {
                if self.view_trim_book_settings.spread_separate {
                    match side {
                        ViewTrimSpreadSide::Left => self.view_trim_book_settings.spread_left,
                        ViewTrimSpreadSide::Right => self.view_trim_book_settings.spread_right,
                    }
                } else {
                    self.view_trim_book_settings.spread_linked.margins_for(side)
                }
            })
            .clamped()
    }

    fn auto_detect_view_trim_margins(&mut self, idx: usize) -> Option<ViewTrimMargins> {
        self.cached_margin_bbox(idx).map(ViewTrimMargins::from_bbox)
    }

    pub(crate) fn persist_pending_view_trim_state(&mut self) {
        if !self.view_trim_save_pending {
            return;
        }
        self.persist_current_view_trim_book_state();
        let dirty_pages = std::mem::take(&mut self.view_trim_dirty_page_overrides);
        for idx in dirty_pages {
            if let Some(page_override) = self.view_trim_page_overrides.get(&idx).copied() {
                self.persist_view_trim_page_override(idx, page_override);
            }
        }
        self.view_trim_save_pending = false;
    }

    #[allow(clippy::too_many_lines)]
    pub(crate) fn draw_view_trim_controls(
        &mut self,
        ui: &mut egui::Ui,
        ctx: &egui::Context,
        fs_idx: usize,
        spread_pair: SpreadPair,
        body_width: f32,
        body_height: f32,
    ) {
        self.clear_stale_view_trim_page_apply();
        let is_spread = matches!(spread_pair, SpreadPair::Double { .. });
        let had_page_override = match spread_pair {
            SpreadPair::Single => self.view_trim_page_overrides.contains_key(&fs_idx),
            SpreadPair::Double { left, right } => {
                self.view_trim_page_overrides.contains_key(&left)
                    || self.view_trim_page_overrides.contains_key(&right)
            }
        };
        let page_single_has_override = self.view_trim_page_overrides.contains_key(&fs_idx);
        let (page_left_has_override, page_right_has_override) = match spread_pair {
            SpreadPair::Double { left, right } => (
                self.view_trim_page_overrides.contains_key(&left),
                self.view_trim_page_overrides.contains_key(&right),
            ),
            SpreadPair::Single => (page_single_has_override, page_single_has_override),
        };

        let mut base_apply_mode = self.effective_view_trim_base_apply_mode();
        let mut base_mode_changed = false;
        let mut page_apply = self.view_trim_page_apply_active_for_current_root();
        let mut page_spread_separate = if page_apply {
            self.view_trim_page_spread_separate
        } else {
            self.view_trim_book_settings.spread_separate
        };
        let mut page_apply_changed = false;
        let mut apply_mode = if page_apply {
            ViewTrimApplyMode::Page
        } else {
            base_apply_mode
        };
        let mut book = self.view_trim_book_settings;
        let mut page_single =
            ViewTrimPageOverride::from_margins(self.view_trim_single_margins_for_ui(fs_idx));
        if let Some(existing) = self.view_trim_page_overrides.get(&fs_idx).copied() {
            page_single = existing;
        }
        let mut page_left = match spread_pair {
            SpreadPair::Double { left, .. } => self
                .view_trim_page_overrides
                .get(&left)
                .copied()
                .map(|p| p.for_spread_side(ViewTrimSpreadSide::Left))
                .unwrap_or_else(|| {
                    ViewTrimPageOverride::from_margins(
                        self.view_trim_spread_margins_for_ui(left, ViewTrimSpreadSide::Left),
                    )
                }),
            SpreadPair::Single => page_single,
        };
        let mut page_right = match spread_pair {
            SpreadPair::Double { right, .. } => self
                .view_trim_page_overrides
                .get(&right)
                .copied()
                .map(|p| p.for_spread_side(ViewTrimSpreadSide::Right))
                .unwrap_or_else(|| {
                    ViewTrimPageOverride::from_margins(
                        self.view_trim_spread_margins_for_ui(right, ViewTrimSpreadSide::Right),
                    )
                }),
            SpreadPair::Single => page_single,
        };

        let mut changed = false;
        let mut repaint_requested = false;
        let mut auto_requested = false;
        let mut reset_requested = false;
        let mut clear_page_requested = false;

        egui::ScrollArea::vertical()
            .max_height(body_height.max(160.0))
            .auto_shrink([false, false])
            .show(ui, |ui| {
                ui.set_min_width(body_width);
                ui.set_max_width(body_width);

                ui.add_enabled_ui(!page_apply, |ui| {
                    base_mode_changed |= ui
                        .radio_value(&mut base_apply_mode, ViewTrimApplyMode::None, "トリムなし")
                        .changed();
                    base_mode_changed |= ui
                        .radio_value(
                            &mut base_apply_mode,
                            ViewTrimApplyMode::Auto,
                            "自動余白カット",
                        )
                        .changed();
                    base_mode_changed |= ui
                        .radio_value(
                            &mut base_apply_mode,
                            ViewTrimApplyMode::Book,
                            "本全体の設定を適用",
                        )
                        .changed();
                });
                if ui
                    .checkbox(&mut page_apply, "このページの個別設定を適用")
                    .changed()
                {
                    page_apply_changed = true;
                    repaint_requested = true;
                }
                apply_mode = if page_apply {
                    ViewTrimApplyMode::Page
                } else {
                    base_apply_mode
                };
                ui.add_space(6.0);

                match apply_mode {
                    ViewTrimApplyMode::None | ViewTrimApplyMode::Auto => {}
                    ViewTrimApplyMode::Book => {
                        book.enabled = true;
                        if is_spread {
                            let mut spread_separate = book.spread_separate;
                            if ui
                                .checkbox(&mut spread_separate, "見開きの左右を別々に調整")
                                .changed()
                            {
                                set_book_spread_separate(&mut book, spread_separate);
                                changed = true;
                            }
                            ui.add_space(4.0);
                            if book.spread_separate {
                                let left_changed =
                                    margins_controls(ui, "左ページ", &mut book.spread_left);
                                if left_changed {
                                    book.enabled = true;
                                    changed = true;
                                }
                                ui.separator();
                                let right_changed =
                                    margins_controls(ui, "右ページ", &mut book.spread_right);
                                if right_changed {
                                    book.enabled = true;
                                    changed = true;
                                }
                            } else {
                                if linked_controls(ui, &mut book.spread_linked) {
                                    book.enabled = true;
                                    changed = true;
                                }
                            }
                        } else {
                            if margins_controls(ui, "単ページ", &mut book.single) {
                                book.enabled = true;
                                changed = true;
                            }
                        }
                    }
                    ViewTrimApplyMode::Page => {
                        if is_spread {
                            let mut spread_separate = page_spread_separate;
                            if ui
                                .checkbox(&mut spread_separate, "見開きの左右を別々に調整")
                                .changed()
                            {
                                set_page_spread_separate(
                                    &mut page_left,
                                    &mut page_right,
                                    spread_separate,
                                );
                                page_spread_separate = spread_separate;
                                changed = true;
                            }
                            ui.add_space(4.0);
                            if page_spread_separate {
                                page_left.enabled = true;
                                let left_changed =
                                    margins_controls(ui, "左ページ", &mut page_left.margins);
                                if left_changed {
                                    page_left.enabled = true;
                                    changed = true;
                                }
                                ui.separator();
                                page_right.enabled = true;
                                let right_changed =
                                    margins_controls(ui, "右ページ", &mut page_right.margins);
                                if right_changed {
                                    page_right.enabled = true;
                                    changed = true;
                                }
                            } else {
                                page_left.enabled = true;
                                page_right.enabled = true;
                                let mut linked = ViewTrimLinkedMargins::average_from_separate(
                                    page_left.margins,
                                    page_right.margins,
                                );
                                if linked_controls(ui, &mut linked) {
                                    set_page_spread_linked_margins(
                                        &mut page_left,
                                        &mut page_right,
                                        linked,
                                    );
                                    changed = true;
                                }
                            }
                        } else {
                            page_single.enabled = true;
                            if margins_controls(ui, "このページ", &mut page_single.margins) {
                                page_single.enabled = true;
                                changed = true;
                            }
                        }
                    }
                }

                if matches!(
                    apply_mode,
                    ViewTrimApplyMode::Book | ViewTrimApplyMode::Page
                ) {
                    ui.separator();
                    ui.horizontal(|ui| {
                        if ui.button("自動検出").clicked() {
                            auto_requested = true;
                        }
                        if ui.button("リセット").clicked() {
                            reset_requested = true;
                        }
                    });
                    if matches!(apply_mode, ViewTrimApplyMode::Page)
                        && ui.button("このページの個別設定を解除").clicked()
                    {
                        clear_page_requested = true;
                    }
                }
            });

        if base_mode_changed {
            changed = true;
            match base_apply_mode {
                ViewTrimApplyMode::Book => {
                    book.enabled = true;
                }
                ViewTrimApplyMode::None | ViewTrimApplyMode::Auto | ViewTrimApplyMode::Page => {}
            }
            if self.legacy_margin_fit_active_for_view_trim() {
                self.set_fullscreen_fit_mode_for_current(ctx, fs_idx, FullscreenFitMode::Page);
            }
        }
        if page_apply_changed {
            if page_apply {
                self.view_trim_page_apply_root_idx = Some(fs_idx);
                if matches!(base_apply_mode, ViewTrimApplyMode::Auto) {
                    match spread_pair {
                        SpreadPair::Single => {
                            if !page_single_has_override {
                                page_single = page_override_from_display_bbox(
                                    self.cached_margin_bbox(fs_idx),
                                );
                            }
                        }
                        SpreadPair::Double { left, right } => {
                            let left_bbox = self.cached_margin_bbox(left);
                            let right_bbox = self.cached_margin_bbox(right);
                            let (auto_left, auto_right) =
                                page_overrides_from_auto_spread_bboxes(left_bbox, right_bbox);
                            if !page_left_has_override {
                                page_left = auto_left;
                            }
                            if !page_right_has_override {
                                page_right = auto_right;
                            }
                        }
                    }
                }
                page_single.enabled = true;
                page_left.enabled = true;
                page_right.enabled = true;
                changed = true;
            } else {
                self.view_trim_page_apply_root_idx = None;
                page_spread_separate = book.spread_separate;
            }
            apply_mode = if page_apply {
                ViewTrimApplyMode::Page
            } else {
                base_apply_mode
            };
        }
        if auto_requested {
            match (apply_mode, spread_pair) {
                (ViewTrimApplyMode::Book, SpreadPair::Single) => {
                    if let Some(margins) = self.auto_detect_view_trim_margins(fs_idx) {
                        book.enabled = true;
                        book.single = margins;
                        changed = true;
                    } else {
                        self.show_feedback_toast("自動検出できる余白がありません".to_string());
                    }
                }
                (ViewTrimApplyMode::Book, SpreadPair::Double { left, right }) => {
                    let left_m = self.auto_detect_view_trim_margins(left);
                    let right_m = self.auto_detect_view_trim_margins(right);
                    let mut detected = false;
                    if book.spread_separate {
                        if let Some(m) = left_m {
                            book.spread_left = m;
                            detected = true;
                            changed = true;
                        }
                        if let Some(m) = right_m {
                            book.spread_right = m;
                            detected = true;
                            changed = true;
                        }
                    } else if let Some(linked) = linked_from_detected(left_m, right_m) {
                        book.spread_linked = linked;
                        detected = true;
                        changed = true;
                    }
                    if detected {
                        book.enabled = true;
                    } else {
                        self.show_feedback_toast("自動検出できる余白がありません".to_string());
                    }
                }
                (ViewTrimApplyMode::Page, SpreadPair::Single) => {
                    if let Some(margins) = self.auto_detect_view_trim_margins(fs_idx) {
                        page_single = ViewTrimPageOverride::from_margins(margins);
                        changed = true;
                    } else {
                        self.show_feedback_toast("自動検出できる余白がありません".to_string());
                    }
                }
                (ViewTrimApplyMode::Page, SpreadPair::Double { left, right }) => {
                    let left_m = self.auto_detect_view_trim_margins(left);
                    let right_m = self.auto_detect_view_trim_margins(right);
                    let mut detected = false;
                    if page_spread_separate {
                        if let Some(m) = left_m {
                            page_left = ViewTrimPageOverride::from_margins(m);
                            detected = true;
                            changed = true;
                        }
                        if let Some(m) = right_m {
                            page_right = ViewTrimPageOverride::from_margins(m);
                            detected = true;
                            changed = true;
                        }
                    } else if let Some(linked) = linked_from_detected(left_m, right_m) {
                        set_page_spread_linked_margins(&mut page_left, &mut page_right, linked);
                        detected = true;
                        changed = true;
                    }
                    if !detected {
                        self.show_feedback_toast("自動検出できる余白がありません".to_string());
                    }
                }
                (ViewTrimApplyMode::None | ViewTrimApplyMode::Auto, _) => {}
            }
        }
        if reset_requested {
            match (apply_mode, spread_pair) {
                (ViewTrimApplyMode::Book, SpreadPair::Single) => {
                    book.single = ViewTrimMargins::default();
                    book.enabled = true;
                    changed = true;
                }
                (ViewTrimApplyMode::Book, SpreadPair::Double { .. }) => {
                    if book.spread_separate {
                        book.spread_left = ViewTrimMargins::default();
                        book.spread_right = ViewTrimMargins::default();
                    } else {
                        book.spread_linked = ViewTrimLinkedMargins::default();
                    }
                    book.enabled = true;
                    changed = true;
                }
                (ViewTrimApplyMode::Page, SpreadPair::Single) => {
                    page_single.margins = ViewTrimMargins::default();
                    page_single.enabled = true;
                    changed = true;
                }
                (ViewTrimApplyMode::Page, SpreadPair::Double { .. }) => {
                    page_left.margins = ViewTrimMargins::default();
                    page_right.margins = ViewTrimMargins::default();
                    page_left.enabled = true;
                    page_right.enabled = true;
                    changed = true;
                }
                (ViewTrimApplyMode::None | ViewTrimApplyMode::Auto, _) => {}
            }
        }
        if clear_page_requested {
            match spread_pair {
                SpreadPair::Single => {
                    self.remove_view_trim_page_override(fs_idx);
                    self.view_trim_page_overrides.remove(&fs_idx);
                    self.view_trim_dirty_page_overrides.remove(&fs_idx);
                }
                SpreadPair::Double { left, right } => {
                    self.remove_view_trim_page_override(left);
                    self.remove_view_trim_page_override(right);
                    self.view_trim_page_overrides.remove(&left);
                    self.view_trim_page_overrides.remove(&right);
                    self.view_trim_dirty_page_overrides.remove(&left);
                    self.view_trim_dirty_page_overrides.remove(&right);
                }
            }
            self.view_trim_page_apply_root_idx = None;
            self.view_trim_apply_mode = base_apply_mode;
            self.view_trim_page_spread_separate = book.spread_separate;
            self.view_trim_book_settings = book;
            changed = true;
            repaint_requested = true;
        } else {
            self.view_trim_apply_mode = base_apply_mode;
            self.view_trim_page_spread_separate = if page_apply {
                page_spread_separate
            } else {
                book.spread_separate
            };
            self.view_trim_book_settings = book;
            if page_apply
                && (changed
                    || page_apply_changed
                    || auto_requested
                    || reset_requested
                    || had_page_override)
            {
                match spread_pair {
                    SpreadPair::Single => {
                        page_single.enabled = true;
                        self.view_trim_page_overrides.insert(fs_idx, page_single);
                        self.view_trim_dirty_page_overrides.insert(fs_idx);
                    }
                    SpreadPair::Double { left, right } => {
                        page_left.enabled = true;
                        page_right.enabled = true;
                        let mut stored_left = page_left;
                        let mut stored_right = page_right;
                        if page_spread_separate {
                            stored_left.spread_side = None;
                            stored_right.spread_side = None;
                        }
                        self.view_trim_page_overrides.insert(left, stored_left);
                        self.view_trim_page_overrides.insert(right, stored_right);
                        self.view_trim_dirty_page_overrides.insert(left);
                        self.view_trim_dirty_page_overrides.insert(right);
                    }
                }
            }
        }

        if changed {
            self.view_trim_save_pending = true;
        }
        if self.view_trim_save_pending && !ctx.input(|i| i.pointer.primary_down()) {
            self.persist_pending_view_trim_state();
        }

        if changed || repaint_requested {
            ctx.request_repaint();
        }
    }

    pub(crate) fn draw_view_trim_panel(
        &mut self,
        ctx: &egui::Context,
        full_rect: egui::Rect,
        fs_idx: usize,
        spread_pair: SpreadPair,
    ) {
        if !self.view_trim_mode {
            return;
        }

        let panel_rect = self.view_trim_panel_rect(full_rect);
        let panel_pos = panel_rect.min;
        let panel_h = panel_rect.height();
        let mut close = false;

        egui::Area::new(egui::Id::new("view_trim_panel"))
            .order(egui::Order::Foreground)
            .fixed_pos(panel_pos)
            .show(ctx, |ui| {
                ui.interact(
                    egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(PANEL_W + 12.0, panel_h + 12.0),
                    ),
                    egui::Id::new("view_trim_panel_click_sink"),
                    egui::Sense::click_and_drag(),
                );
                egui::Frame::popup(ui.style())
                    .fill(egui::Color32::from_rgba_unmultiplied(20, 20, 20, 230))
                    .stroke(egui::Stroke::new(
                        1.0,
                        egui::Color32::from_rgba_unmultiplied(255, 255, 255, 40),
                    ))
                    .corner_radius(6.0)
                    .show(ui, |ui| {
                        ui.set_min_width(PANEL_W);
                        ui.set_max_width(PANEL_W);
                        ui.set_max_height(panel_h);
                        crate::os_theme::apply_dark_ui(ui);

                        ui.horizontal(|ui| {
                            ui.heading("表示トリム");
                            ui.with_layout(
                                egui::Layout::right_to_left(egui::Align::Center),
                                |ui| {
                                    if close_button(ui) {
                                        close = true;
                                    }
                                },
                            );
                        });
                        ui.separator();
                        self.draw_view_trim_controls(
                            ui,
                            ctx,
                            fs_idx,
                            spread_pair,
                            PANEL_W,
                            (panel_h - 42.0).max(160.0),
                        );
                    });
            });

        if close {
            self.view_trim_mode = false;
            ctx.request_repaint();
        }
    }
}
