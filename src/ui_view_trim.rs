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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ViewTrimUiMode {
    None,
    Auto,
    Manual,
}

fn view_trim_ui_mode_from_internal(
    base_apply_mode: ViewTrimApplyMode,
    page_apply: bool,
) -> ViewTrimUiMode {
    if page_apply {
        return ViewTrimUiMode::Manual;
    }
    match base_apply_mode {
        ViewTrimApplyMode::None => ViewTrimUiMode::None,
        ViewTrimApplyMode::Auto => ViewTrimUiMode::Auto,
        ViewTrimApplyMode::Book | ViewTrimApplyMode::Page => ViewTrimUiMode::Manual,
    }
}

fn view_trim_internal_from_ui(
    ui_mode: ViewTrimUiMode,
    manual_page_apply: bool,
) -> (ViewTrimApplyMode, bool) {
    match ui_mode {
        ViewTrimUiMode::None => (ViewTrimApplyMode::None, false),
        ViewTrimUiMode::Auto => (ViewTrimApplyMode::Auto, false),
        ViewTrimUiMode::Manual => (ViewTrimApplyMode::Book, manual_page_apply),
    }
}

pub(crate) struct PendingViewTrimTransfer {
    pub(crate) batch: crate::view_trim_db::ViewTrimWriteBatch,
    pub(crate) dirty_page_indices: Vec<usize>,
}

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

/// ページ範囲を選んだときの編集値を解決する唯一の優先順位。
///
/// 保存済み個別値が正本で、未作成のページだけ自動検出値、最後に本全体値を初期値として使う。
/// 見開きでは保存時の左右 semantics を現在の表示側へ変換してから返す。
fn resolve_page_override_for_scope(
    existing: Option<ViewTrimPageOverride>,
    book_fallback: ViewTrimMargins,
    auto_seed: Option<ViewTrimPageOverride>,
    spread_side: Option<ViewTrimSpreadSide>,
) -> ViewTrimPageOverride {
    let page_override = existing
        .or(auto_seed)
        .unwrap_or_else(|| ViewTrimPageOverride::from_margins(book_fallback));
    spread_side
        .map(|side| page_override.for_spread_side(side))
        .unwrap_or(page_override)
}

fn book_spread_margins(book: ViewTrimBookSettings, side: ViewTrimSpreadSide) -> ViewTrimMargins {
    if book.spread_separate {
        match side {
            ViewTrimSpreadSide::Left => book.spread_left,
            ViewTrimSpreadSide::Right => book.spread_right,
        }
    } else {
        book.spread_linked.margins_for(side)
    }
}

fn enabled_page_override(
    page_override: Option<ViewTrimPageOverride>,
) -> Option<ViewTrimPageOverride> {
    page_override.filter(|page_override| page_override.enabled)
}

fn view_trim_page_scope_selected(
    page_overrides: &std::collections::HashMap<usize, ViewTrimPageOverride>,
    editor_root_idx: usize,
    spread_pair: SpreadPair,
) -> bool {
    match spread_pair {
        SpreadPair::Single => {
            enabled_page_override(page_overrides.get(&editor_root_idx).copied()).is_some()
        }
        SpreadPair::Double { left, right } => {
            enabled_page_override(page_overrides.get(&left).copied()).is_some()
                || enabled_page_override(page_overrides.get(&right).copied()).is_some()
        }
    }
}

fn page_override_for_editor(
    existing: Option<ViewTrimPageOverride>,
    book_fallback: ViewTrimMargins,
    spread_side: Option<ViewTrimSpreadSide>,
) -> ViewTrimPageOverride {
    let existing = enabled_page_override(existing);
    let mut page_override =
        resolve_page_override_for_scope(existing, book_fallback, None, spread_side);
    page_override.enabled = existing.is_some();
    page_override
}

fn page_spread_separate_from_overrides(
    left: Option<ViewTrimPageOverride>,
    right: Option<ViewTrimPageOverride>,
    book_fallback: bool,
) -> bool {
    let enabled = [enabled_page_override(left), enabled_page_override(right)];
    if enabled.iter().all(Option::is_none) {
        book_fallback
    } else {
        enabled
            .into_iter()
            .flatten()
            .any(|page_override| page_override.spread_side.is_none())
    }
}

fn store_page_scope_overrides(
    page_overrides: &mut std::collections::HashMap<usize, ViewTrimPageOverride>,
    dirty_page_overrides: &mut std::collections::HashSet<usize>,
    editor_root_idx: usize,
    spread_pair: SpreadPair,
    page_single: ViewTrimPageOverride,
    page_left: ViewTrimPageOverride,
    page_right: ViewTrimPageOverride,
    spread_separate: bool,
) {
    match spread_pair {
        SpreadPair::Single => {
            if page_single.enabled {
                page_overrides.insert(editor_root_idx, page_single);
                dirty_page_overrides.insert(editor_root_idx);
            }
        }
        SpreadPair::Double { left, right } => {
            let mut stored_left = page_left;
            let mut stored_right = page_right;
            if spread_separate {
                stored_left.spread_side = None;
                stored_right.spread_side = None;
            }
            if stored_left.enabled {
                page_overrides.insert(left, stored_left);
                dirty_page_overrides.insert(left);
            }
            if stored_right.enabled {
                page_overrides.insert(right, stored_right);
                dirty_page_overrides.insert(right);
            }
        }
    }
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

    #[test]
    fn page_scope_reselection_restores_existing_single_override() {
        let saved = ViewTrimPageOverride::from_margins(ViewTrimMargins {
            left: 0.11,
            top: 0.04,
            right: 0.07,
            bottom: 0.03,
        });
        let changed_book = ViewTrimMargins {
            left: 0.02,
            ..Default::default()
        };

        let restored = resolve_page_override_for_scope(Some(saved), changed_book, None, None);

        assert_eq!(restored, saved);
    }

    #[test]
    fn book_edit_on_other_page_does_not_change_page_override() {
        let original_page = ViewTrimPageOverride::from_margins(ViewTrimMargins {
            left: 0.13,
            bottom: 0.06,
            ..Default::default()
        });
        let book_after_other_page_edit = ViewTrimMargins {
            top: 0.09,
            right: 0.08,
            ..Default::default()
        };

        let restored = resolve_page_override_for_scope(
            Some(original_page),
            book_after_other_page_edit,
            None,
            None,
        );

        assert_eq!(restored, original_page);
    }

    #[test]
    fn page_scope_reselection_restores_double_overrides() {
        let left = ViewTrimPageOverride::from_spread_margins(
            ViewTrimMargins {
                left: 0.03,
                top: 0.04,
                right: 0.12,
                bottom: 0.05,
            },
            ViewTrimSpreadSide::Left,
        );
        let right = ViewTrimPageOverride::from_spread_margins(
            ViewTrimMargins {
                left: 0.14,
                top: 0.06,
                right: 0.02,
                bottom: 0.07,
            },
            ViewTrimSpreadSide::Right,
        );

        let restored_left = resolve_page_override_for_scope(
            Some(left),
            ViewTrimMargins::default(),
            None,
            Some(ViewTrimSpreadSide::Left),
        );
        let restored_right = resolve_page_override_for_scope(
            Some(right),
            ViewTrimMargins::default(),
            None,
            Some(ViewTrimSpreadSide::Right),
        );

        assert_eq!(restored_left, left);
        assert_eq!(restored_right, right);
    }

    #[test]
    fn auto_page_seed_never_overwrites_existing_override() {
        let saved = ViewTrimPageOverride::from_margins(ViewTrimMargins {
            left: 0.05,
            top: 0.03,
            ..Default::default()
        });
        let auto_seed = ViewTrimPageOverride::from_margins(ViewTrimMargins {
            left: 0.18,
            top: 0.17,
            right: 0.16,
            bottom: 0.15,
        });

        let restored = resolve_page_override_for_scope(
            Some(saved),
            ViewTrimMargins::default(),
            Some(auto_seed),
            None,
        );

        assert_eq!(restored, saved);
        assert_eq!(
            resolve_page_override_for_scope(
                None,
                ViewTrimMargins::default(),
                Some(auto_seed),
                None,
            ),
            auto_seed
        );
    }

    #[test]
    fn page_scope_selection_is_derived_from_enabled_override() {
        let editor_root_idx = 12;
        let mut page_overrides = std::collections::HashMap::new();
        page_overrides.insert(
            editor_root_idx,
            ViewTrimPageOverride::from_margins(ViewTrimMargins::default()),
        );

        assert!(view_trim_page_scope_selected(
            &page_overrides,
            editor_root_idx,
            SpreadPair::Single,
        ));
        page_overrides.get_mut(&editor_root_idx).unwrap().enabled = false;
        assert!(!view_trim_page_scope_selected(
            &page_overrides,
            editor_root_idx,
            SpreadPair::Single,
        ));
    }

    #[test]
    fn page_scope_edit_with_distinct_fullscreen_idx_stores_only_page_override() {
        let editor_root_idx = 12;
        let fullscreen_idx = 4;
        let book = ViewTrimBookSettings {
            enabled: true,
            single: ViewTrimMargins {
                left: 0.02,
                ..Default::default()
            },
            ..Default::default()
        };
        let original_book = book;
        let page = ViewTrimPageOverride::from_margins(ViewTrimMargins {
            top: 0.11,
            right: 0.07,
            ..Default::default()
        });
        let mut page_overrides = std::collections::HashMap::new();
        let mut dirty = std::collections::HashSet::new();

        assert_ne!(editor_root_idx, fullscreen_idx);
        store_page_scope_overrides(
            &mut page_overrides,
            &mut dirty,
            editor_root_idx,
            SpreadPair::Single,
            page,
            ViewTrimPageOverride::default(),
            ViewTrimPageOverride::default(),
            false,
        );

        assert_eq!(book, original_book);
        assert_eq!(page_overrides.get(&editor_root_idx), Some(&page));
        assert!(dirty.contains(&editor_root_idx));
        assert!(!page_overrides.contains_key(&fullscreen_idx));
    }

    #[test]
    fn page_scope_selection_follows_overrides_across_navigation() {
        let old_editor_root_idx = 12;
        let new_editor_root_idx = 13;
        let page_overrides = std::collections::HashMap::from([(
            old_editor_root_idx,
            ViewTrimPageOverride::from_margins(ViewTrimMargins::default()),
        )]);

        // 旧仕様の「移動したら一時スコープを解除」ではなく、各ページの保存行から
        // 毎回導出するため、戻ったときは選択操作なしで Page に戻る。
        assert!(view_trim_page_scope_selected(
            &page_overrides,
            old_editor_root_idx,
            SpreadPair::Single,
        ));
        assert!(!view_trim_page_scope_selected(
            &page_overrides,
            new_editor_root_idx,
            SpreadPair::Single,
        ));
        assert!(view_trim_page_scope_selected(
            &page_overrides,
            old_editor_root_idx,
            SpreadPair::Single,
        ));
    }

    #[test]
    fn double_page_scope_stores_and_restores_both_sides_after_navigation() {
        let left_idx = 20;
        let right_idx = 21;
        let left = ViewTrimPageOverride::from_spread_margins(
            ViewTrimMargins {
                left: 0.03,
                top: 0.04,
                right: 0.12,
                bottom: 0.05,
            },
            ViewTrimSpreadSide::Left,
        );
        let right = ViewTrimPageOverride::from_spread_margins(
            ViewTrimMargins {
                left: 0.14,
                top: 0.06,
                right: 0.02,
                bottom: 0.07,
            },
            ViewTrimSpreadSide::Right,
        );
        let mut page_overrides = std::collections::HashMap::new();
        let mut dirty = std::collections::HashSet::new();

        store_page_scope_overrides(
            &mut page_overrides,
            &mut dirty,
            left_idx,
            SpreadPair::Double {
                left: left_idx,
                right: right_idx,
            },
            ViewTrimPageOverride::default(),
            left,
            right,
            false,
        );
        let restored_left = resolve_page_override_for_scope(
            page_overrides.get(&left_idx).copied(),
            ViewTrimMargins::default(),
            None,
            Some(ViewTrimSpreadSide::Left),
        );
        let restored_right = resolve_page_override_for_scope(
            page_overrides.get(&right_idx).copied(),
            ViewTrimMargins::default(),
            None,
            Some(ViewTrimSpreadSide::Right),
        );

        assert_eq!(restored_left, left);
        assert_eq!(restored_right, right);
        assert_eq!(
            dirty,
            std::collections::HashSet::from([left_idx, right_idx])
        );
    }

    #[test]
    fn double_page_scope_preserves_one_sided_override() {
        let left_idx = 20;
        let right_idx = 21;
        let left = ViewTrimPageOverride::from_margins(ViewTrimMargins {
            left: 0.08,
            ..Default::default()
        });
        let mut page_overrides = std::collections::HashMap::new();
        let mut dirty = std::collections::HashSet::new();

        store_page_scope_overrides(
            &mut page_overrides,
            &mut dirty,
            left_idx,
            SpreadPair::Double {
                left: left_idx,
                right: right_idx,
            },
            ViewTrimPageOverride::default(),
            left,
            ViewTrimPageOverride::default(),
            true,
        );

        assert!(view_trim_page_scope_selected(
            &page_overrides,
            left_idx,
            SpreadPair::Double {
                left: left_idx,
                right: right_idx,
            },
        ));
        assert_eq!(page_overrides.get(&left_idx), Some(&left));
        assert!(!page_overrides.contains_key(&right_idx));
        assert_eq!(dirty, std::collections::HashSet::from([left_idx]));
    }

    #[test]
    fn view_trim_ui_state_maps_to_existing_internal_modes() {
        assert_eq!(
            view_trim_internal_from_ui(ViewTrimUiMode::None, true),
            (ViewTrimApplyMode::None, false)
        );
        assert_eq!(
            view_trim_internal_from_ui(ViewTrimUiMode::Auto, true),
            (ViewTrimApplyMode::Auto, false)
        );
        assert_eq!(
            view_trim_internal_from_ui(ViewTrimUiMode::Manual, false),
            (ViewTrimApplyMode::Book, false)
        );
        assert_eq!(
            view_trim_internal_from_ui(ViewTrimUiMode::Manual, true),
            (ViewTrimApplyMode::Book, true)
        );
    }

    #[test]
    fn view_trim_internal_modes_map_to_three_ui_modes() {
        assert_eq!(
            view_trim_ui_mode_from_internal(ViewTrimApplyMode::None, false),
            ViewTrimUiMode::None
        );
        assert_eq!(
            view_trim_ui_mode_from_internal(ViewTrimApplyMode::Auto, false),
            ViewTrimUiMode::Auto
        );
        assert_eq!(
            view_trim_ui_mode_from_internal(ViewTrimApplyMode::Book, false),
            ViewTrimUiMode::Manual
        );
        assert_eq!(
            view_trim_ui_mode_from_internal(ViewTrimApplyMode::Book, true),
            ViewTrimUiMode::Manual
        );
    }
}

fn set_book_spread_separate(book: &mut ViewTrimBookSettings, separate: bool) {
    *book = book.with_spread_separate(separate);
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

    fn effective_view_trim_base_apply_mode(&self) -> ViewTrimApplyMode {
        crate::view_trim::effective_view_trim_base_apply_mode(
            self.view_trim_apply_mode,
            self.legacy_margin_fit_active_for_view_trim(),
        )
    }

    pub(crate) fn effective_view_trim_apply_mode_for_idx(&self, idx: usize) -> ViewTrimApplyMode {
        // 手動設定時は enabled なページ個別行そのものを永続 Page 選択として扱い、
        // ページを表示するたび自動適用する。一時状態で「今このページを編集中か」を持つと、
        // ページ移動で個別設定が黙って無効化され、ページごとに設定を持つ意味が失われる
        // (表示トリム実装時の見落とし。2026-07-28 修正)。
        // view_trim_pages はリリース済みデータなので削除・移行はしない。旧実装では
        // 「チェックしている間だけ効く」値だった行が、以後は常に効くようになる。
        crate::view_trim::effective_view_trim_apply_mode(
            self.effective_view_trim_base_apply_mode(),
            self.view_trim_page_overrides.get(&idx).copied(),
        )
    }

    pub(crate) fn view_trim_single_bbox(&self, idx: usize) -> Option<egui::Rect> {
        self.stored_view_trim_bbox_for_idx(idx, None)
    }

    pub(crate) fn view_trim_spread_bbox(
        &self,
        idx: usize,
        side: ViewTrimSpreadSide,
    ) -> Option<egui::Rect> {
        self.stored_view_trim_bbox_for_idx(idx, Some(side))
    }

    /// 保存値からの bbox 解決は remote 表示と共有する。ここに本体専用の枝を足すと
    /// 端末側だけ違う位置で切れるので、優先順位は `view_trim::stored_view_trim_bbox`
    /// 側だけで持つ。
    fn stored_view_trim_bbox_for_idx(
        &self,
        idx: usize,
        side: Option<ViewTrimSpreadSide>,
    ) -> Option<egui::Rect> {
        crate::view_trim::stored_view_trim_bbox(
            self.effective_view_trim_apply_mode_for_idx(idx),
            self.view_trim_book_settings,
            self.view_trim_page_overrides.get(&idx).copied(),
            side,
        )
    }

    pub(crate) fn view_trim_single_content_bbox(&mut self, idx: usize) -> Option<egui::Rect> {
        match self.effective_view_trim_apply_mode_for_idx(idx) {
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
        match self.effective_view_trim_apply_mode_for_idx(idx) {
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
        match self.effective_view_trim_base_apply_mode() {
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
        _fs_idx: usize,
        _spread_pair: SpreadPair,
    ) -> bool {
        !matches!(
            self.effective_view_trim_base_apply_mode(),
            ViewTrimApplyMode::None
        )
    }

    fn auto_detect_view_trim_margins(&mut self, idx: usize) -> Option<ViewTrimMargins> {
        self.cached_margin_bbox(idx).map(ViewTrimMargins::from_bbox)
    }

    pub(crate) fn remove_view_trim_page_overrides_for_spread(
        &mut self,
        editor_root_idx: usize,
        spread_pair: SpreadPair,
    ) {
        let indices = match spread_pair {
            SpreadPair::Single => vec![editor_root_idx],
            SpreadPair::Double { left, right } => vec![left, right],
        };
        for idx in indices {
            self.remove_view_trim_page_override(idx);
            self.view_trim_page_overrides.remove(&idx);
            self.view_trim_dirty_page_overrides.remove(&idx);
        }
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

    /// 明示メタ情報転送用に未保存値をメモリだけで切り離す。SQLite への書き込みは
    /// transfer worker が行う。spawn / worker preparation が失敗した場合は
    /// [`Self::restore_pending_view_trim_transfer`] で dirty 状態を戻す。
    pub(crate) fn take_pending_view_trim_transfer(&mut self) -> Option<PendingViewTrimTransfer> {
        if !self.view_trim_save_pending {
            return None;
        }
        let book = self.spread_container_key().map(|key| {
            let state = crate::view_trim::ViewTrimBookState {
                apply_mode: match self.view_trim_apply_mode {
                    ViewTrimApplyMode::Page => ViewTrimApplyMode::None,
                    mode => mode,
                },
                book_settings: self.view_trim_book_settings,
            };
            (key, state)
        });
        let mut dirty_page_indices = self
            .view_trim_dirty_page_overrides
            .iter()
            .copied()
            .collect::<Vec<_>>();
        dirty_page_indices.sort_unstable();
        let pages = dirty_page_indices
            .iter()
            .filter_map(|&idx| {
                Some((
                    self.page_path_key(idx)?,
                    *self.view_trim_page_overrides.get(&idx)?,
                ))
            })
            .collect();

        self.view_trim_dirty_page_overrides.clear();
        self.view_trim_save_pending = false;
        Some(PendingViewTrimTransfer {
            batch: crate::view_trim_db::ViewTrimWriteBatch { book, pages },
            dirty_page_indices,
        })
    }

    pub(crate) fn restore_pending_view_trim_transfer(&mut self, pending: PendingViewTrimTransfer) {
        self.view_trim_dirty_page_overrides
            .extend(pending.dirty_page_indices);
        self.view_trim_save_pending = true;
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
        let is_spread = matches!(spread_pair, SpreadPair::Double { .. });
        let had_any_page_override = match spread_pair {
            SpreadPair::Single => self.view_trim_page_overrides.contains_key(&fs_idx),
            SpreadPair::Double { left, right } => {
                self.view_trim_page_overrides.contains_key(&left)
                    || self.view_trim_page_overrides.contains_key(&right)
            }
        };
        let page_single_existing = self.view_trim_page_overrides.get(&fs_idx).copied();
        let (page_left_existing, page_right_existing) = match spread_pair {
            SpreadPair::Double { left, right } => (
                self.view_trim_page_overrides.get(&left).copied(),
                self.view_trim_page_overrides.get(&right).copied(),
            ),
            SpreadPair::Single => (page_single_existing, page_single_existing),
        };

        let mut base_apply_mode = self.effective_view_trim_base_apply_mode();
        let page_scope_selected =
            view_trim_page_scope_selected(&self.view_trim_page_overrides, fs_idx, spread_pair);
        let mut page_apply =
            matches!(base_apply_mode, ViewTrimApplyMode::Book) && page_scope_selected;
        let mut ui_mode = view_trim_ui_mode_from_internal(base_apply_mode, page_apply);
        let mut base_mode_changed = false;
        let mut page_spread_separate = match spread_pair {
            SpreadPair::Double { .. } if page_apply => page_spread_separate_from_overrides(
                page_left_existing,
                page_right_existing,
                self.view_trim_book_settings.spread_separate,
            ),
            _ => self.view_trim_book_settings.spread_separate,
        };
        // Auto → Manual → このページは 2 クリックになるため、Auto から Manual へ入った
        // 事実だけ UI 一時状態に保持し、次の page_apply ON で従来の自動余白シードを使う。
        let auto_page_seed_pending_id = egui::Id::new(("view_trim_auto_page_seed_pending", fs_idx));
        let mut auto_page_seed_pending = ui.ctx().data(|data| {
            data.get_temp::<bool>(auto_page_seed_pending_id)
                .unwrap_or(false)
        });
        let mut apply_mode = if page_apply {
            ViewTrimApplyMode::Page
        } else {
            base_apply_mode
        };
        let mut book = self.view_trim_book_settings;
        let mut page_single = page_override_for_editor(page_single_existing, book.single, None);
        let mut page_left = match spread_pair {
            SpreadPair::Double { .. } => page_override_for_editor(
                page_left_existing,
                book_spread_margins(book, ViewTrimSpreadSide::Left),
                Some(ViewTrimSpreadSide::Left),
            ),
            SpreadPair::Single => page_single,
        };
        let mut page_right = match spread_pair {
            SpreadPair::Double { .. } => page_override_for_editor(
                page_right_existing,
                book_spread_margins(book, ViewTrimSpreadSide::Right),
                Some(ViewTrimSpreadSide::Right),
            ),
            SpreadPair::Single => page_single,
        };

        let mut changed = false;
        let mut repaint_requested = false;
        let mut auto_requested = false;
        let mut reset_requested = false;
        let mut clear_page_requested = false;
        let mut create_page_requested = false;
        let mut page_values_changed = false;

        egui::ScrollArea::vertical()
            .max_height(body_height.max(160.0))
            .auto_shrink([false, false])
            .show(ui, |ui| {
                ui.set_min_width(body_width);
                ui.set_max_width(body_width);

                // 旧 UI はモード (何をするか) と適用範囲 (どこへ保存するか) を
                // 同じラジオ列と独立 checkbox に混在させ、間に scope control が
                // 挟まるため、続くスライダーの書き込み先が読み取りにくかった。
                let ui_mode_before = ui_mode;
                ui.radio_value(&mut ui_mode, ViewTrimUiMode::None, "トリムなし");
                ui.radio_value(
                    &mut ui_mode,
                    ViewTrimUiMode::Auto,
                    "自動余白カット（本全体）",
                )
                .on_hover_text("本全体に適用します。余白量はページごとに自動検出します");
                ui.radio_value(&mut ui_mode, ViewTrimUiMode::Manual, "手動設定");

                if ui_mode != ui_mode_before {
                    auto_page_seed_pending =
                        ui_mode == ViewTrimUiMode::Manual && ui_mode_before == ViewTrimUiMode::Auto;
                    let (next_base_mode, _) = view_trim_internal_from_ui(ui_mode, page_apply);
                    base_mode_changed = next_base_mode != base_apply_mode;
                    base_apply_mode = next_base_mode;
                    page_apply = ui_mode == ViewTrimUiMode::Manual && page_scope_selected;
                    repaint_requested = true;
                }

                if ui_mode == ViewTrimUiMode::Manual {
                    ui.horizontal(|ui| {
                        ui.add_space(12.0);
                        ui.label("適用範囲：");
                        let book_clicked = ui.selectable_label(!page_apply, "本全体").clicked();
                        let page_clicked = ui.selectable_label(page_apply, "このページ").clicked();
                        if book_clicked && (page_apply || had_any_page_override) {
                            page_apply = false;
                            clear_page_requested = true;
                            repaint_requested = true;
                        } else if page_clicked && !page_apply {
                            page_apply = true;
                            create_page_requested = true;
                            repaint_requested = true;
                        }
                    });
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
                                page_values_changed = true;
                            }
                            ui.add_space(4.0);
                            if page_spread_separate {
                                let left_changed =
                                    margins_controls(ui, "左ページ", &mut page_left.margins);
                                if left_changed {
                                    page_left.enabled = true;
                                    changed = true;
                                    page_values_changed = true;
                                }
                                ui.separator();
                                let right_changed =
                                    margins_controls(ui, "右ページ", &mut page_right.margins);
                                if right_changed {
                                    page_right.enabled = true;
                                    changed = true;
                                    page_values_changed = true;
                                }
                            } else {
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
                                    page_values_changed = true;
                                }
                            }
                        } else {
                            if margins_controls(ui, "このページ", &mut page_single.margins) {
                                page_single.enabled = true;
                                changed = true;
                                page_values_changed = true;
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
        if create_page_requested {
            if auto_page_seed_pending {
                match spread_pair {
                    SpreadPair::Single => {
                        let auto_seed =
                            page_override_from_display_bbox(self.cached_margin_bbox(fs_idx));
                        page_single = resolve_page_override_for_scope(
                            enabled_page_override(page_single_existing),
                            book.single,
                            Some(auto_seed),
                            None,
                        );
                    }
                    SpreadPair::Double { left, right } => {
                        let left_bbox = self.cached_margin_bbox(left);
                        let right_bbox = self.cached_margin_bbox(right);
                        let (auto_left, auto_right) =
                            page_overrides_from_auto_spread_bboxes(left_bbox, right_bbox);
                        page_left = resolve_page_override_for_scope(
                            enabled_page_override(page_left_existing),
                            book_spread_margins(book, ViewTrimSpreadSide::Left),
                            Some(auto_left),
                            Some(ViewTrimSpreadSide::Left),
                        );
                        page_right = resolve_page_override_for_scope(
                            enabled_page_override(page_right_existing),
                            book_spread_margins(book, ViewTrimSpreadSide::Right),
                            Some(auto_right),
                            Some(ViewTrimSpreadSide::Right),
                        );
                    }
                }
                auto_page_seed_pending = false;
            }
            page_single.enabled = true;
            page_left.enabled = true;
            page_right.enabled = true;
            page_values_changed = true;
            changed = true;
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
                        page_values_changed = true;
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
                            page_values_changed = true;
                        }
                        if let Some(m) = right_m {
                            page_right = ViewTrimPageOverride::from_margins(m);
                            detected = true;
                            changed = true;
                            page_values_changed = true;
                        }
                    } else if let Some(linked) = linked_from_detected(left_m, right_m) {
                        set_page_spread_linked_margins(&mut page_left, &mut page_right, linked);
                        detected = true;
                        changed = true;
                        page_values_changed = true;
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
                    page_values_changed = true;
                }
                (ViewTrimApplyMode::Page, SpreadPair::Double { .. }) => {
                    page_left.margins = ViewTrimMargins::default();
                    page_right.margins = ViewTrimMargins::default();
                    page_left.enabled = true;
                    page_right.enabled = true;
                    changed = true;
                    page_values_changed = true;
                }
                (ViewTrimApplyMode::None | ViewTrimApplyMode::Auto, _) => {}
            }
        }
        if clear_page_requested {
            self.remove_view_trim_page_overrides_for_spread(fs_idx, spread_pair);
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
            if page_apply && page_values_changed {
                store_page_scope_overrides(
                    &mut self.view_trim_page_overrides,
                    &mut self.view_trim_dirty_page_overrides,
                    fs_idx,
                    spread_pair,
                    page_single,
                    page_left,
                    page_right,
                    page_spread_separate,
                );
            }
        }

        // Manual の本全体を実際に編集したら、Auto 由来の一時シード候補は古くなる。
        if changed && ui_mode == ViewTrimUiMode::Manual && !page_apply && !base_mode_changed {
            auto_page_seed_pending = false;
        }
        ui.ctx()
            .data_mut(|data| data.insert_temp(auto_page_seed_pending_id, auto_page_seed_pending));

        if changed {
            self.view_trim_save_pending = true;
        }
        if self.view_trim_save_pending && !ctx.input(|i| i.pointer.primary_down()) {
            self.persist_pending_view_trim_state();
        }

        if changed || repaint_requested {
            self.reanchor_continuous_reading_after_view_trim_change(fs_idx);
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
