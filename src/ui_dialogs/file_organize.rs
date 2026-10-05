//! Fixed destinations: one request owner, Shell execution, and external-change refresh only.
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::{atomic::Ordering, mpsc};

use crate::app::{App, ExternalChangeCheck};
use crate::grid_item::GridItem;
use crate::settings::FileOrganizeDestination;
use crate::shell_file_ops::{ShellTransferOperation, ShellTransferRequest, ShellTransferResult};
use eframe::egui;

#[derive(Default)]
pub(crate) enum FileOrganizeRequest {
    #[default]
    Hidden,
    Selecting(Box<FileOrganizeSelection>),
    Running(Box<FileOrganizeRunning>),
}

pub(crate) struct FileOrganizeSelection {
    pub(crate) sources: Vec<PathBuf>,
    pub(crate) destinations: Vec<FileOrganizeDestination>,
    pub(crate) focus: Option<(usize, Option<ShellTransferOperation>)>,
}

pub(crate) struct FileOrganizeRunning {
    pub(crate) request: ShellTransferRequest,
    pub(crate) rx: mpsc::Receiver<ShellTransferResult>,
}

impl FileOrganizeRequest {
    // Consume the selected request once. In particular, a duplicate cannot retire Running.
    fn take_for_submission(
        &mut self,
        row: usize,
        operation: ShellTransferOperation,
        remote_owned: bool,
        closing: bool,
        owner: Option<isize>,
    ) -> Result<Option<ShellTransferRequest>, &'static str> {
        if !matches!(self, Self::Selecting(_)) {
            return Ok(None);
        }
        let Self::Selecting(selection) = std::mem::take(self) else {
            unreachable!()
        };
        let FileOrganizeSelection {
            sources,
            destinations,
            ..
        } = *selection;
        if remote_owned {
            return Err("リモート接続中のため整理操作を実行しませんでした");
        }
        if closing {
            return Err("終了が要求されたため整理操作を実行しませんでした");
        }
        if owner.is_none_or(|hwnd| hwnd == 0) {
            return Err("整理操作を開始できませんでした");
        }
        let Some(destination) = destinations.get(row) else {
            return Err("整理先を選び直してください");
        };
        Ok(Some(ShellTransferRequest {
            sources,
            destination: destination.path.clone(),
            operation,
        }))
    }
}

fn resolve_sources(
    items: &[GridItem],
    checked: &HashSet<usize>,
    cursor: Option<usize>,
    order: &[usize],
) -> Result<Vec<PathBuf>, String> {
    let indices = if checked.is_empty() {
        cursor.into_iter().collect::<Vec<_>>()
    } else {
        // Grid order is the visible order; refuse stale/missing indices rather than omit targets.
        let mut indices: Vec<_> = order
            .iter()
            .copied()
            .filter(|idx| checked.contains(idx))
            .collect();
        let ordered: HashSet<_> = indices.iter().copied().collect();
        let mut remaining: Vec<_> = checked
            .iter()
            .copied()
            .filter(|idx| !ordered.contains(idx))
            .collect();
        remaining.sort_unstable();
        indices.extend(remaining);
        indices
    };
    if indices.is_empty() {
        return Err("整理する項目を選択してください".to_owned());
    }
    let mut sources = Vec::with_capacity(indices.len());
    for index in indices {
        let Some(item) = items.get(index) else {
            return Err("選択項目が変更されました。選び直してください".to_owned());
        };
        let Some(path) = item.drag_source_path() else {
            let reason = item
                .file_operation_refusal()
                .map(|reason| reason.message("移動 / コピー"))
                .unwrap_or_else(|| "この項目は移動 / コピーできません".to_owned());
            return Err(format!(
                "{}: {reason}\n対象外の項目があるため実行しません。該当項目のチェックを外してください",
                item.name()
            ));
        };
        sources.push(path.to_owned());
    }
    Ok(sources)
}

fn refresh_matches(current: &Path, request: &ShellTransferRequest) -> bool {
    crate::folder_tree::path_eq(current, &request.destination)
        || request.sources.iter().any(|source| {
            source
                .parent()
                .is_some_and(|parent| crate::folder_tree::path_eq(current, parent))
        })
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FileOrganizeUiAction {
    Submit(usize, ShellTransferOperation),
    Preferences,
    Close,
}

/// The production Modal is also used by headless layout and snapshot tests.
pub fn show_file_organize_modal(
    ctx: &egui::Context,
    sources: &[PathBuf],
    destinations: &[FileOrganizeDestination],
    focus: &mut Option<(usize, Option<ShellTransferOperation>)>,
) -> egui::ModalResponse<Option<FileOrganizeUiAction>> {
    egui::Modal::new(egui::Id::new("file_organize_modal")).show(ctx, |ui| {
        ui.set_width((ctx.content_rect().width() - 48.0).clamp(240.0, 780.0));
        ui.heading("ファイル整理先");
        render_file_organize_contents(ui, sources, destinations, focus)
    })
}

fn table_text_cell(ui: &mut egui::Ui, size: egui::Vec2, text: &str, strong: bool) {
    ui.allocate_ui_with_layout(
        size,
        egui::Layout::left_to_right(egui::Align::Center),
        |ui| {
            ui.set_min_size(size);
            let styled_text = if strong {
                egui::RichText::new(text).strong()
            } else {
                egui::RichText::new(text)
            };
            ui.add(egui::Label::new(styled_text).truncate())
                .on_hover_text(text);
        },
    );
}

// Return a reveal request only for this pass's keyboard navigation. Pointer
// scrolling keeps egui's existing offset even when the selected row is hidden.
fn navigate_file_organize_focus(
    ctx: &egui::Context,
    destination_count: usize,
    focus: &mut Option<(usize, Option<ShellTransferOperation>)>,
) -> Option<usize> {
    if destination_count == 0 || crate::ime_focus::ime_input_active(ctx) {
        return None;
    }
    ctx.input_mut(|input| {
        let modifiers = egui::Modifiers::NONE;
        let down = input.consume_key(modifiers, egui::Key::ArrowDown);
        let up = input.consume_key(modifiers, egui::Key::ArrowUp);
        let left = input.consume_key(modifiers, egui::Key::ArrowLeft);
        let right = input.consume_key(modifiers, egui::Key::ArrowRight);
        if !(down || up || left || right) {
            return None;
        }
        let (row, operation) = focus.unwrap_or((0, None));
        let row = if down {
            (row + usize::from(focus.is_some())).min(destination_count - 1)
        } else if up {
            row.saturating_sub(1)
        } else {
            row
        };
        let operation = if left {
            Some(ShellTransferOperation::Move)
        } else if right {
            Some(ShellTransferOperation::Copy)
        } else {
            operation
        };
        *focus = Some((row, operation));
        Some(row)
    })
}

/// Same contents used by the application and headless UI snapshots.
pub fn render_file_organize_contents(
    ui: &mut egui::Ui,
    sources: &[PathBuf],
    destinations: &[FileOrganizeDestination],
    focus: &mut Option<(usize, Option<ShellTransferOperation>)>,
) -> Option<FileOrganizeUiAction> {
    let reveal_row = navigate_file_organize_focus(ui.ctx(), destinations.len(), focus);
    let content_top = ui.cursor().top();
    ui.label(format!("対象: {} 件", sources.len()));
    ui.collapsing("対象の一覧", |ui| {
        ui.spacing_mut().scroll = egui::style::ScrollStyle::solid();
        let text_height = ui.text_style_height(&egui::TextStyle::Body);
        let height = ((text_height + ui.spacing().item_spacing.y) * sources.len() as f32)
            .min(100.0)
            .min(ui.ctx().content_rect().height() * 0.25);
        ui.allocate_ui_with_layout(
            egui::vec2(ui.available_width(), height),
            egui::Layout::top_down(egui::Align::Min),
            |ui| {
                egui::ScrollArea::vertical()
                    .id_salt("organize_sources")
                    .max_height(height)
                    .min_scrolled_height(0.0)
                    .auto_shrink([false, false])
                    .show_rows(ui, text_height, sources.len(), |ui, rows| {
                        for row in rows {
                            let path = sources[row].display().to_string();
                            ui.add(egui::Label::new(&path).truncate())
                                .on_hover_text(path);
                        }
                    });
            },
        );
    });
    ui.separator();
    let mut action = None;
    if destinations.is_empty() {
        ui.label("ファイル整理先はまだ登録されていません。環境設定で登録してください。");
        if ui.button("環境設定で登録…").clicked() {
            action = Some(FileOrganizeUiAction::Preferences);
        }
    } else {
        ui.spacing_mut().scroll = egui::style::ScrollStyle::solid();
        let button_text_size = ["移動", "コピー"]
            .into_iter()
            .map(|label| {
                ui.painter()
                    .layout_no_wrap(
                        label.to_owned(),
                        egui::TextStyle::Button.resolve(ui.style()),
                        ui.visuals().text_color(),
                    )
                    .size()
            })
            .fold(egui::Vec2::ZERO, |size, text| size.max(text));
        let row_height = ui
            .spacing()
            .interact_size
            .y
            .max(
                ui.text_style_height(&egui::TextStyle::Body)
                    .max(button_text_size.y)
                    + 2.0 * ui.spacing().button_padding.y,
            )
            .max(28.0);
        let button_width = (button_text_size.x + 2.0 * ui.spacing().button_padding.x).max(72.0);
        let operation_gap = 24.0;
        let spacing = ui.spacing().item_spacing;
        let width = ui.available_width();
        // Reserve the solid bar's column even while it is hidden/animating, so
        // header and row columns agree and their widths never depend on last pass.
        let row_width = width - ui.spacing().scroll.allocated_width();
        let text_width =
            (row_width - 2.0 * button_width - operation_gap - 3.0 * spacing.x).max(0.0);
        let name_width = text_width * 0.25;
        let path_width = text_width - name_width;
        ui.allocate_ui_with_layout(
            egui::vec2(row_width, row_height),
            egui::Layout::left_to_right(egui::Align::Center),
            |ui| {
                table_text_cell(ui, egui::vec2(name_width, row_height), "名前", true);
                table_text_cell(ui, egui::vec2(path_width, row_height), "パス", true);
                ui.label("操作");
            },
        );
        // Area/Modal feeds last pass's content size into Ui::available_height.
        // Own the list viewport here: natural row height capped by the screen,
        // with measured header space and a footer reserve. Never feed Area's
        // previous height back into the next list height.
        let chrome_height = ui.cursor().top() - content_top
            + ui.text_style_height(&egui::TextStyle::Heading)
            + row_height
            + 4.0 * spacing.y
            + egui::Frame::popup(ui.style()).total_margin().sum().y
            + 32.0;
        let height = ((row_height + spacing.y) * destinations.len() as f32 - spacing.y)
            .min(360.0)
            .min((ui.ctx().content_rect().height() - chrome_height).max(row_height));
        ui.allocate_ui_with_layout(
            egui::vec2(width, height),
            egui::Layout::top_down(egui::Align::Min),
            |ui| {
                let mut scroll = egui::ScrollArea::vertical()
                    .id_salt("organize_destinations")
                    .max_height(height)
                    .min_scrolled_height(0.0)
                    .auto_shrink([false, false]);
                if let Some(row) = reveal_row {
                    let offset = egui::scroll_area::State::load(
                        ui.ctx(),
                        ui.make_persistent_id(egui::Id::new("organize_destinations")),
                    )
                    .unwrap_or_default()
                    .offset
                    .y;
                    let top = row as f32 * (row_height + spacing.y);
                    // Set the nearest visible offset before show_rows chooses
                    // which rows to draw, including an offscreen selection.
                    scroll = scroll.vertical_scroll_offset(
                        offset.clamp((top + row_height - height).max(0.0), top),
                    );
                }
                scroll.show_rows(ui, row_height, destinations.len(), |ui, rows| {
                    for row in rows {
                        let destination = &destinations[row];
                        let response = ui
                            .allocate_ui_with_layout(
                                egui::vec2(row_width, row_height),
                                egui::Layout::left_to_right(egui::Align::Center),
                                |ui| {
                                    ui.set_min_size(egui::vec2(row_width, row_height));
                                    table_text_cell(
                                        ui,
                                        egui::vec2(name_width, row_height),
                                        &destination.name,
                                        false,
                                    );
                                    table_text_cell(
                                        ui,
                                        egui::vec2(path_width, row_height),
                                        &destination.path.display().to_string(),
                                        false,
                                    );
                                    for (operation, label) in [
                                        (ShellTransferOperation::Move, "移動"),
                                        (ShellTransferOperation::Copy, "コピー"),
                                    ] {
                                        if operation == ShellTransferOperation::Copy {
                                            ui.add_space(operation_gap);
                                        }
                                        if ui
                                            .add_sized(
                                                egui::vec2(button_width, row_height),
                                                egui::Button::new(label).selected(
                                                    *focus == Some((row, Some(operation))),
                                                ),
                                            )
                                            .clicked()
                                        {
                                            *focus = Some((row, Some(operation)));
                                            action =
                                                Some(FileOrganizeUiAction::Submit(row, operation));
                                        }
                                    }
                                },
                            )
                            .response;
                        if focus.is_some_and(|(selected, _)| selected == row) {
                            ui.painter().rect_stroke(
                                response.rect,
                                4.0,
                                ui.visuals().selection.stroke,
                                egui::StrokeKind::Inside,
                            );
                        }
                    }
                });
            },
        );
    }
    ui.separator();
    if ui.button("閉じる").clicked() {
        action = Some(FileOrganizeUiAction::Close);
    }
    action
}

impl App {
    pub(crate) fn file_organize_dialog_visible(&self) -> bool {
        matches!(
            self.file_organize_request,
            FileOrganizeRequest::Selecting(_)
        )
    }

    pub(crate) fn request_file_organize_dialog(&mut self, clicked: Option<usize>) {
        if !matches!(self.file_organize_request, FileOrganizeRequest::Hidden) {
            self.show_feedback_toast("整理操作が完了するまでお待ちください".to_owned());
            return;
        }
        if self.remote_session_blocks_local_control() {
            self.show_feedback_toast("リモート接続中は整理操作を開始できません".to_owned());
            return;
        }
        match resolve_sources(
            &self.items,
            &self.checked,
            clicked.or(self.selected),
            self.current_grid_order(),
        ) {
            Ok(sources) => {
                self.file_organize_request =
                    FileOrganizeRequest::Selecting(Box::new(FileOrganizeSelection {
                        sources,
                        destinations: self.settings.file_organize_destinations.clone(),
                        focus: None,
                    }))
            }
            Err(message) => self.show_feedback_toast(message),
        }
    }

    pub(crate) fn submit_file_organize(
        &mut self,
        ctx: &egui::Context,
        row: usize,
        operation: ShellTransferOperation,
    ) {
        // The sole admission boundary shared by pointer and Enter. No worker is spawned earlier.
        let closing = self.shutdown_requested.load(Ordering::SeqCst)
            || self
                .tray_controller
                .as_ref()
                .is_some_and(|tray| tray.is_quit_requested())
            || ctx.input(|input| {
                input
                    .raw
                    .viewports
                    .get(&egui::ViewportId::ROOT)
                    .is_some_and(|viewport| viewport.close_requested())
            });
        match self.file_organize_request.take_for_submission(
            row,
            operation,
            self.remote_session_blocks_local_control(),
            closing,
            self.main_hwnd,
        ) {
            Ok(Some(request)) => {
                let rx = crate::shell_file_ops::transfer_items_async(
                    self.main_hwnd,
                    request.clone(),
                    ctx.clone(),
                );
                self.file_organize_request =
                    FileOrganizeRequest::Running(Box::new(FileOrganizeRunning { request, rx }));
            }
            Ok(None) => {}
            Err(message) => self.show_feedback_toast(message.to_owned()),
        }
    }

    pub(crate) fn poll_file_organize(&mut self, ctx: &egui::Context) {
        if self.file_organize_dialog_visible() && self.remote_session_blocks_local_control() {
            self.file_organize_request = FileOrganizeRequest::Hidden;
            self.show_feedback_toast(
                "リモート接続されたため整理先の選択画面を閉じました".to_owned(),
            );
            return;
        }
        let FileOrganizeRequest::Running(running) = &self.file_organize_request else {
            return;
        };
        let result = match running.rx.try_recv() {
            Ok(result) => result,
            Err(mpsc::TryRecvError::Empty) => return,
            Err(mpsc::TryRecvError::Disconnected) => {
                Err("整理操作の結果を受信できませんでした".to_owned())
            }
        };
        let FileOrganizeRequest::Running(running) = std::mem::take(&mut self.file_organize_request)
        else {
            unreachable!()
        };
        let request = running.request;
        match result {
            Ok(outcome) if outcome.aborted => self.show_feedback_toast(
                "操作が中断されました。処理済みの項目は元に戻りません".to_owned(),
            ),
            Ok(_) => {}
            Err(message) => {
                crate::logger::log(format!("file_organize: {message}"));
                self.show_feedback_toast(format!(
                    "操作を完了できませんでした。フォルダの内容を確認してください\n{message}"
                ));
            }
        }
        // No removal, metadata migration, retained-list cleanup or viewer invalidation here.
        if self.is_physical_folder_listing()
            && self
                .current_folder
                .as_deref()
                .is_some_and(|current| refresh_matches(current, &request))
        {
            self.check_external_folder_changes(ctx, ExternalChangeCheck::Notified);
        }
    }

    pub(crate) fn show_file_organize_dialog(&mut self, ctx: &egui::Context) {
        if !self.file_organize_dialog_visible() {
            return;
        }
        let escape = self.dialog_escape_pressed(ctx);
        let enter = self.dialog_enter_pressed(ctx);
        let FileOrganizeRequest::Selecting(selection) = &mut self.file_organize_request else {
            return;
        };
        let FileOrganizeSelection {
            sources,
            destinations,
            focus,
        } = selection.as_mut();
        let action = show_file_organize_modal(ctx, sources, destinations, focus).inner;
        let action = if escape {
            Some(FileOrganizeUiAction::Close)
        } else {
            action.or_else(|| {
                if enter {
                    focus.and_then(|(row, operation)| {
                        operation.map(|operation| FileOrganizeUiAction::Submit(row, operation))
                    })
                } else {
                    None
                }
            })
        };
        match action {
            Some(FileOrganizeUiAction::Submit(row, operation)) => {
                self.submit_file_organize(ctx, row, operation)
            }
            Some(FileOrganizeUiAction::Close) => {
                self.file_organize_request = FileOrganizeRequest::Hidden
            }
            Some(FileOrganizeUiAction::Preferences) => {
                self.file_organize_request = FileOrganizeRequest::Hidden;
                self.open_preferences_page(crate::ui_dialogs::preferences::PreferencesPage::Folder);
            }
            None => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    type NavigationHarness = egui_kittest::Harness<
        'static,
        (
            Vec<FileOrganizeDestination>,
            Option<(usize, Option<ShellTransferOperation>)>,
        ),
    >;

    fn navigation_harness(size: egui::Vec2) -> NavigationHarness {
        let mut fonts_ready = false;
        let mut harness = egui_kittest::Harness::builder()
            .with_size(size)
            .build_state(
                move |ctx, state: &mut (Vec<FileOrganizeDestination>, Option<_>)| {
                    if !fonts_ready {
                        crate::ui_fonts::configure_fonts(ctx);
                        fonts_ready = true;
                    }
                    let _ = show_file_organize_modal(
                        ctx,
                        &[r"C:\source\image.jpg".into()],
                        &state.0,
                        &mut state.1,
                    );
                },
                (
                    (0..60)
                        .map(|row| FileOrganizeDestination {
                            name: format!("destination {row}"),
                            path: PathBuf::from(format!(r"D:\destination\{row}")),
                        })
                        .collect(),
                    None,
                ),
            );
        harness.run();
        harness
    }

    fn assert_navigation_row_visible(harness: &NavigationHarness, row: usize) {
        use egui_kittest::kittest::Queryable;

        assert_eq!(
            harness.state().1,
            Some((row, Some(ShellTransferOperation::Copy)))
        );
        let label = harness.get_by_label(&format!("destination {row}")).rect();
        // The row's selection outline uses the real ScrollArea clip rect. A
        // rendered but partly clipped row must also fail this assertion.
        let (outline, clip) = harness
            .output()
            .shapes
            .iter()
            .find_map(|shape| match &shape.shape {
                egui::Shape::Rect(rect)
                    if rect.stroke.width > 0.0
                        && rect.rect.height() < 100.0
                        && rect.rect.width() > label.width() + 100.0
                        && rect.rect.contains_rect(label) =>
                {
                    Some((rect.rect, shape.clip_rect))
                }
                _ => None,
            })
            .expect("selected row must be drawn");
        assert!(
            clip.contains_rect(outline),
            "row={row}, row rect={outline:?}, clip={clip:?}"
        );
    }

    #[test]
    fn file_organize_keyboard_navigation_reveals_every_row_down_and_up() {
        use egui_kittest::kittest::Queryable;

        for size in [egui::vec2(1000.0, 720.0), egui::vec2(420.0, 320.0)] {
            let mut harness = navigation_harness(size);
            harness.key_press(egui::Key::ArrowRight);
            harness.step();
            assert_navigation_row_visible(&harness, 0);
            for (key, rows) in [
                (egui::Key::ArrowDown, (1..60).collect::<Vec<_>>()),
                (egui::Key::ArrowUp, (0..59).rev().collect()),
            ] {
                for row in rows {
                    let last_row = if key == egui::Key::ArrowUp && row == 58 {
                        Some(harness.get_by_label("destination 59").rect())
                    } else {
                        None
                    };
                    harness.key_down(key);
                    harness.step();
                    assert_navigation_row_visible(&harness, row);
                    if let Some(last_row) = last_row {
                        // Both rows already fit: navigating up must retain
                        // the current offset instead of recentering the list.
                        assert_eq!(harness.get_by_label("destination 59").rect(), last_row);
                    }
                    harness.key_up(key);
                    harness.step();
                    assert_navigation_row_visible(&harness, row);
                }
            }
        }
    }

    #[test]
    fn file_organize_pointer_scroll_is_preserved_until_keyboard_navigation() {
        use egui_kittest::kittest::Queryable;

        let mut harness = navigation_harness(egui::vec2(1000.0, 720.0));
        harness.key_press(egui::Key::ArrowRight);
        harness.step();
        assert_navigation_row_visible(&harness, 0);
        harness.hover_at(harness.get_by_label("destination 0").rect().center());
        harness.event(egui::Event::MouseWheel {
            unit: egui::MouseWheelUnit::Point,
            delta: egui::vec2(0.0, -600.0),
            modifiers: egui::Modifiers::NONE,
        });
        // Hover tooltips can keep requesting repaint; allow the wheel to
        // settle without waiting for the entire Modal to become idle.
        harness.run_steps(16);
        harness.hover_at(egui::Pos2::ZERO);
        harness.step();
        assert_eq!(
            harness.state().1,
            Some((0, Some(ShellTransferOperation::Copy)))
        );
        assert!(harness.query_by_label("destination 0").is_none());
        for _ in 0..32 {
            harness.step();
            assert!(harness.query_by_label("destination 0").is_none());
        }
        // Horizontal navigation also reveals the current offscreen row, even
        // when the chosen operation itself stays unchanged.
        harness.key_press(egui::Key::ArrowRight);
        harness.step();
        assert_navigation_row_visible(&harness, 0);
    }

    #[test]
    fn file_organize_modal_size_is_stable_across_frames_and_viewport_changes() {
        use egui_kittest::{Harness, kittest::Queryable};

        for count in [0, 1, 5, 60] {
            let destinations = (0..count)
                .map(|row| FileOrganizeDestination {
                    name: format!("整理先 {row}"),
                    path: PathBuf::from(
                        r"\\server\share\長い名前のフォルダ\さらに長いフォルダ\整理先",
                    ),
                })
                .collect::<Vec<_>>();
            let mut harness = Harness::builder()
                .with_size(egui::vec2(1000.0, 720.0))
                .build_state(
                    |ctx,
                     state: &mut (
                        Vec<FileOrganizeDestination>,
                        Option<(usize, Option<ShellTransferOperation>)>,
                        egui::Rect,
                        bool,
                    )| {
                        if !state.3 {
                            crate::ui_fonts::configure_fonts(ctx);
                            state.3 = true;
                            ctx.request_repaint();
                            return;
                        }
                        state.2 = show_file_organize_modal(
                            ctx,
                            &[r"C:\source\image.jpg".into()],
                            &state.0,
                            &mut state.1,
                        )
                        .response
                        .rect;
                    },
                    (destinations, None, egui::Rect::NOTHING, false),
                );
            for expanded in [false, true] {
                if expanded {
                    harness.get_by_label("対象の一覧").click();
                    harness.run_steps(8);
                }
                for size in [
                    egui::vec2(1000.0, 720.0),
                    egui::vec2(420.0, 320.0),
                    egui::vec2(1000.0, 720.0),
                ] {
                    harness.set_size(size);
                    harness.run_steps(8);
                    let settled = harness.state().2;
                    for _ in 0..32 {
                        harness.step();
                        assert!(
                            (harness.state().2.size() - settled.size()).length() < 0.5,
                            "count={count}, viewport={size:?}, settled={settled:?}, actual={:?}",
                            harness.state().2
                        );
                    }
                    assert!(settled.width() <= size.x && settled.height() <= size.y);
                    if count == 1 {
                        let moved = harness.get_by_label("移動").rect();
                        let copied = harness.get_by_label("コピー").rect();
                        let closed = harness.get_by_label("閉じる").rect();
                        assert!((moved.top() - copied.top()).abs() < 0.5);
                        assert!(copied.left() - moved.right() >= 24.0);
                        assert!(moved.bottom() < closed.top());
                        assert!(settled.contains_rect(moved) && settled.contains_rect(copied));
                    }
                }
            }
        }
    }

    #[test]
    fn file_organize_table_buttons_keep_pointer_focus_and_action() {
        use egui_kittest::{Harness, kittest::Queryable};
        let mut harness = Harness::builder()
            .with_size(egui::vec2(1000.0, 720.0))
            .build_state(
                |ctx,
                 state: &mut (
                    Option<(usize, Option<ShellTransferOperation>)>,
                    Option<FileOrganizeUiAction>,
                )| {
                    state.1 = show_file_organize_modal(
                        ctx,
                        &[r"C:\source\image.jpg".into()],
                        &[FileOrganizeDestination {
                            name: "destination".into(),
                            path: r"D:\destination".into(),
                        }],
                        &mut state.0,
                    )
                    .inner;
                },
                (None, None),
            );
        harness.run();
        for operation in [ShellTransferOperation::Move, ShellTransferOperation::Copy] {
            let label = if operation == ShellTransferOperation::Move {
                "移動"
            } else {
                "コピー"
            };
            harness.get_by_label(label).click();
            // Stop on the input frame so later idle frames do not overwrite the emitted action.
            harness.step();
            assert_eq!(harness.state().0, Some((0, Some(operation))));
            assert_eq!(
                harness.state().1,
                Some(FileOrganizeUiAction::Submit(0, operation))
            );
        }
    }

    fn selecting() -> FileOrganizeRequest {
        FileOrganizeRequest::Selecting(Box::new(FileOrganizeSelection {
            sources: vec![PathBuf::from(r"C:\source\a.jpg")],
            destinations: vec![FileOrganizeDestination {
                name: "archive".into(),
                path: PathBuf::from(r"C:\destination"),
            }],
            focus: None,
        }))
    }

    #[test]
    fn file_organize_selection_checks_every_target_and_keeps_grid_order() {
        let items = vec![
            GridItem::Image("a.jpg".into()),
            GridItem::Folder("folder".into()),
            GridItem::ZipImage {
                zip_path: "a.zip".into(),
                entry_name: "page.jpg".into(),
            },
        ];
        assert_eq!(
            resolve_sources(&items, &HashSet::new(), Some(1), &[0, 1, 2]).unwrap(),
            vec![PathBuf::from("folder")]
        );
        assert_eq!(
            resolve_sources(&items, &HashSet::from([0, 1]), Some(2), &[1, 0, 2]).unwrap(),
            vec![PathBuf::from("folder"), PathBuf::from("a.jpg")]
        );
        assert!(
            resolve_sources(&items, &HashSet::from([0, 2]), Some(1), &[0, 1, 2])
                .unwrap_err()
                .contains("実行しません")
        );
        assert!(resolve_sources(&items, &HashSet::from([8]), Some(1), &[0, 1, 2]).is_err());
        assert!(resolve_sources(&items, &HashSet::new(), Some(2), &[0, 1, 2]).is_err());
    }

    #[test]
    fn file_organize_admission_rejects_remote_exit_and_duplicate() {
        for (remote, closing, owner) in [
            (true, false, Some(1)),
            (false, true, Some(1)),
            (false, false, None),
        ] {
            let mut state = selecting();
            assert!(
                state
                    .take_for_submission(0, ShellTransferOperation::Move, remote, closing, owner)
                    .is_err()
            );
            assert!(matches!(state, FileOrganizeRequest::Hidden));
        }
        let mut state = selecting();
        let request = state
            .take_for_submission(0, ShellTransferOperation::Copy, false, false, Some(1))
            .unwrap()
            .unwrap();
        let (_tx, rx) = mpsc::channel();
        state = FileOrganizeRequest::Running(Box::new(FileOrganizeRunning { request, rx }));
        assert!(
            state
                .take_for_submission(0, ShellTransferOperation::Move, true, true, Some(1))
                .unwrap()
                .is_none()
        );
        assert!(matches!(state, FileOrganizeRequest::Running(_)));
    }

    #[test]
    fn file_organize_refresh_uses_current_source_or_destination_only() {
        let request = ShellTransferRequest {
            sources: vec![PathBuf::from(r"C:\source\a.jpg")],
            destination: PathBuf::from(r"D:\destination"),
            operation: ShellTransferOperation::Move,
        };
        assert!(refresh_matches(Path::new(r"C:\source"), &request));
        assert!(refresh_matches(Path::new(r"D:\destination"), &request));
        assert!(!refresh_matches(Path::new(r"C:\elsewhere"), &request));
    }
}
