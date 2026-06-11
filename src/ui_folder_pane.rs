//! UI glue for the left filesystem folder tree pane.

use eframe::egui;

use crate::app::App;
use crate::folder_pane::{self, FolderPaneCommand, FolderPaneTreeKey};
use crate::ui_main::AddressBarNav;

const FOLDER_PANE_MIN_WIDTH: f32 = 180.0;
const FOLDER_PANE_MAX_WIDTH: f32 = 900.0;
const FOLDER_PANE_MIN_RATIO: f32 = 0.12;
const FOLDER_PANE_MAX_RATIO: f32 = 0.45;
const FOLDER_PANE_ROW_HEIGHT: f32 = 24.0;
const FOLDER_PANE_INDENT_WIDTH: f32 = 14.0;
const FOLDER_PANE_ARROW_WIDTH: f32 = 24.0;

impl App {
    pub(crate) fn set_folder_tree_pane_visible(&mut self, visible: bool) {
        self.settings.folder_tree_pane_visible = visible;
        if visible {
            let active = self.effective_folder();
            self.folder_pane
                .sync_to_active(active.as_deref(), self.settings.sort_order);
            self.folder_pane.set_focus_tree_at_active();
        } else {
            self.folder_pane.set_focus_grid();
        }
        self.settings.save();
    }

    /// トグルキー (キーボード T / ゲームパッド Y) でフォルダツリーペインを開閉する。
    /// - 非表示 → 表示 (カーソルはアクティブフォルダに置かれる)。
    /// - 表示中 → 閉じる。ただしカーソルが別フォルダへ動いていれば、Enter と同じく
    ///   そのフォルダへ移動してグリッド一覧に戻ってから閉じる
    ///   (worker scan 経由なので UI スレッドの read_dir で固まらない)。
    pub(crate) fn toggle_folder_tree_pane_from_key(&mut self) {
        if !self.settings.folder_tree_pane_visible {
            self.set_folder_tree_pane_visible(true);
            return;
        }
        // 閉じる前に「カーソルが別フォルダへ動いていれば」その移動先を取得する。
        let target = self.folder_pane.cursor_nav_target_if_moved();
        // set_folder_tree_pane_visible(false) が focus をグリッドへ戻し settings も保存する。
        self.set_folder_tree_pane_visible(false);
        if let Some(target) = target {
            self.start_folder_pane_open(target);
        }
    }

    pub(crate) fn sync_folder_pane_state(&mut self, ctx: &egui::Context) {
        if !self.settings.folder_tree_pane_visible {
            self.folder_pane.set_focus_grid();
            return;
        }
        let active = self.effective_folder();
        self.folder_pane
            .sync_to_active(active.as_deref(), self.settings.sort_order);
        if self.folder_pane.poll_pending() {
            ctx.request_repaint();
        }
        if self.folder_pane.has_pending() {
            ctx.request_repaint_after(std::time::Duration::from_millis(80));
        }
    }

    pub(crate) fn folder_pane_disabled(&self) -> bool {
        self.is_snapshot_active()
    }

    pub(crate) fn folder_pane_blocks_grid_keyboard(&self) -> bool {
        self.settings.folder_tree_pane_visible
            && self.folder_pane.has_focus
            && !self.folder_pane_disabled()
    }

    pub(crate) fn handle_folder_pane_keyboard(
        &mut self,
        ctx: &egui::Context,
    ) -> Option<AddressBarNav> {
        if !self.settings.folder_tree_pane_visible {
            self.folder_pane.set_focus_grid();
            return None;
        }
        self.sync_folder_pane_state(ctx);

        if self.folder_pane_disabled() {
            self.folder_pane.set_focus_grid();
            return None;
        }

        if self.folder_pane.has_focus {
            let key = ctx.input_mut(|i| {
                if i.consume_key(egui::Modifiers::NONE, egui::Key::ArrowUp) {
                    Some(FolderPaneTreeKey::Up)
                } else if i.consume_key(egui::Modifiers::NONE, egui::Key::ArrowDown) {
                    Some(FolderPaneTreeKey::Down)
                } else if i.consume_key(egui::Modifiers::NONE, egui::Key::ArrowLeft) {
                    Some(FolderPaneTreeKey::Left)
                } else if i.consume_key(egui::Modifiers::NONE, egui::Key::ArrowRight) {
                    Some(FolderPaneTreeKey::Right)
                } else if i.consume_key(egui::Modifiers::NONE, egui::Key::Enter) {
                    Some(FolderPaneTreeKey::Enter)
                } else {
                    None
                }
            });
            if let Some(key) = key {
                if let Some(FolderPaneCommand::Open(path)) = self
                    .folder_pane
                    .handle_tree_key(key, self.settings.sort_order)
                {
                    self.folder_pane.set_focus_grid();
                    // クリック経路と同じく worker scan 経由で開く (UI スレッドの
                    // read_dir ブロック回避)。Enter は consume 済みなので、この後
                    // grid keyboard へ抜けても二重発火しない。
                    self.start_folder_pane_open(path);
                    return None;
                }
            }
            return None;
        }

        let esc_to_tree_allowed = self.fullscreen_idx.is_none()
            && !self.any_dialog_open()
            && !self.show_search_bar
            && !self.favsearch.active
            && !self.global_search.active
            && !self.address_has_focus
            && !self.search_has_focus
            && !self.favsearch.has_focus
            && !self.global_search.has_focus;
        if esc_to_tree_allowed
            && ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Escape))
        {
            self.folder_pane.set_focus_tree_at_active();
            return None;
        }

        None
    }

    pub(crate) fn render_folder_pane(&mut self, ctx: &egui::Context) -> Option<std::path::PathBuf> {
        if !self.settings.folder_tree_pane_visible {
            return None;
        }

        self.sync_folder_pane_state(ctx);
        let disabled = self.folder_pane_disabled();
        if disabled {
            self.folder_pane.set_focus_grid();
        }

        let mut nav = None;
        let content_width = ctx.content_rect().width().max(1.0);
        let max_width =
            FOLDER_PANE_MAX_WIDTH.min((content_width - 240.0).max(FOLDER_PANE_MIN_WIDTH));
        let default_width = (self.settings.folder_tree_pane_width_ratio * content_width)
            .clamp(FOLDER_PANE_MIN_WIDTH, max_width);
        let panel = egui::SidePanel::left("folder_tree_pane")
            .resizable(true)
            .default_width(default_width)
            .width_range(FOLDER_PANE_MIN_WIDTH..=max_width)
            .show(ctx, |ui| {
                ui.add_enabled_ui(!disabled, |ui| {
                    self.render_folder_pane_header(ui);
                    ui.separator();
                    nav = self.render_folder_pane_tree(ui);
                });
            });
        let width_ratio = (panel.response.rect.width() / content_width)
            .clamp(FOLDER_PANE_MIN_RATIO, FOLDER_PANE_MAX_RATIO);
        if width_ratio.is_finite()
            && (self.settings.folder_tree_pane_width_ratio - width_ratio).abs() > 0.0005
        {
            self.settings.folder_tree_pane_width_ratio = width_ratio;
        }
        nav
    }

    fn render_folder_pane_header(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            let selected_drive = self.folder_pane.selected_drive.clone();
            let selected_text = selected_drive
                .as_deref()
                .map(folder_pane::drive_label)
                .unwrap_or_else(|| "Drive".to_string());
            let mut next_drive = selected_drive.clone();
            let drives = self.folder_pane.drives().to_vec();
            egui::ComboBox::from_id_salt("folder_pane_drive_combo")
                .width(84.0)
                .selected_text(selected_text)
                .show_ui(ui, |ui| {
                    for drive in drives {
                        let label = folder_pane::drive_label(&drive);
                        let selected = selected_drive
                            .as_ref()
                            .is_some_and(|current| crate::folder_tree::path_eq(current, &drive));
                        if ui.selectable_label(selected, label).clicked() {
                            next_drive = Some(drive);
                        }
                    }
                });
            if let Some(drive) = next_drive
                && selected_drive
                    .as_ref()
                    .is_none_or(|current| !crate::folder_tree::path_eq(current, &drive))
            {
                self.folder_pane
                    .select_drive(drive, self.settings.sort_order);
            }

            let reload = ui
                .small_button("↻")
                .on_hover_text("フォルダツリーを再読み込み");
            if reload.clicked() {
                let active = self.effective_folder();
                self.folder_pane
                    .reload_for_active(active.as_deref(), self.settings.sort_order);
            }
        });
    }

    fn render_folder_pane_tree(&mut self, ui: &mut egui::Ui) -> Option<std::path::PathBuf> {
        let rows = self.folder_pane.visible_rows();
        if rows.is_empty() {
            ui.centered_and_justified(|ui| {
                ui.label("ドライブが見つかりません");
            });
            return None;
        }

        let mut nav = None;
        let scroll_to_cursor = self.folder_pane.scroll_to_cursor;
        let mut cursor_scrolled = false;
        egui::ScrollArea::vertical()
            .id_salt("folder_pane_tree_scroll")
            .auto_shrink([false, false])
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                for row in rows {
                    let mut row_response = None;
                    ui.horizontal(|ui| {
                        ui.set_height(FOLDER_PANE_ROW_HEIGHT);
                        ui.spacing_mut().item_spacing.x = 2.0;
                        ui.add_space(row.depth as f32 * FOLDER_PANE_INDENT_WIDTH);
                        let arrow = if row.has_children_or_unknown {
                            if row.expanded { "▼" } else { "▶" }
                        } else {
                            " "
                        };
                        let arrow_resp = ui.add_sized(
                            egui::vec2(FOLDER_PANE_ARROW_WIDTH, FOLDER_PANE_ROW_HEIGHT),
                            egui::Button::new(egui::RichText::new(arrow).size(16.0).strong())
                                .frame(false),
                        );
                        if row.has_children_or_unknown && arrow_resp.clicked() {
                            self.folder_pane.set_focus_tree();
                            self.folder_pane.set_cursor(row.path.clone());
                            let key = if row.expanded {
                                FolderPaneTreeKey::Left
                            } else {
                                FolderPaneTreeKey::Right
                            };
                            let _ = self
                                .folder_pane
                                .handle_tree_key(key, self.settings.sort_order);
                        }

                        let label = folder_pane::folder_label(&row.path);
                        let mut text = egui::RichText::new(label);
                        if row.is_active {
                            text = text.strong();
                        }
                        if row.error.is_some() {
                            text = text.color(ui.visuals().warn_fg_color);
                        }
                        let selected = row.is_cursor && self.folder_pane.has_focus;
                        let trailing_width = if row.loading || row.error.is_some() {
                            18.0
                        } else {
                            0.0
                        };
                        let label_width = (ui.available_width() - trailing_width).max(16.0);
                        let (label_rect, label_resp) = ui.allocate_exact_size(
                            egui::vec2(label_width, FOLDER_PANE_ROW_HEIGHT),
                            egui::Sense::click(),
                        );
                        if selected {
                            ui.painter().rect_filled(
                                label_rect,
                                ui.visuals().widgets.active.corner_radius,
                                ui.visuals().selection.bg_fill,
                            );
                        }
                        let mut label_ui = ui.new_child(
                            egui::UiBuilder::new()
                                .max_rect(label_rect)
                                .layout(egui::Layout::left_to_right(egui::Align::Center)),
                        );
                        label_ui.set_clip_rect(label_rect.intersect(ui.clip_rect()));
                        label_ui.add(
                            egui::Label::new(text)
                                .truncate()
                                .selectable(false)
                                .halign(egui::Align::Min),
                        );
                        let resp = label_resp.on_hover_text(row.path.display().to_string());
                        if resp.clicked() {
                            self.folder_pane.set_focus_tree();
                            self.folder_pane.set_cursor(row.path.clone());
                            if self.folder_pane.active_path().is_none_or(|active| {
                                !crate::folder_tree::path_eq(active, &row.path)
                            }) {
                                nav = Some(row.path.clone());
                            }
                        }
                        if row.loading {
                            ui.spinner();
                        } else if row.error.is_some() {
                            ui.label(egui::RichText::new("!").color(ui.visuals().warn_fg_color));
                        }
                        row_response = Some(resp);
                    });

                    if scroll_to_cursor
                        && row.is_cursor
                        && let Some(resp) = row_response
                    {
                        resp.scroll_to_me(Some(egui::Align::Center));
                        cursor_scrolled = true;
                    }
                }
            });
        if scroll_to_cursor && cursor_scrolled {
            self.folder_pane.scroll_to_cursor = false;
        }
        nav
    }
}
