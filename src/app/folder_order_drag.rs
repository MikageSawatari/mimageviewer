//! Main-grid manual reorder gesture. The snapshot is immutable; only its
//! sub-row scroll remainder changes before the existing refresh owns adoption.
use super::*;

#[derive(Clone)]
struct FolderOrderDrag {
    start: Arc<DragStart>,
    scroll_remainder: f32,
    scroll_frame: Option<u64>,
}

/// A terminal belongs to the gesture held before this frame's cell Responses.
/// A new press in the same input batch cannot replace its source group.
pub(crate) struct FolderOrderDragEnd {
    drag: FolderOrderDrag,
    release: Option<egui::Pos2>,
}

struct DragStart {
    context: ViewerContextId,
    viewport: egui::ViewportId,
    scroll_session: grid_scroll::GridScrollSessionId,
    folder: PathBuf,
    folder_key: String,
    generation: u64,
    order_revision: u64,
    geometry: GridScrollGeometry,
    grabbed: String,
    visible: Vec<usize>,
    keys: Vec<String>,
    moving: Vec<usize>,
    facet: crate::settings::FacetFilter,
    scan_options: ExternalRescanOptions,
    rating: [bool; 6],
    search: Option<Arc<LocalSearchSubmission>>,
    color: (bool, [u8; 3], u32),
}

fn drag_id() -> egui::Id {
    egui::Id::unique("folder_manual_order_drag")
}

/// `remove_temp` needs `Default`; the gesture has no meaningful empty value.
fn take_drag(data: &mut egui::util::IdTypeMap) -> Option<FolderOrderDrag> {
    let drag = data.get_temp::<FolderOrderDrag>(drag_id());
    data.remove::<FolderOrderDrag>(drag_id());
    drag
}

/// Insertion refers to the original visible sequence, before removing the group.
fn moved_order(keys: &[String], moving: &[usize], insertion: usize) -> Vec<String> {
    let insertion = insertion.min(keys.len());
    let destination = insertion.saturating_sub(moving.partition_point(|index| *index < insertion));
    let mut group = Vec::with_capacity(moving.len());
    let mut remaining = Vec::with_capacity(keys.len().saturating_sub(moving.len()));
    let mut selected = moving.iter().peekable();
    for (index, key) in keys.iter().enumerate() {
        if selected.peek().is_some_and(|position| **position == index) {
            group.push(key.clone());
            selected.next();
        } else {
            remaining.push(key.clone());
        }
    }
    let destination = destination.min(remaining.len());
    remaining.splice(destination..destination, group);
    remaining
}

fn drop_target(
    clip: egui::Rect,
    origin: egui::Pos2,
    geometry: GridScrollGeometry,
    cell_w: f32,
    len: usize,
    pointer: egui::Pos2,
) -> Option<(usize, f32, egui::Rect)> {
    if !clip.contains(pointer) || cell_w <= 0.0 {
        return None;
    }
    let local = pointer - origin;
    if local.x < 0.0 || local.y < 0.0 {
        return None;
    }
    let col = (local.x / cell_w).floor() as usize;
    let row = (local.y / geometry.row_h).floor() as usize;
    if col >= geometry.cols {
        return None;
    }
    let index = row.checked_mul(geometry.cols)?.checked_add(col)?;
    if index >= len {
        return None;
    }
    let rect = egui::Rect::from_min_size(
        origin + egui::vec2(col as f32 * cell_w, row as f32 * geometry.row_h),
        egui::vec2(cell_w, geometry.row_h),
    );
    let after = pointer.x >= rect.center().x;
    Some((
        index + usize::from(after),
        if after { rect.right() } else { rect.left() },
        rect,
    ))
}

fn edge_delta(pointer_y: f32, clip: egui::Rect) -> f32 {
    let height = clip.height().max(0.0);
    if height <= 1.0 {
        return 0.0;
    }
    let edge = 64.0_f32.min(height * 0.45).max(1.0);
    if pointer_y < clip.top() + edge {
        -((clip.top() + edge - pointer_y) / edge).clamp(0.0, 1.0) * 34.0
    } else if pointer_y > clip.bottom() - edge {
        ((pointer_y - (clip.bottom() - edge)) / edge).clamp(0.0, 1.0) * 34.0
    } else {
        0.0
    }
}

fn scroll_rows(offset: f32, remainder: &mut f32, delta: f32, geometry: GridScrollGeometry) -> f32 {
    if delta == 0.0
        || (delta < 0.0 && offset <= 0.0)
        || (delta > 0.0 && offset >= geometry.max_offset)
    {
        *remainder = 0.0;
        return offset;
    }
    if remainder.signum() != delta.signum() {
        *remainder = 0.0;
    }
    let sum = *remainder + delta;
    let rows = (sum / geometry.row_h).trunc();
    *remainder = sum - rows * geometry.row_h;
    let next = (offset + rows * geometry.row_h).clamp(0.0, geometry.max_offset);
    if (delta < 0.0 && next == 0.0) || (delta > 0.0 && next == geometry.max_offset) {
        *remainder = 0.0;
    }
    next
}

impl App {
    pub(crate) fn begin_folder_order_drag(&mut self, ctx: &egui::Context, index: usize) -> bool {
        if !self.folder_manual_order_drag_enabled() || !self.grid_item_input_allowed() {
            return false;
        }
        let Some(grabbed) = self
            .items
            .get(index)
            .and_then(crate::folder_order::physical_item_key)
        else {
            return false;
        };
        if !self.visible_indices.contains(&index) {
            return false;
        }
        let Some(keys) = self
            .visible_indices
            .iter()
            .map(|index| {
                self.items
                    .get(*index)
                    .and_then(crate::folder_order::physical_item_key)
            })
            .collect::<Option<Vec<_>>>()
        else {
            return false;
        };
        let group = self.checked.contains(&index);
        let moving = self
            .visible_indices
            .iter()
            .enumerate()
            .filter_map(|(position, item)| {
                (if group {
                    self.checked.contains(item)
                } else {
                    *item == index
                })
                .then_some(position)
            })
            .collect();
        let start = DragStart {
            context: self.projected_viewer_context_id(),
            viewport: ctx.viewport_id(),
            scroll_session: self.grid_scroll.session(),
            folder: self.current_folder.clone().unwrap(),
            folder_key: self.folder_order.folder_key.clone().unwrap(),
            generation: self.items_generation,
            order_revision: self.folder_order.order_revision,
            geometry: self.grid_scroll.geometry(),
            grabbed,
            visible: self.visible_indices.clone(),
            keys,
            moving,
            facet: self.settings.facet_filter.clone(),
            scan_options: self.current_external_scan_options(),
            rating: self.effective_rating_filter(),
            search: self
                .search_filter
                .as_ref()
                .map(|filter| Arc::clone(&filter.submitted)),
            color: (
                self.color_filter.enabled,
                self.color_filter.query_rgb,
                self.color_filter.tolerance.to_bits(),
            ),
        };
        // egui's temporary owner avoids adding inline App/bundle payloads. Its
        // frame clones share the frozen identities rather than copying the list.
        ctx.data_mut(|data| {
            data.insert_temp(
                drag_id(),
                FolderOrderDrag {
                    start: Arc::new(start),
                    scroll_remainder: 0.0,
                    scroll_frame: None,
                },
            )
        });
        true
    }

    fn folder_order_drag_matches(&self, ctx: &egui::Context, start: &DragStart) -> bool {
        start.context == self.projected_viewer_context_id()
            && start.viewport == ctx.viewport_id()
            && start.scroll_session == self.grid_scroll.session()
            && self.current_folder.as_ref() == Some(&start.folder)
            && self.folder_order.folder_key.as_ref() == Some(&start.folder_key)
            && start.generation == self.items_generation
            && start.order_revision == self.folder_order.order_revision
            && self.folder_manual_order_drag_enabled()
            && self.grid_item_input_allowed()
            && !self.any_dialog_open()
            && start.geometry.cols == self.grid_scroll.geometry().cols
            && start.geometry.row_h == self.grid_scroll.geometry().row_h
            && start.facet == self.settings.facet_filter
            && start.scan_options.file_type_visibility == self.settings.file_type_visibility
            && start.scan_options.show_hidden_files == self.settings.show_hidden_files
            && start.scan_options.include_convertible_archives
                == !self.settings.archive_file_handling_ignores_convertible()
            && start.scan_options.include_epub == !self.settings.epub_file_handling_ignores_epub()
            && start.rating == self.effective_rating_filter()
            && start.search.as_deref()
                == self
                    .search_filter
                    .as_ref()
                    .map(|filter| filter.submitted.as_ref())
            && start.color
                == (
                    self.color_filter.enabled,
                    self.color_filter.query_rgb,
                    self.color_filter.tolerance.to_bits(),
                )
            && start.visible == self.visible_indices
            && start
                .moving
                .iter()
                .any(|position| start.keys.get(*position) == Some(&start.grabbed))
    }

    pub(crate) fn cancel_folder_order_drag(&self, ctx: &egui::Context) {
        ctx.data_mut(|data| {
            data.remove::<FolderOrderDrag>(drag_id());
        });
    }

    pub(crate) fn folder_order_drag_active(&self, ctx: &egui::Context) -> bool {
        let drag = ctx.data(|data| data.get_temp::<FolderOrderDrag>(drag_id()));
        match drag {
            Some(drag) if self.folder_order_drag_matches(ctx, &drag.start) => true,
            Some(_) => {
                self.cancel_folder_order_drag(ctx);
                false
            }
            None => false,
        }
    }

    pub(crate) fn take_folder_order_drag_end(
        &self,
        ctx: &egui::Context,
    ) -> Option<FolderOrderDragEnd> {
        if !self.folder_order_drag_active(ctx) {
            return None;
        }
        let release = ctx.input(|input| {
            input
                .pointer
                .button_releases()
                .find(|(button, _)| *button == egui::PointerButton::Primary)
        });
        let (_, release) = release?;
        let drag = ctx.data_mut(take_drag)?;
        Some(FolderOrderDragEnd { drag, release })
    }

    fn commit_folder_order_drag_drop(
        &mut self,
        ctx: &egui::Context,
        drag: &FolderOrderDrag,
        target: Option<(usize, f32, egui::Rect)>,
    ) {
        if self.folder_order_drag_matches(ctx, &drag.start)
            && let Some((insertion, _, _)) = target
        {
            let reordered = moved_order(&drag.start.keys, &drag.start.moving, insertion);
            if reordered != drag.start.keys {
                self.commit_folder_manual_reorder(&reordered);
            }
        }
    }

    /// Called after the input dispatcher and ScrollArea's native readback. New
    /// explicit positions travel through the existing external scroll owner next layout.
    pub(crate) fn finish_folder_order_drag_frame(
        &mut self,
        ui: &egui::Ui,
        clip: egui::Rect,
        content_origin: egui::Pos2,
        geometry: GridScrollGeometry,
        cell_w: f32,
        ending: Option<FolderOrderDragEnd>,
    ) {
        let ctx = ui.ctx();
        if let Some(ending) = ending
            && ctx.input(|input| input.focused)
        {
            let target = ending.release.and_then(|pos| {
                drop_target(
                    clip,
                    content_origin,
                    geometry,
                    cell_w,
                    ending.drag.start.keys.len(),
                    pos,
                )
            });
            self.commit_folder_order_drag_drop(ctx, &ending.drag, target);
        }
        let Some(mut drag) = ctx.data_mut(take_drag) else {
            return;
        };
        if !self.folder_order_drag_matches(ctx, &drag.start) {
            return;
        }
        let (down, release, pointer, focused) = ctx.input(|input| {
            let down = input.pointer.primary_down();
            // An old release followed by a fresh press in one batch cannot end
            // the fresh gesture. Release coordinates own the terminal target.
            let release = (!down)
                .then(|| {
                    input
                        .pointer
                        .button_releases()
                        .filter(|(button, _)| *button == egui::PointerButton::Primary)
                        .last()
                        .and_then(|(_, pos)| pos)
                })
                .flatten();
            (down, release, input.pointer.interact_pos(), input.focused)
        });
        if !focused {
            return;
        }
        let pointer = release.or(pointer);
        let target = pointer.and_then(|pos| {
            drop_target(
                clip,
                content_origin,
                geometry,
                cell_w,
                drag.start.keys.len(),
                pos,
            )
        });
        if !down {
            if release.is_some() {
                self.commit_folder_order_drag_drop(ctx, &drag, target);
            }
            return;
        }
        if let Some((_, x, rect)) = target {
            let y0 = rect.top() + 5.0;
            let y1 = rect.bottom() - 5.0;
            if y1 > y0 {
                let painter = ui.painter().with_clip_rect(clip);
                let stroke = egui::Stroke::new(3.0, ui.visuals().selection.stroke.color);
                painter.line_segment([egui::pos2(x, y0), egui::pos2(x, y1)], stroke);
                for y in [y0, y1] {
                    painter.line_segment([egui::pos2(x - 6.0, y), egui::pos2(x + 6.0, y)], stroke);
                }
            }
        }
        let delta = pointer
            .filter(|pos| clip.contains(*pos))
            .map_or(0.0, |pos| edge_delta(pos.y, clip));
        let frame = ctx.cumulative_frame_nr();
        if drag.scroll_frame != Some(frame) {
            drag.scroll_frame = Some(frame);
            let before = self.grid_scroll.offset();
            let next = scroll_rows(before, &mut drag.scroll_remainder, delta, geometry);
            if before != next {
                self.grid_scroll.set_position(next);
            }
        }
        let next = self.grid_scroll.offset();
        if delta < 0.0 && next > 0.0 || delta > 0.0 && next < geometry.max_offset {
            ctx.request_repaint_after(std::time::Duration::from_millis(16));
        }
        ctx.data_mut(|data| data.insert_temp(drag_id(), drag));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn manual_app() -> AppTestEnvForTest {
        let mut app = setup_app_for_test();
        app.settings.sidecar_backup_enabled = false;
        app.settings.tag_sidecar_backup_enabled = false;
        app.settings.auto_fullscreen_image_folders = false;
        let folder = app.tmp.path().join("manual-drag-owner");
        std::fs::create_dir(&folder).unwrap();
        std::fs::write(folder.join("a.jpg"), b"fixture").unwrap();
        std::fs::write(folder.join("b.jpg"), b"fixture").unwrap();
        app.load_folder(folder);
        app.finish_page_identity_preparation_for_test();
        app.folder_order.enter_manual();
        app.grid_scroll
            .record_geometry(GridScrollGeometry::new(2, 100.0, 300.0, 300.0));
        app
    }

    #[test]
    fn folder_manual_drag_layout_change_cancels_without_changing_order() {
        let mut app = manual_app();
        let ctx = egui::Context::default();
        assert!(app.begin_folder_order_drag(&ctx, 0));
        let before = app.folder_order.clone();
        app.grid_scroll
            .record_geometry(GridScrollGeometry::new(3, 100.0, 300.0, 300.0));
        assert!(!app.folder_order_drag_active(&ctx));
        assert_eq!(app.folder_order, before);
    }

    #[cfg(windows)]
    #[test]
    fn folder_manual_drag_context_switch_cancels_without_changing_sibling_record() {
        let mut app = manual_app();
        let ctx = egui::Context::default();
        assert!(app.begin_folder_order_drag(&ctx, 0));
        let before = app.folder_order.clone();
        let sibling_order = before.clone();
        let folder = app.current_folder.clone();
        let sibling = app.build_window_context_for_test(91914, |owner| {
            owner.folder_order = sibling_order;
            owner.current_folder = folder;
        });
        app.with_viewer_context(sibling, |owner| {
            assert!(!owner.folder_order_drag_active(&ctx));
            assert_eq!(owner.folder_order, before);
        })
        .unwrap();
        assert!(!app.folder_order_drag_active(&ctx));
        assert_eq!(app.folder_order, before);
    }

    #[test]
    fn folder_manual_drag_group_crosses_rows_in_visible_relative_order() {
        let keys = ["a", "b", "c", "d", "e", "f"].map(str::to_owned);
        assert_eq!(
            moved_order(&keys, &[0, 3, 4], 6),
            ["b", "c", "f", "a", "d", "e"]
        );
        assert_eq!(
            moved_order(&keys, &[0, 3, 4], 2),
            ["b", "a", "d", "e", "c", "f"]
        );
        assert_eq!(moved_order(&keys, &[1, 2], 3), keys);
    }

    #[test]
    fn folder_manual_drag_target_uses_halves_and_rejects_blank_or_outside() {
        let clip = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(300.0, 300.0));
        let geometry = GridScrollGeometry::new(3, 100.0, 300.0, 300.0);
        let target = |x, y| {
            drop_target(clip, egui::Pos2::ZERO, geometry, 100.0, 7, egui::pos2(x, y))
                .map(|(index, x, _)| (index, x))
        };
        assert_eq!(target(110.0, 120.0), Some((4, 100.0)));
        assert_eq!(target(180.0, 120.0), Some((5, 200.0)));
        assert_eq!(target(180.0, 220.0), None);
        assert_eq!(target(301.0, 120.0), None);
    }

    #[test]
    fn folder_manual_drag_autoscroll_accumulates_only_whole_rows_in_external_owner() {
        let geometry = GridScrollGeometry::new(3, 100.0, 300.0, 1200.0);
        let mut scroll = grid_scroll::GridScrollState::default();
        scroll.record_geometry(geometry);
        let mut remainder = 0.0;
        for step in 1..=4 {
            let next = scroll_rows(scroll.offset(), &mut remainder, 34.0, geometry);
            if next != scroll.offset() {
                scroll.set_position(next);
            }
            assert_eq!(scroll.offset() % geometry.row_h, 0.0);
            assert_eq!(scroll.offset(), if step < 3 { 0.0 } else { 100.0 });
        }
        assert_eq!(remainder, 36.0);
        assert_eq!(
            scroll.resolve_layout(geometry, false, None, true).position,
            egui::scroll_area::ExternalScrollPosition::Set(100.0)
        );
        let next = scroll_rows(geometry.max_offset, &mut remainder, 34.0, geometry);
        assert_eq!(next, geometry.max_offset);
        assert_eq!(remainder, 0.0);
        let clip = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(300.0, 300.0));
        assert!(edge_delta(1.0, clip) < 0.0);
        assert!(edge_delta(299.0, clip) > 0.0);
        assert_eq!(edge_delta(150.0, clip), 0.0);
    }
}
