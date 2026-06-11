//! UI glue for the left filesystem folder tree pane.

use eframe::egui;

use crate::app::App;
use crate::folder_pane::{self, FolderPaneCommand, FolderPaneTreeKey};
use crate::ui_main::AddressBarNav;

impl App {
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
                    return Some(AddressBarNav::Direct(path));
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
        egui::SidePanel::left("folder_tree_pane")
            .resizable(true)
            .default_width(260.0)
            .width_range(180.0..=460.0)
            .show(ctx, |ui| {
                ui.add_enabled_ui(!disabled, |ui| {
                    self.render_folder_pane_header(ui);
                    ui.separator();
                    nav = self.render_folder_pane_tree(ui);
                });
            });
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
                for row in rows {
                    let mut row_response = None;
                    ui.horizontal(|ui| {
                        ui.add_space(row.depth as f32 * 14.0);
                        let arrow = if row.has_children_or_unknown {
                            if row.expanded { "▾" } else { "▸" }
                        } else {
                            " "
                        };
                        let arrow_resp = ui.add_sized(
                            egui::vec2(18.0, 20.0),
                            egui::Button::new(arrow).small().frame(false),
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
                        let resp = ui
                            .selectable_label(selected, text)
                            .on_hover_text(row.path.display().to_string());
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
