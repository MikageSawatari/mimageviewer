//! スマートフォルダの作成・管理ダイアログ。
//!
//! 定義は `Settings::smart_folders` を正本として即時保存する。ファイル走査はここでは
//! 行わず、一覧を開いたときだけ `app::smart_folder` の background worker が実行する。

use std::collections::BTreeSet;
use std::path::PathBuf;

use eframe::egui;

use crate::app::App;
use crate::settings::{
    FacetDatePreset, FacetSizePreset, GridViewMode, SmartFolderContainerKind,
    SmartFolderDefinition, SmartFolderSource, SortOrder, SubfolderExpansionOrder,
};

fn source_path_exists(sources: &[SmartFolderSource], path: &std::path::Path) -> bool {
    let key = crate::path_key::normalize_keep_drive(path);
    sources
        .iter()
        .any(|source| crate::path_key::normalize_keep_drive(&source.path) == key)
}

fn push_source_if_new(definition: &mut SmartFolderDefinition, path: PathBuf) -> bool {
    if path.as_os_str().is_empty() || source_path_exists(&definition.sources, &path) {
        return false;
    }
    definition.sources.push(SmartFolderSource {
        id: uuid::Uuid::new_v4(),
        path,
        enabled: true,
    });
    true
}

impl App {
    pub(crate) fn begin_new_smart_folder(&mut self) {
        let number = self.settings.smart_folders.len() + 1;
        let mut definition = SmartFolderDefinition::new(format!("スマートフォルダ {number}"));
        if let Some(folder) = self.effective_folder().filter(|path| path.is_dir()) {
            push_source_if_new(&mut definition, folder);
        }
        let id = definition.id;
        self.settings.smart_folders.push(definition);
        self.settings.save();
        self.smart_folder_editor_selected = Some(id);
        self.show_smart_folder_editor = true;
    }

    pub(crate) fn open_smart_folder_manager(&mut self, selected: Option<uuid::Uuid>) {
        self.smart_folder_editor_selected = selected.or_else(|| {
            self.settings
                .smart_folders
                .first()
                .map(|definition| definition.id)
        });
        self.show_smart_folder_editor = true;
    }

    pub(crate) fn show_smart_folder_editor_dialog(&mut self, ctx: &egui::Context) {
        if !self.show_smart_folder_editor {
            return;
        }

        if self.smart_folder_editor_selected.is_none() {
            self.smart_folder_editor_selected = self
                .settings
                .smart_folders
                .first()
                .map(|definition| definition.id);
        }
        let selected_id = self.smart_folder_editor_selected;
        let selected_index = selected_id.and_then(|id| {
            self.settings
                .smart_folders
                .iter()
                .position(|definition| definition.id == id)
        });
        let mut draft = selected_index.map(|index| self.settings.smart_folders[index].clone());
        let favorite_sources: Vec<(String, PathBuf)> = self
            .settings
            .favorites
            .iter()
            .map(|favorite| (favorite.name.clone(), favorite.path.clone()))
            .collect();
        let known_tags: Vec<String> = self
            .settings
            .tags
            .iter()
            .map(|tag| tag.name.clone())
            .collect();

        let mut open = true;
        let mut close_requested = false;
        let mut create_requested = false;
        let mut select_requested = None;
        let mut definition_swap = None;
        let mut delete_requested = None;
        let mut source_remove = None;
        let mut source_swap = None;
        let mut pick_source_folder = false;
        let mut add_favorite_source = None;
        let mut open_definition = None;
        let mut invalidated_definition = None;
        let mut deleted_definition = None;
        let escape_pressed = self.dialog_escape_pressed(ctx);
        let dialog_pos = ctx.content_rect().min + egui::vec2(48.0, 32.0);

        egui::Window::new("スマートフォルダ")
            .open(&mut open)
            .resizable(true)
            .collapsible(false)
            .default_pos(dialog_pos)
            .default_size(egui::vec2(900.0, 650.0))
            .min_size(egui::vec2(720.0, 460.0))
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    ui.vertical(|ui| {
                        ui.set_width(220.0);
                        ui.horizontal(|ui| {
                            if ui.button("新規").clicked() {
                                create_requested = true;
                            }
                            let can_move = selected_index.is_some();
                            if ui
                                .add_enabled(
                                    can_move && selected_index != Some(0),
                                    egui::Button::new("↑"),
                                )
                                .clicked()
                            {
                                let index = selected_index.unwrap();
                                definition_swap = Some((index, index - 1));
                            }
                            if ui
                                .add_enabled(
                                    can_move
                                        && selected_index.is_some_and(|index| {
                                            index + 1 < self.settings.smart_folders.len()
                                        }),
                                    egui::Button::new("↓"),
                                )
                                .clicked()
                            {
                                let index = selected_index.unwrap();
                                definition_swap = Some((index, index + 1));
                            }
                            if ui
                                .add_enabled(can_move, egui::Button::new("削除"))
                                .clicked()
                            {
                                delete_requested = selected_id;
                            }
                        });
                        ui.separator();
                        egui::ScrollArea::vertical()
                            .id_salt("smart_folder_definition_list")
                            .show(ui, |ui| {
                                if self.settings.smart_folders.is_empty() {
                                    ui.label(egui::RichText::new("(未登録)").weak());
                                }
                                for definition in &self.settings.smart_folders {
                                    if ui
                                        .selectable_label(
                                            selected_id == Some(definition.id),
                                            &definition.name,
                                        )
                                        .on_hover_text(format!(
                                            "{} 件の検索元",
                                            definition.sources.len()
                                        ))
                                        .clicked()
                                    {
                                        select_requested = Some(definition.id);
                                    }
                                }
                            });
                    });

                    ui.separator();
                    ui.vertical(|ui| {
                        if let Some(definition) = draft.as_mut() {
                            ui.horizontal(|ui| {
                                ui.label("名前:");
                                ui.text_edit_singleline(&mut definition.name);
                                if ui.button("開く").clicked() {
                                    open_definition = Some(definition.id);
                                }
                            });
                            ui.add_space(4.0);
                            ui.label(egui::RichText::new("検索元フォルダ").strong());
                            ui.horizontal(|ui| {
                                if ui.button("フォルダを追加…").clicked() {
                                    pick_source_folder = true;
                                }
                                ui.menu_button("お気に入りから追加", |ui| {
                                    if favorite_sources.is_empty() {
                                        ui.add_enabled(false, egui::Button::new("(未登録)"));
                                    }
                                    for (name, path) in &favorite_sources {
                                        let already = source_path_exists(&definition.sources, path);
                                        if ui
                                            .add_enabled(!already, egui::Button::new(name))
                                            .on_hover_text(path.to_string_lossy())
                                            .clicked()
                                        {
                                            add_favorite_source = Some(path.clone());
                                            ui.close();
                                        }
                                    }
                                });
                                ui.label(
                                    egui::RichText::new(
                                        "開くたびに有効な検索元をバックグラウンド走査します",
                                    )
                                    .weak(),
                                );
                            });
                            egui::ScrollArea::vertical()
                                .id_salt("smart_folder_source_list")
                                .max_height(150.0)
                                .show(ui, |ui| {
                                    let source_count = definition.sources.len();
                                    for (index, source) in definition.sources.iter_mut().enumerate()
                                    {
                                        ui.horizontal(|ui| {
                                            ui.checkbox(&mut source.enabled, "");
                                            ui.label(source.path.to_string_lossy())
                                                .on_hover_text(source.path.to_string_lossy());
                                            if ui.small_button("↑").clicked() && index > 0 {
                                                source_swap = Some((index, index - 1));
                                            }
                                            if ui.small_button("↓").clicked()
                                                && index + 1 < source_count
                                            {
                                                source_swap = Some((index, index + 1));
                                            }
                                            if ui.small_button("削除").clicked() {
                                                source_remove = Some(index);
                                            }
                                        });
                                    }
                                });

                            ui.separator();
                            ui.label(egui::RichText::new("表示").strong());
                            ui.horizontal_wrapped(|ui| {
                                egui::ComboBox::from_id_salt("smart_folder_sort")
                                    .selected_text(definition.sort.label())
                                    .show_ui(ui, |ui| {
                                        for &sort in SortOrder::all() {
                                            ui.selectable_value(
                                                &mut definition.sort,
                                                sort,
                                                sort.label(),
                                            );
                                        }
                                    });
                                egui::ComboBox::from_id_salt("smart_folder_grouping")
                                    .selected_text(definition.grouping.label())
                                    .show_ui(ui, |ui| {
                                        for &grouping in SubfolderExpansionOrder::all() {
                                            ui.selectable_value(
                                                &mut definition.grouping,
                                                grouping,
                                                grouping.label(),
                                            );
                                        }
                                    });
                                egui::ComboBox::from_id_salt("smart_folder_view_mode")
                                    .selected_text(definition.view_mode.label())
                                    .show_ui(ui, |ui| {
                                        for &view_mode in GridViewMode::all() {
                                            ui.selectable_value(
                                                &mut definition.view_mode,
                                                view_mode,
                                                view_mode.label(),
                                            );
                                        }
                                    });
                            });

                            ui.separator();
                            ui.label(egui::RichText::new("保存する条件").strong());
                            ui.horizontal(|ui| {
                                ui.label("名前に含む:");
                                ui.text_edit_singleline(&mut definition.filter.name_contains);
                            });
                            ui.horizontal_wrapped(|ui| {
                                ui.label("種類:");
                                for &kind in SmartFolderContainerKind::all() {
                                    let mut enabled = definition.filter.kinds.contains(&kind);
                                    if ui.checkbox(&mut enabled, kind.label()).changed() {
                                        if enabled {
                                            definition.filter.kinds.insert(kind);
                                        } else {
                                            definition.filter.kinds.remove(&kind);
                                        }
                                    }
                                }
                                ui.label(egui::RichText::new("(未選択は全種類)").weak());
                            });
                            let mut extensions = definition
                                .filter
                                .extensions
                                .iter()
                                .cloned()
                                .collect::<Vec<_>>()
                                .join(", ");
                            ui.horizontal(|ui| {
                                ui.label("拡張子:");
                                if ui.text_edit_singleline(&mut extensions).changed() {
                                    definition.filter.extensions = extensions
                                        .split([',', '、', ' '])
                                        .map(|value| {
                                            value
                                                .trim()
                                                .trim_start_matches('.')
                                                .to_ascii_lowercase()
                                        })
                                        .filter(|value| !value.is_empty())
                                        .collect::<BTreeSet<_>>();
                                }
                                ui.label(egui::RichText::new("(空欄は全て)").weak());
                            });
                            ui.horizontal_wrapped(|ui| {
                                ui.label("更新日:");
                                egui::ComboBox::from_id_salt("smart_folder_date_filter")
                                    .selected_text(
                                        definition
                                            .filter
                                            .date_preset
                                            .map(FacetDatePreset::label)
                                            .unwrap_or("指定なし"),
                                    )
                                    .show_ui(ui, |ui| {
                                        ui.selectable_value(
                                            &mut definition.filter.date_preset,
                                            None,
                                            "指定なし",
                                        );
                                        for &preset in FacetDatePreset::all() {
                                            ui.selectable_value(
                                                &mut definition.filter.date_preset,
                                                Some(preset),
                                                preset.label(),
                                            );
                                        }
                                    });
                                ui.label("サイズ:");
                                egui::ComboBox::from_id_salt("smart_folder_size_filter")
                                    .selected_text(
                                        definition
                                            .filter
                                            .size_preset
                                            .map(FacetSizePreset::label)
                                            .unwrap_or("指定なし"),
                                    )
                                    .show_ui(ui, |ui| {
                                        ui.selectable_value(
                                            &mut definition.filter.size_preset,
                                            None,
                                            "指定なし",
                                        );
                                        for &preset in FacetSizePreset::all() {
                                            ui.selectable_value(
                                                &mut definition.filter.size_preset,
                                                Some(preset),
                                                preset.label(),
                                            );
                                        }
                                    });
                            });
                            ui.horizontal_wrapped(|ui| {
                                ui.label("レーティング:");
                                for rating in 0..=5 {
                                    let label = if rating == 0 {
                                        "未評価".to_string()
                                    } else {
                                        format!("★{rating}")
                                    };
                                    ui.checkbox(&mut definition.filter.ratings[rating], label);
                                }
                            });
                            if !known_tags.is_empty() {
                                ui.horizontal_wrapped(|ui| {
                                    ui.label("タグ:");
                                    for tag in &known_tags {
                                        let mut enabled = definition.filter.tags.contains(tag);
                                        if ui.checkbox(&mut enabled, tag).changed() {
                                            if enabled {
                                                definition.filter.tags.insert(tag.clone());
                                            } else {
                                                definition.filter.tags.remove(tag);
                                            }
                                        }
                                    }
                                    ui.checkbox(
                                        &mut definition.filter.include_untagged,
                                        "タグなしも含む",
                                    );
                                });
                            }
                        } else {
                            ui.centered_and_justified(|ui| {
                                ui.label("左の一覧から選ぶか、「新規」を押してください");
                            });
                        }
                    });
                });

                ui.separator();
                ui.horizontal(|ui| {
                    if ui.button("閉じる").clicked() {
                        close_requested = true;
                    }
                    ui.label(
                        egui::RichText::new("外部の変更は自動監視せず、開く／更新時に再走査します")
                            .weak(),
                    );
                });
            });

        let mut dirty = false;
        if let (Some(index), Some(mut updated)) = (selected_index, draft) {
            if let Some((a, b)) = source_swap {
                updated.sources.swap(a, b);
            }
            if let Some(index) = source_remove {
                if index < updated.sources.len() {
                    updated.sources.remove(index);
                }
            }
            if let Some(path) = add_favorite_source {
                push_source_if_new(&mut updated, path);
            }
            updated.name = updated.name.trim().to_string();
            if updated.name.is_empty() {
                updated.name = "スマートフォルダ".to_string();
            }
            if self.settings.smart_folders.get(index) != Some(&updated) {
                invalidated_definition = Some(updated.id);
                self.settings.smart_folders[index] = updated;
                dirty = true;
            }
        }

        if pick_source_folder {
            let start = self
                .effective_folder()
                .filter(|path| path.is_dir())
                .unwrap_or_else(|| PathBuf::from("."));
            if let Some(path) = rfd::FileDialog::new().set_directory(start).pick_folder()
                && let Some(index) = selected_index
                && let Some(definition) = self.settings.smart_folders.get_mut(index)
                && push_source_if_new(definition, path)
            {
                invalidated_definition = Some(definition.id);
                dirty = true;
            }
        }
        if let Some((a, b)) = definition_swap {
            self.settings.smart_folders.swap(a, b);
            dirty = true;
        }
        if let Some(id) = select_requested {
            self.smart_folder_editor_selected = Some(id);
        }
        if create_requested {
            self.begin_new_smart_folder();
        }
        if let Some(id) = delete_requested {
            self.smart_folder_delete_confirm = Some(id);
        }

        if let Some(id) = self.smart_folder_delete_confirm {
            let target = self
                .settings
                .smart_folders
                .iter()
                .find(|definition| definition.id == id)
                .map(|definition| definition.name.clone());
            if let Some(name) = target {
                let mut confirmed = false;
                let mut cancel = false;
                let response = egui::Modal::new(egui::Id::new("smart_folder_delete_confirm")).show(
                    ctx,
                    |ui| {
                        ui.heading("スマートフォルダを削除");
                        ui.label(format!(
                            "「{name}」を削除します。実ファイルは削除されません。"
                        ));
                        ui.horizontal(|ui| {
                            if ui.button("削除").clicked() {
                                confirmed = true;
                            }
                            if ui.button("キャンセル").clicked() {
                                cancel = true;
                            }
                        });
                    },
                );
                if confirmed {
                    if let Some(index) = self
                        .settings
                        .smart_folders
                        .iter()
                        .position(|definition| definition.id == id)
                    {
                        self.settings.smart_folders.remove(index);
                        self.smart_folder_editor_selected = self
                            .settings
                            .smart_folders
                            .get(index.min(self.settings.smart_folders.len().saturating_sub(1)))
                            .map(|definition| definition.id);
                        deleted_definition = Some(id);
                        dirty = true;
                    }
                    self.smart_folder_delete_confirm = None;
                } else if cancel || response.should_close() {
                    self.smart_folder_delete_confirm = None;
                }
            } else {
                self.smart_folder_delete_confirm = None;
            }
        }

        if dirty {
            self.settings.save();
        }
        if let Some(id) = invalidated_definition {
            self.invalidate_smart_folder_definition(id);
        }
        if let Some(id) = deleted_definition {
            self.forget_smart_folder_definition(id);
        }
        if let Some(id) = open_definition {
            let refresh =
                self.items_are_smart_folder_view && self.current_smart_folder_id == Some(id);
            self.open_smart_folder(id, refresh);
        }
        if escape_pressed || close_requested || !open {
            self.show_smart_folder_editor = false;
            self.smart_folder_delete_confirm = None;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn duplicate_source_paths_are_not_added_twice() {
        let mut definition = SmartFolderDefinition::new("test");
        assert!(push_source_if_new(
            &mut definition,
            PathBuf::from(r"C:\Books")
        ));
        assert!(!push_source_if_new(
            &mut definition,
            PathBuf::from(r"c:\books")
        ));
        assert_eq!(definition.sources.len(), 1);
    }
}
