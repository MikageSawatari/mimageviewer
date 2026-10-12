//! Shared, side-effect-free overview drawing. The owning viewer applies returned stamped commands.
//! Texture lookup visits only bounded layout cells; this module never creates/loads textures.

use std::collections::HashMap;
use std::ops::Range;

use eframe::egui::{self, Color32, Pos2, Rect, Sense, Stroke, Vec2};

use crate::page_overview::{
    PageOverviewDirection, PageOverviewLayout, PageOverviewStamp, PageOverviewTarget,
    PageOverviewView, WhiteCompanion,
};

/// Caller-resolved edited/rotated static thumbnail. UV corners are TL, TR, BR, BL.
#[derive(Clone, Copy, Debug)]
pub enum PageOverviewThumbnail {
    Loaded {
        texture_id: egui::TextureId,
        display_size: Vec2,
        uv_corners: [Pos2; 4],
    },
    Failed,
}

pub struct PageOverviewRenderInput<'a> {
    pub view: &'a PageOverviewView,
    pub title: &'a str,
    /// The bounded real pages in the current display unit (normally one page or a spread).
    pub current_items_indices: &'a [usize],
    pub thumbnails: Option<&'a HashMap<usize, PageOverviewThumbnail>>,
    pub direction: PageOverviewDirection,
    /// The caller resolves modal/popup/surface input ownership. False means drawing only.
    pub input_enabled: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PageOverviewUiAction {
    Select(PageOverviewTarget),
    Cancel,
    ScrollRows(isize),
    ColumnsDelta(i8),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PageOverviewUiCommand {
    pub stamp: PageOverviewStamp,
    pub action: PageOverviewUiAction,
}

#[derive(Clone, Copy, Debug)]
pub struct PageOverviewGeometry {
    pub surface: Rect,
    pub header: Rect,
    pub back_button: Rect,
    pub title: Rect,
    pub position: Rect,
    pub columns_label: Rect,
    pub columns_less: Rect,
    pub columns_more: Rect,
    pub scrollbar: Rect,
    pub layout: PageOverviewLayout,
}

pub struct PageOverviewRenderOutput {
    pub commands: Vec<PageOverviewUiCommand>,
    pub layout: PageOverviewLayout,
    pub visible_ordinals: Range<usize>,
    pub overscan_ordinals: Range<usize>,
    pub surface_rect: Rect,
    pub back_button_rect: Rect,
}

/// Shared before-draw geometry lets the owner reflow its session before drawing a resized surface.
pub fn page_overview_geometry(
    surface: Rect,
    view: &PageOverviewView,
    direction: PageOverviewDirection,
) -> PageOverviewGeometry {
    let padding = 4.0;
    let header_height = 60.0_f32.min(surface.height().max(0.0));
    let header = Rect::from_min_max(
        surface.min,
        egui::pos2(surface.right(), surface.top() + header_height),
    );
    let button_height = 24.0_f32.min((header.height() - 2.0 * padding).max(0.0));
    let back_width: f32 = if surface.width() < 360.0 { 64.0 } else { 140.0 };
    let back_button = Rect::from_min_size(
        surface.min + egui::vec2(padding, padding),
        egui::vec2(
            back_width.min((surface.width() - 2.0 * padding).max(0.0)),
            button_height,
        ),
    );
    let title = Rect::from_min_max(
        egui::pos2(
            (back_button.right() + 8.0).min(surface.right()),
            back_button.top(),
        ),
        egui::pos2(
            (surface.right() - padding - 28.0).max(back_button.right()),
            back_button.bottom(),
        ),
    );
    let toolbar_y = back_button.bottom() + 4.0;
    let control_width = 24.0_f32.min((surface.width() / 4.0).max(0.0));
    let more_x = (surface.right() - padding - control_width).max(surface.left());
    let columns_more = Rect::from_min_size(
        egui::pos2(more_x, toolbar_y),
        egui::vec2(control_width, 24.0),
    )
    .intersect(header);
    let columns_label = Rect::from_min_size(
        egui::pos2((more_x - 40.0).max(surface.left()), toolbar_y),
        egui::vec2(36.0, 24.0),
    )
    .intersect(header);
    let columns_less = Rect::from_min_size(
        egui::pos2(
            (columns_label.left() - 4.0 - control_width).max(surface.left()),
            toolbar_y,
        ),
        egui::vec2(control_width, 24.0),
    )
    .intersect(header);
    let position = Rect::from_min_max(
        egui::pos2(surface.left() + padding, toolbar_y),
        egui::pos2(
            (columns_less.left() - 6.0).max(surface.left() + padding),
            (toolbar_y + 24.0).min(header.bottom()),
        ),
    );
    let body = Rect::from_min_max(egui::pos2(surface.left(), header.bottom()), surface.max);
    let scrollbar_width = 18.0_f32.min(body.width().max(0.0));
    let scrollbar = Rect::from_min_max(
        egui::pos2(body.right() - scrollbar_width, body.top()),
        body.max,
    );
    let grid = Rect::from_min_max(body.min, egui::pos2(scrollbar.left(), body.bottom()));
    PageOverviewGeometry {
        surface,
        header,
        back_button,
        title,
        position,
        columns_label,
        columns_less,
        columns_more,
        scrollbar,
        layout: view.layout(grid, direction),
    }
}

fn truncated_text(
    ui: &egui::Ui,
    painter: &egui::Painter,
    rect: Rect,
    text: &str,
    color: Color32,
    size: f32,
) {
    if rect.width() <= 0.0 || rect.height() <= 0.0 {
        return;
    }
    let mut job = egui::text::LayoutJob::simple_singleline(
        text.to_owned(),
        egui::FontId::proportional(size),
        color,
    );
    job.wrap = egui::text::TextWrapping::truncate_at_width(rect.width());
    let galley = ui.fonts_mut(|fonts| fonts.layout_job(job));
    painter.with_clip_rect(rect).galley(
        egui::pos2(rect.left(), rect.center().y - galley.size().y / 2.0),
        galley,
        color,
    );
}

fn button(
    ui: &mut egui::Ui,
    rect: Rect,
    id: egui::Id,
    text: &str,
    enabled: bool,
    input_enabled: bool,
) -> bool {
    if !rect.is_positive() {
        return false;
    }
    let response = ui.interact(
        rect,
        id,
        if enabled && input_enabled {
            Sense::CLICK
        } else {
            Sense::hover()
        },
    );
    let style = ui.style().interact(&response);
    let fill = if enabled {
        style.bg_fill
    } else {
        ui.visuals().widgets.noninteractive.bg_fill
    };
    let color = if enabled {
        ui.visuals().text_color()
    } else {
        ui.visuals().weak_text_color()
    };
    ui.painter()
        .rect(rect, 4.0, fill, style.bg_stroke, egui::StrokeKind::Inside);
    let mut job = egui::text::LayoutJob::simple_singleline(
        text.to_owned(),
        egui::FontId::proportional(13.0),
        color,
    );
    job.wrap = egui::text::TextWrapping::truncate_at_width((rect.width() - 4.0).max(0.0));
    let galley = ui.fonts_mut(|fonts| fonts.layout_job(job));
    ui.painter()
        .with_clip_rect(rect)
        .galley(rect.center() - galley.size() / 2.0, galley, color);
    enabled && input_enabled && primary_click_in_drawn_rect(ui, rect)
}

/// Resolve the current drawn geometry, including cells exposed by this pass's resize.
/// Widget click flags use the previous interaction layout and can miss a new placeholder.
fn primary_click_in_drawn_rect(ui: &egui::Ui, rect: Rect) -> bool {
    ui.rect_contains_pointer(rect)
        && ui.input(|input| input.pointer.button_clicked(egui::PointerButton::Primary))
}

fn clamped_scroll_delta(
    view: &PageOverviewView,
    layout: &PageOverviewLayout,
    delta: isize,
) -> isize {
    let top = layout.clamp_anchor(view.scroll_anchor).row;
    let next = top
        .saturating_add_signed(delta)
        .min(layout.max_scroll_row());
    if next >= top {
        (next - top).min(isize::MAX as usize) as isize
    } else {
        -((top - next).min(isize::MAX as usize) as isize)
    }
}

fn wheel_actions(
    events: &[egui::Event],
    view: &PageOverviewView,
    layout: &PageOverviewLayout,
) -> Vec<PageOverviewUiAction> {
    let mut rows = 0_isize;
    let mut columns = 0_i8;
    for event in events {
        let egui::Event::MouseWheel {
            unit,
            delta,
            modifiers,
        } = event
        else {
            continue;
        };
        if !delta.y.is_finite() || delta.y == 0.0 {
            continue;
        }
        let direction = if delta.y > 0.0 { -1 } else { 1 };
        if modifiers.ctrl {
            // Positive Ctrl-wheel zooms in by showing fewer columns.
            columns = columns.saturating_add(direction as i8);
        } else {
            let step = if *unit == egui::MouseWheelUnit::Page {
                layout.page_rows().min(isize::MAX as usize) as isize
            } else {
                1
            };
            rows = rows.saturating_add(direction * step);
        }
    }
    let mut actions = Vec::new();
    let delta = clamped_scroll_delta(view, layout, rows);
    if delta != 0 {
        actions.push(PageOverviewUiAction::ScrollRows(delta));
    }
    let preferred = i16::from(view.preferred_columns);
    let columns = ((preferred + i16::from(columns)).clamp(1, 10) - preferred) as i8;
    if columns != 0 {
        actions.push(PageOverviewUiAction::ColumnsDelta(columns));
    }
    actions
}

fn draw_thumbnail(painter: &egui::Painter, rect: Rect, thumbnail: PageOverviewThumbnail) -> bool {
    let PageOverviewThumbnail::Loaded {
        texture_id,
        display_size,
        uv_corners,
    } = thumbnail
    else {
        return false;
    };
    if !display_size.is_finite()
        || display_size.x <= 0.0
        || display_size.y <= 0.0
        || !rect.is_positive()
    {
        return false;
    }
    let scale = (rect.width() / display_size.x).min(rect.height() / display_size.y);
    let target = Rect::from_center_size(rect.center(), display_size * scale);
    let mut mesh = egui::Mesh::with_texture(texture_id);
    for (pos, uv) in [
        target.left_top(),
        target.right_top(),
        target.right_bottom(),
        target.left_bottom(),
    ]
    .into_iter()
    .zip(uv_corners)
    {
        mesh.vertices.push(egui::epaint::Vertex {
            pos,
            uv,
            color: Color32::WHITE,
        });
    }
    mesh.indices.extend_from_slice(&[0, 1, 2, 0, 2, 3]);
    painter.add(egui::Shape::mesh(mesh));
    true
}

pub fn draw_page_overview(
    ui: &mut egui::Ui,
    input: PageOverviewRenderInput<'_>,
) -> PageOverviewRenderOutput {
    let surface = ui.available_rect_before_wrap();
    let geometry = page_overview_geometry(surface, input.view, input.direction);
    let layout = geometry.layout;
    let view = input.view;
    let anchor = layout.clamp_anchor(view.scroll_anchor);
    let visible_ordinals = layout.visible_ordinals(anchor);
    let overscan_ordinals = layout.overscan_ordinals(anchor);
    let mut actions = Vec::new();
    let painter = ui
        .painter()
        .with_clip_rect(surface.intersect(ui.clip_rect()));
    painter.rect_filled(surface, 0.0, ui.visuals().panel_fill);
    painter.rect_filled(geometry.header, 0.0, ui.visuals().faint_bg_color);
    let id = ui.id().with((
        "page-overview",
        view.stamp.context_serial,
        view.stamp.incarnation,
    ));
    if button(
        ui,
        geometry.back_button,
        id.with("back"),
        if surface.width() < 360.0 {
            "戻る"
        } else {
            "ページ表示に戻る"
        },
        true,
        input.input_enabled,
    ) {
        actions.push(PageOverviewUiAction::Cancel);
    }
    truncated_text(
        ui,
        &painter,
        geometry.title,
        input.title,
        ui.visuals().text_color(),
        14.0,
    );
    let current = input
        .current_items_indices
        .first()
        .and_then(|&idx| view.projection.ordinal_for_item(idx));
    let position = current.map_or_else(
        || format!("全 {} ページ", view.projection.len()),
        |ordinal| format!("現在 {} / {}", ordinal + 1, view.projection.len()),
    );
    truncated_text(
        ui,
        &painter,
        geometry.position,
        &position,
        ui.visuals().weak_text_color(),
        12.0,
    );
    truncated_text(
        ui,
        &painter,
        geometry.columns_label,
        &format!("{} 列", view.preferred_columns),
        ui.visuals().text_color(),
        12.0,
    );
    if button(
        ui,
        geometry.columns_less,
        id.with("less"),
        "-",
        view.preferred_columns > 1,
        input.input_enabled,
    ) {
        actions.push(PageOverviewUiAction::ColumnsDelta(-1));
    }
    if button(
        ui,
        geometry.columns_more,
        id.with("more"),
        "+",
        view.preferred_columns < 10,
        input.input_enabled,
    ) {
        actions.push(PageOverviewUiAction::ColumnsDelta(1));
    }

    let grid_painter = painter.with_clip_rect(layout.viewport().intersect(ui.clip_rect()));
    for ordinal in overscan_ordinals.clone() {
        let Some(entry) = view.projection.entry(ordinal) else {
            continue;
        };
        let Some(cell) = layout.cell_rect(ordinal, anchor) else {
            continue;
        };
        if !cell.intersects(layout.viewport()) {
            continue;
        }
        let clipped = cell.intersect(layout.viewport()).intersect(ui.clip_rect());
        if !clipped.is_positive() {
            continue;
        }
        let response = ui.interact(
            clipped,
            id.with(("cell", ordinal)),
            if input.input_enabled {
                Sense::CLICK
            } else {
                Sense::hover()
            },
        );
        let current = input.current_items_indices.contains(&entry.items_idx);
        let cursor = ordinal == view.cursor;
        grid_painter.rect_filled(
            cell,
            4.0,
            if current {
                ui.visuals().selection.bg_fill.gamma_multiply(0.25)
            } else {
                ui.visuals().faint_bg_color
            },
        );
        let image_rect = Rect::from_min_max(
            cell.min + egui::vec2(5.0, 25.0),
            cell.max - egui::vec2(5.0, 40.0),
        );
        let thumbnail = input
            .thumbnails
            .and_then(|map| map.get(&entry.items_idx))
            .copied();
        if !thumbnail.is_some_and(|thumbnail| draw_thumbnail(&grid_painter, image_rect, thumbnail))
        {
            let failed = matches!(thumbnail, Some(PageOverviewThumbnail::Failed));
            truncated_text(
                ui,
                &grid_painter,
                image_rect,
                if failed {
                    "読み込み失敗"
                } else {
                    "未生成"
                },
                if failed {
                    ui.visuals().error_fg_color
                } else {
                    ui.visuals().weak_text_color()
                },
                12.0,
            );
        }
        if let Some(companion) = entry.page.white_companion {
            let annotation = Rect::from_min_size(
                egui::pos2(image_rect.left(), image_rect.bottom() - 20.0),
                egui::vec2(image_rect.width().min(70.0), 18.0),
            );
            grid_painter.rect_filled(annotation, 2.0, Color32::WHITE);
            grid_painter.rect_stroke(
                annotation,
                2.0,
                Stroke::new(1.0, ui.visuals().weak_text_color()),
                egui::StrokeKind::Inside,
            );
            truncated_text(
                ui,
                &grid_painter,
                annotation.shrink(2.0),
                match companion {
                    WhiteCompanion::Composition => "白紙あり",
                    WhiteCompanion::DisplayPadding => "余白あり",
                },
                Color32::BLACK,
                10.0,
            );
        }
        if current {
            truncated_text(
                ui,
                &grid_painter,
                Rect::from_min_size(
                    cell.min + egui::vec2(5.0, 2.0),
                    egui::vec2(cell.width() - 10.0, 20.0),
                ),
                "表示中",
                ui.visuals().text_color(),
                11.0,
            );
        }
        if cursor {
            grid_painter.rect_stroke(
                cell.shrink(1.0),
                4.0,
                Stroke::new(2.0, ui.visuals().selection.stroke.color),
                egui::StrokeKind::Inside,
            );
        } else if response.hovered() {
            grid_painter.rect_stroke(
                cell.shrink(1.0),
                4.0,
                ui.visuals().widgets.hovered.bg_stroke,
                egui::StrokeKind::Inside,
            );
        }
        truncated_text(
            ui,
            &grid_painter,
            Rect::from_min_max(
                egui::pos2(cell.left() + 5.0, cell.bottom() - 36.0),
                egui::pos2(cell.right() - 5.0, cell.bottom() - 19.0),
            ),
            &format!("{}", ordinal + 1),
            ui.visuals().text_color(),
            12.0,
        );
        truncated_text(
            ui,
            &grid_painter,
            Rect::from_min_max(
                egui::pos2(cell.left() + 5.0, cell.bottom() - 19.0),
                egui::pos2(cell.right() - 5.0, cell.bottom() - 2.0),
            ),
            &entry.page.label,
            ui.visuals().weak_text_color(),
            11.0,
        );
        if input.input_enabled && primary_click_in_drawn_rect(ui, clipped) {
            if let Some(selection) = view.capture_select(ordinal) {
                actions.push(PageOverviewUiAction::Select(selection.target));
            }
        }
    }

    let bar = geometry.scrollbar;
    let arrow_height = 18.0_f32.min((bar.height() / 2.0).max(0.0));
    let up = Rect::from_min_size(bar.min, egui::vec2(bar.width(), arrow_height));
    let down = Rect::from_min_max(egui::pos2(bar.left(), bar.bottom() - arrow_height), bar.max);
    for (rect, name, text, delta) in [(up, "up", "↑", -1), (down, "down", "↓", 1)] {
        let step = clamped_scroll_delta(view, &layout, delta);
        if button(
            ui,
            rect,
            id.with(name),
            text,
            step != 0,
            input.input_enabled,
        ) {
            actions.push(PageOverviewUiAction::ScrollRows(step));
        }
    }
    let track = Rect::from_min_max(
        egui::pos2(bar.left() + 3.0, up.bottom()),
        egui::pos2(bar.right() - 3.0, down.top()),
    );
    if track.is_positive() && layout.total_rows() > 0 {
        painter.rect_filled(track, 3.0, ui.visuals().faint_bg_color);
        let fraction = (layout.page_rows() as f32 / layout.total_rows() as f32).min(1.0);
        let thumb_height = (track.height() * fraction).max(18.0).min(track.height());
        let travel = track.height() - thumb_height;
        let progress = if layout.max_scroll_row() == 0 {
            0.0
        } else {
            anchor.row as f32 / layout.max_scroll_row() as f32
        };
        let thumb = Rect::from_min_size(
            track.min + egui::vec2(0.0, travel * progress),
            egui::vec2(track.width(), thumb_height),
        );
        painter.rect_filled(thumb, 3.0, ui.visuals().widgets.inactive.fg_stroke.color);
        let _response = ui.interact(
            track,
            id.with("scroll-track"),
            if input.input_enabled {
                Sense::CLICK
            } else {
                Sense::hover()
            },
        );
        if input.input_enabled && primary_click_in_drawn_rect(ui, track) {
            if let Some(point) = ui.input(|input| input.pointer.interact_pos()) {
                let row = if travel <= 0.0 {
                    0
                } else {
                    (((point.y - track.top() - thumb_height / 2.0) / travel).clamp(0.0, 1.0)
                        * layout.max_scroll_row() as f32)
                        .round() as usize
                };
                let delta = if row >= anchor.row {
                    (row - anchor.row).min(isize::MAX as usize) as isize
                } else {
                    -((anchor.row - row).min(isize::MAX as usize) as isize)
                };
                if delta != 0 {
                    actions.push(PageOverviewUiAction::ScrollRows(delta));
                }
            }
        }
    }
    if input.input_enabled && ui.rect_contains_pointer(surface) {
        let wheel = ui.input_mut(|state| {
            let actions = wheel_actions(&state.events, view, &layout);
            state
                .events
                .retain(|event| !matches!(event, egui::Event::MouseWheel { .. }));
            state.raw_scroll_delta = Vec2::ZERO;
            state.smooth_scroll_delta = Vec2::ZERO;
            actions
        });
        actions.extend(wheel);
    }
    ui.allocate_rect(surface, Sense::hover());
    PageOverviewRenderOutput {
        commands: actions
            .into_iter()
            .map(|action| PageOverviewUiCommand {
                stamp: view.stamp,
                action,
            })
            .collect(),
        layout,
        visible_ordinals,
        overscan_ordinals,
        surface_rect: surface,
        back_button_rect: geometry.back_button,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::book_resume_db::PageIdentity;
    use crate::page_overview::{
        PageOverviewEntry, PageOverviewMetadata, PageOverviewPage, PageOverviewProjection,
        PageOverviewState,
    };
    use std::sync::Arc;

    fn view(count: usize) -> PageOverviewView {
        let metadata = PageOverviewMetadata::new((0..count).map(|idx| PageOverviewEntry {
            items_idx: idx,
            page: Arc::new(PageOverviewPage {
                identity: PageIdentity::Image {
                    path: format!("g:/test/{idx}.png"),
                },
                label: Arc::from(format!("page {idx}")),
                white_companion: (idx == 1).then_some(WhiteCompanion::Composition),
            }),
        }))
        .unwrap();
        let projection = Arc::new(
            PageOverviewProjection::new(Arc::new(metadata), Arc::new((0..count).collect()))
                .unwrap(),
        );
        PageOverviewState::open(
            projection,
            PageOverviewStamp {
                context_serial: 1,
                items_generation: 2,
                reader_order_revision: 3,
                incarnation: 4,
            },
            0,
            4,
            Rect::from_min_size(Pos2::ZERO, egui::vec2(720.0, 500.0)),
            PageOverviewDirection::LeftToRight,
        )
        .unwrap()
        .session()
        .unwrap()
        .view()
        .clone()
    }

    fn wheel(delta: f32, ctrl: bool) -> egui::Event {
        egui::Event::MouseWheel {
            unit: egui::MouseWheelUnit::Line,
            delta: egui::vec2(0.0, delta),
            modifiers: egui::Modifiers {
                ctrl,
                ..Default::default()
            },
        }
    }

    fn frame(
        ctx: &egui::Context,
        view: &PageOverviewView,
        size: Vec2,
        enabled: bool,
        events: Vec<egui::Event>,
    ) -> PageOverviewRenderOutput {
        let mut output = None;
        let _ = ctx.run(egui::RawInput { screen_rect: Some(Rect::from_min_size(Pos2::ZERO, size)), events, ..Default::default() }, |ctx| {
            egui::CentralPanel::default().frame(egui::Frame::NONE).show(ctx, |ui| {
                let failed = HashMap::from([(3, PageOverviewThumbnail::Failed)]);
                output = Some(draw_page_overview(ui, PageOverviewRenderInput { view, title: "A very long book title that must fit without moving the return button", current_items_indices: &[0, 1], thumbnails: Some(&failed), direction: PageOverviewDirection::RightToLeft, input_enabled: enabled }));
            });
        });
        output.unwrap()
    }

    fn click(
        ctx: &egui::Context,
        view: &PageOverviewView,
        size: Vec2,
        enabled: bool,
        point: Pos2,
    ) -> PageOverviewRenderOutput {
        frame(
            ctx,
            view,
            size,
            enabled,
            vec![
                egui::Event::PointerMoved(point),
                egui::Event::PointerButton {
                    pos: point,
                    button: egui::PointerButton::Primary,
                    pressed: true,
                    modifiers: Default::default(),
                },
            ],
        );
        frame(
            ctx,
            view,
            size,
            enabled,
            vec![egui::Event::PointerButton {
                pos: point,
                button: egui::PointerButton::Primary,
                pressed: false,
                modifiers: Default::default(),
            }],
        )
    }

    #[test]
    fn overview_wheel_is_row_snapped_and_clamped_at_both_boundaries() {
        let mut view = view(50_000);
        let grid = view.layout(
            Rect::from_min_size(Pos2::ZERO, egui::vec2(408.0, 376.0)),
            PageOverviewDirection::LeftToRight,
        );
        assert!(wheel_actions(&[wheel(3.0, false)], &view, &grid).is_empty());
        assert_eq!(
            wheel_actions(&[wheel(-3.0, false)], &view, &grid),
            vec![PageOverviewUiAction::ScrollRows(1)]
        );
        view.scroll_anchor.row = grid.max_scroll_row();
        assert!(wheel_actions(&[wheel(-3.0, false)], &view, &grid).is_empty());
        assert_eq!(
            wheel_actions(&[wheel(3.0, false)], &view, &grid),
            vec![PageOverviewUiAction::ScrollRows(-1)]
        );
        view.preferred_columns = 1;
        assert!(wheel_actions(&[wheel(3.0, true)], &view, &grid).is_empty());
        assert_eq!(
            wheel_actions(&[wheel(-3.0, true)], &view, &grid),
            vec![PageOverviewUiAction::ColumnsDelta(1)]
        );
    }

    #[test]
    fn overview_placeholder_failed_and_white_annotation_click_same_real_page() {
        let ctx = egui::Context::default();
        let view = view(20);
        let size = egui::vec2(720.0, 500.0);
        let first = frame(&ctx, &view, size, true, vec![]);
        for ordinal in [0, 1, 3] {
            let cell = first.layout.cell_rect(ordinal, view.scroll_anchor).unwrap();
            let result = click(&ctx, &view, size, true, cell.center());
            assert_eq!(
                result.commands,
                vec![PageOverviewUiCommand {
                    stamp: view.stamp,
                    action: PageOverviewUiAction::Select(
                        view.capture_select(ordinal).unwrap().target
                    )
                }]
            );
        }
        assert_eq!(view.cursor, 0);
        assert_eq!(view.scroll_anchor.row, 0);
    }

    #[test]
    fn overview_small_surface_keeps_return_reachable_and_clickable() {
        for size in [
            egui::vec2(180.0, 180.0),
            egui::vec2(120.0, 80.0),
            egui::vec2(320.0, 420.0),
        ] {
            let ctx = egui::Context::default();
            let view = view(20);
            let first = frame(&ctx, &view, size, true, vec![]);
            assert!(first.surface_rect.contains_rect(first.back_button_rect));
            assert!(first.back_button_rect.is_positive());
            let result = click(&ctx, &view, size, true, first.back_button_rect.center());
            assert_eq!(
                result.commands,
                vec![PageOverviewUiCommand {
                    stamp: view.stamp,
                    action: PageOverviewUiAction::Cancel
                }]
            );
        }
    }

    #[test]
    fn overview_input_disabled_draws_without_commands_or_wheel_consumption() {
        let ctx = egui::Context::default();
        let view = view(20);
        let size = egui::vec2(720.0, 500.0);
        let first = frame(&ctx, &view, size, false, vec![]);
        assert!(
            click(&ctx, &view, size, false, first.back_button_rect.center())
                .commands
                .is_empty()
        );
        let result = frame(
            &ctx,
            &view,
            size,
            false,
            vec![
                egui::Event::PointerMoved(egui::pos2(50.0, 80.0)),
                wheel(-3.0, false),
            ],
        );
        assert!(result.commands.is_empty());
        assert!(ctx.input(|input| {
            input
                .events
                .iter()
                .any(|event| matches!(event, egui::Event::MouseWheel { .. }))
        }));
    }

    #[test]
    fn overview_consumes_permitted_wheel_even_at_scroll_limit() {
        let ctx = egui::Context::default();
        let view = view(20);
        let size = egui::vec2(720.0, 500.0);
        frame(&ctx, &view, size, true, vec![]);
        let result = frame(
            &ctx,
            &view,
            size,
            true,
            vec![
                egui::Event::PointerMoved(egui::pos2(50.0, 80.0)),
                wheel(3.0, false),
            ],
        );
        assert!(result.commands.is_empty());
        assert!(ctx.input(|input| {
            input
                .events
                .iter()
                .all(|event| !matches!(event, egui::Event::MouseWheel { .. }))
        }));
        assert_eq!(ctx.input(|input| input.raw_scroll_delta), Vec2::ZERO);
        assert_eq!(ctx.input(|input| input.smooth_scroll_delta), Vec2::ZERO);
        let result = frame(&ctx, &view, size, true, vec![wheel(-3.0, true)]);
        assert_eq!(
            result.commands,
            vec![PageOverviewUiCommand {
                stamp: view.stamp,
                action: PageOverviewUiAction::ColumnsDelta(1)
            }]
        );
    }

    #[test]
    fn overview_draw_range_is_bounded_and_background_click_does_not_cancel() {
        let ctx = egui::Context::default();
        let mut view = view(50_000);
        view.scroll_anchor.row = 10_000;
        let size = egui::vec2(408.0, 436.0);
        let first = frame(&ctx, &view, size, true, vec![]);
        assert!(first.overscan_ordinals.len() <= 5 * first.layout.columns());
        assert!(first.overscan_ordinals.start > 0);
        let gap = first
            .layout
            .cell_rect(first.visible_ordinals.start, view.scroll_anchor)
            .unwrap();
        let point = egui::pos2(gap.left() - 4.0, gap.center().y);
        assert!(click(&ctx, &view, size, true, point).commands.is_empty());
    }

    #[test]
    fn overview_focused_widgets_never_dispatch_raw_enter_space_or_repeat() {
        let ctx = egui::Context::default();
        let view = view(20);
        let size = egui::vec2(720.0, 500.0);
        let mut base_id = None;
        let _ = ctx.run(
            egui::RawInput {
                screen_rect: Some(Rect::from_min_size(Pos2::ZERO, size)),
                ..Default::default()
            },
            |ctx| {
                egui::CentralPanel::default()
                    .frame(egui::Frame::NONE)
                    .show(ctx, |ui| {
                        base_id = Some(ui.id().with((
                            "page-overview",
                            view.stamp.context_serial,
                            view.stamp.incarnation,
                        )));
                        draw_page_overview(
                            ui,
                            PageOverviewRenderInput {
                                view: &view,
                                title: "book",
                                current_items_indices: &[0],
                                thumbnails: None,
                                direction: PageOverviewDirection::LeftToRight,
                                input_enabled: true,
                            },
                        );
                    });
            },
        );
        let base_id = base_id.unwrap();
        for widget in [
            base_id.with("back"),
            base_id.with(("cell", 0_usize)),
            base_id.with("less"),
            base_id.with("more"),
            base_id.with("scroll-track"),
        ] {
            for key in [egui::Key::Enter, egui::Key::Space] {
                for repeat in [false, true] {
                    ctx.memory_mut(|memory| memory.request_focus(widget));
                    let output = frame(
                        &ctx,
                        &view,
                        size,
                        true,
                        vec![egui::Event::Key {
                            key,
                            physical_key: Some(key),
                            pressed: true,
                            repeat,
                            modifiers: egui::Modifiers::NONE,
                        }],
                    );
                    assert!(
                        output.commands.is_empty(),
                        "raw {key:?} repeat={repeat} generated a UI command"
                    );
                    frame(
                        &ctx,
                        &view,
                        size,
                        true,
                        vec![egui::Event::Key {
                            key,
                            physical_key: Some(key),
                            pressed: false,
                            repeat: false,
                            modifiers: egui::Modifiers::NONE,
                        }],
                    );
                }
            }
        }
    }

    #[test]
    fn overview_resize_exposes_new_placeholder_and_selects_on_first_click_pass() {
        let ctx = egui::Context::default();
        let view = view(50_000);
        let initial = frame(&ctx, &view, egui::vec2(420.0, 260.0), true, vec![]);
        let enlarged_size = egui::vec2(820.0, 1000.0);
        let geometry = page_overview_geometry(
            Rect::from_min_size(Pos2::ZERO, enlarged_size),
            &view,
            PageOverviewDirection::RightToLeft,
        );
        let ordinal = initial.overscan_ordinals.end;
        let point = geometry
            .layout
            .cell_rect(ordinal, view.scroll_anchor)
            .unwrap()
            .center();
        assert!(geometry.layout.viewport().contains(point));
        let output = frame(
            &ctx,
            &view,
            enlarged_size,
            true,
            vec![
                egui::Event::PointerMoved(point),
                egui::Event::PointerButton {
                    pos: point,
                    button: egui::PointerButton::Primary,
                    pressed: true,
                    modifiers: egui::Modifiers::NONE,
                },
                egui::Event::PointerButton {
                    pos: point,
                    button: egui::PointerButton::Primary,
                    pressed: false,
                    modifiers: egui::Modifiers::NONE,
                },
            ],
        );
        assert_eq!(
            output.commands,
            vec![PageOverviewUiCommand {
                stamp: view.stamp,
                action: PageOverviewUiAction::Select(view.capture_select(ordinal).unwrap().target)
            }]
        );
    }
}
