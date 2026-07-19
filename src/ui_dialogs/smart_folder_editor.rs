//! スマートフォルダの作成・ルール追加・管理 UI。
//!
//! 条件の正本は通常一覧の絞り込み UI。管理画面で同じ巨大フォームを再実装せず、
//! 「現在の実フォルダ + 現在の絞り込み」を確認ダイアログから OR ルールとして追加する。

use std::path::PathBuf;

use eframe::egui;

use crate::app::App;
use crate::settings::{
    FacetItemKind, SmartFolderDefinition, SmartFolderFilter, SmartFolderRule,
    SubfolderExpansionOrder,
};

const SAVABLE_KINDS: &[FacetItemKind] = &[
    FacetItemKind::Folder,
    FacetItemKind::Image,
    FacetItemKind::Video,
    FacetItemKind::Audio,
    FacetItemKind::Zip,
    FacetItemKind::Pdf,
    FacetItemKind::Archive,
];

#[derive(Clone, Debug)]
pub(crate) struct SmartFolderRuleDraft {
    pub(crate) target_id: uuid::Uuid,
    pub(crate) sources: Vec<PathBuf>,
    pub(crate) include_descendants: bool,
    pub(crate) filter: SmartFolderFilter,
    pub(crate) ignored_conditions: Vec<String>,
}

fn filter_summary(filter: &SmartFolderFilter) -> Vec<String> {
    let mut parts = Vec::new();
    if !filter.name_contains.is_empty() {
        parts.push(format!("名前に「{}」を含む", filter.name_contains));
    }
    if !filter.kinds.is_empty() {
        parts.push(format!(
            "種類: {}",
            filter
                .kinds
                .iter()
                .map(|kind| kind.label())
                .collect::<Vec<_>>()
                .join("、")
        ));
    }
    if !filter.extensions.is_empty() {
        parts.push(format!(
            "拡張子: {}",
            filter
                .extensions
                .iter()
                .map(|extension| format!(".{extension}"))
                .collect::<Vec<_>>()
                .join("、")
        ));
    }
    if let Some(date) = filter.date_preset {
        parts.push(format!("更新日: {}", date.label()));
    }
    if let Some(size) = filter.size_preset {
        parts.push(format!("サイズ: {}", size.label()));
    }
    if filter.ratings != [true; 6] {
        let ratings = (0..=5)
            .filter(|rating| filter.ratings[*rating])
            .map(|rating| {
                if rating == 0 {
                    "未評価".to_string()
                } else {
                    format!("★{rating}")
                }
            })
            .collect::<Vec<_>>()
            .join("、");
        parts.push(format!("レーティング: {ratings}"));
    }
    if !filter.tags.is_empty() || filter.include_untagged {
        let mut tags = filter
            .tags
            .iter()
            .map(|tag| format!("#{tag}"))
            .collect::<Vec<_>>();
        if filter.include_untagged {
            tags.push("タグなし".to_string());
        }
        parts.push(format!(
            "タグ({}): {}",
            filter.tag_mode.label(),
            tags.join("、")
        ));
    }
    if !filter.edits.is_empty() {
        parts.push(format!(
            "状態: {}{}",
            filter
                .edits
                .iter()
                .map(|flag| flag.menu_label())
                .collect::<Vec<_>>()
                .join("、"),
            if filter.edit_include_descendants {
                "（子フォルダも対象）"
            } else {
                ""
            }
        ));
    }
    if parts.is_empty() {
        parts.push("追加の絞り込みなし".to_string());
    }
    parts
}

fn rule_summary(rule: &SmartFolderRule) -> String {
    let mut parts = filter_summary(&rule.filter);
    parts.insert(
        0,
        if rule.include_descendants {
            "サブフォルダを含む".to_string()
        } else {
            "このフォルダ直下のみ".to_string()
        },
    );
    parts.join(" / ")
}

fn capture_smart_folder_filter(
    facet: &crate::settings::FacetFilter,
    ratings: [bool; 6],
) -> SmartFolderFilter {
    let mut filter = SmartFolderFilter::default();
    filter.kinds = facet
        .kinds
        .iter()
        .filter(|kind| SAVABLE_KINDS.contains(kind))
        .copied()
        .collect();
    filter.extensions = facet.exts.clone();
    filter.date_preset = facet.date_preset;
    filter.size_preset = facet.size_preset;
    filter.ratings = ratings;
    filter.tags = facet.tags.clone();
    filter.tag_mode = facet.tag_mode;
    filter.include_untagged = facet.include_untagged;
    filter.edits = facet.edits.clone();
    filter.edit_include_descendants = facet.edit_include_descendants;
    filter
}

impl App {
    pub(crate) fn begin_new_smart_folder(&mut self) {
        let mut number = self.settings.smart_folders.len() + 1;
        loop {
            let candidate = format!("スマートフォルダ {number}");
            if !self
                .settings
                .smart_folders
                .iter()
                .any(|definition| definition.name.eq_ignore_ascii_case(&candidate))
            {
                self.smart_folder_create_name = Some(candidate);
                break;
            }
            number += 1;
        }
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

    /// 現在表示を再現できる実検索元。I/O は行わず、既存の view state だけで判定する。
    pub(crate) fn smart_folder_current_rule_source(
        &self,
    ) -> Result<(Vec<PathBuf>, bool), &'static str> {
        if self.items_are_subfolder_expansion_view {
            let roots = if self.subfolder_expansion_roots.is_empty() {
                self.subfolder_expansion_root.iter().cloned().collect()
            } else {
                self.subfolder_expansion_roots.clone()
            };
            return if roots.is_empty() {
                Err("サブ展開の検索元を特定できないため追加できません")
            } else {
                Ok((roots, true))
            };
        }
        if self.subfolder_expansion_available()
            && let Some(folder) = self.current_folder.clone()
        {
            return Ok((vec![folder], false));
        }
        Err(
            "ZIP／PDF内、検索結果、読書履歴など、実フォルダを検索元として特定できない表示では追加できません",
        )
    }

    pub(crate) fn begin_add_current_smart_folder_rule(&mut self, target_id: uuid::Uuid) {
        if !self
            .settings
            .smart_folders
            .iter()
            .any(|definition| definition.id == target_id)
        {
            return;
        }
        let Ok((sources, include_descendants)) = self.smart_folder_current_rule_source() else {
            return;
        };
        let facet = &self.settings.facet_filter;
        let filter = capture_smart_folder_filter(facet, self.effective_rating_filter());

        let mut ignored_conditions = Vec::new();
        if !facet.ai_models.is_empty() {
            ignored_conditions.push(format!(
                "AIモデル: {}",
                facet
                    .ai_models
                    .iter()
                    .cloned()
                    .collect::<Vec<_>>()
                    .join("、")
            ));
        }
        if !facet.ai_tools.is_empty() {
            ignored_conditions.push(format!(
                "生成ツール: {}",
                facet
                    .ai_tools
                    .iter()
                    .cloned()
                    .collect::<Vec<_>>()
                    .join("、")
            ));
        }
        if self.color_filter.enabled {
            ignored_conditions.push(format!(
                "画像色: {}",
                crate::color_search::hex_rgb(self.color_filter.query_rgb)
            ));
        }
        if !facet.place_keys.is_empty() {
            ignored_conditions.push("場所".to_string());
        }
        self.smart_folder_rule_draft = Some(SmartFolderRuleDraft {
            target_id,
            sources,
            include_descendants,
            filter,
            ignored_conditions,
        });
    }

    fn show_smart_folder_create_dialog(&mut self, ctx: &egui::Context) {
        let Some(mut name) = self.smart_folder_create_name.clone() else {
            return;
        };
        let enter_pressed = self.dialog_enter_pressed(ctx);
        let escape_pressed = self.dialog_escape_pressed(ctx);
        let mut open = true;
        let mut confirm = false;
        let mut cancel = false;
        let trimmed = name.trim().to_string();
        let duplicate = self
            .settings
            .smart_folders
            .iter()
            .any(|definition| definition.name.eq_ignore_ascii_case(&trimmed));
        egui::Window::new("新しいスマートフォルダ")
            .open(&mut open)
            .collapsible(false)
            .resizable(false)
            .default_pos(ctx.content_rect().min + egui::vec2(90.0, 70.0))
            .show(ctx, |ui| {
                ui.label("名前:");
                let response = ui.add(
                    egui::TextEdit::singleline(&mut name)
                        .desired_width(360.0)
                        .hint_text("スマートフォルダ名"),
                );
                if duplicate && !trimmed.is_empty() {
                    ui.colored_label(ui.visuals().error_fg_color, "同じ名前が既にあります");
                }
                ui.label(
                    egui::RichText::new(
                        "作成後、一覧のスマートフォルダメニューから現在の表示条件を追加します",
                    )
                    .weak(),
                );
                ui.horizontal(|ui| {
                    let can_confirm = !trimmed.is_empty() && !duplicate;
                    if ui
                        .add_enabled(can_confirm, egui::Button::new("作成"))
                        .clicked()
                        || (can_confirm && response.lost_focus() && enter_pressed)
                    {
                        confirm = true;
                    }
                    if ui.button("キャンセル").clicked() {
                        cancel = true;
                    }
                });
            });
        if confirm {
            let definition = SmartFolderDefinition::new(trimmed);
            self.smart_folder_editor_selected = Some(definition.id);
            self.settings.smart_folders.push(definition);
            self.settings.save();
            self.smart_folder_create_name = None;
        } else if cancel || escape_pressed || !open {
            self.smart_folder_create_name = None;
        } else {
            self.smart_folder_create_name = Some(name);
        }
    }

    fn show_smart_folder_rule_dialog(&mut self, ctx: &egui::Context) {
        let Some(mut draft) = self.smart_folder_rule_draft.clone() else {
            return;
        };
        let Some(target_name) = self
            .settings
            .smart_folders
            .iter()
            .find(|definition| definition.id == draft.target_id)
            .map(|definition| definition.name.clone())
        else {
            self.smart_folder_rule_draft = None;
            return;
        };
        let escape_pressed = self.dialog_escape_pressed(ctx);
        let mut open = true;
        let mut confirm = false;
        let mut cancel = false;
        egui::Window::new("現在のアイテム表示条件を追加")
            .open(&mut open)
            .collapsible(false)
            .resizable(true)
            .default_pos(ctx.content_rect().min + egui::vec2(80.0, 55.0))
            .default_width(620.0)
            .show(ctx, |ui| {
                ui.label(format!("追加先: {target_name}"));
                ui.separator();
                ui.label(egui::RichText::new("検索元").strong());
                for source in &draft.sources {
                    ui.label(source.to_string_lossy())
                        .on_hover_text(source.to_string_lossy());
                }
                ui.checkbox(&mut draft.include_descendants, "サブフォルダを含む")
                    .on_hover_text(
                        "サブ展開中は既定でONです。通常一覧で追加する場合は既定でOFFです。",
                    );
                ui.label(
                    egui::RichText::new(
                        "先に一覧でサブ展開を使うと、サブフォルダを含めた結果を確認できます",
                    )
                    .weak(),
                );
                ui.separator();
                ui.horizontal(|ui| {
                    ui.label("名前に含む:");
                    ui.add(
                        egui::TextEdit::singleline(&mut draft.filter.name_contains)
                            .desired_width(320.0),
                    );
                    ui.label(egui::RichText::new("（任意）").weak());
                });
                ui.label(egui::RichText::new("保存する条件").strong());
                for summary in filter_summary(&draft.filter) {
                    ui.label(format!("・{summary}"));
                }
                if !draft.ignored_conditions.is_empty() {
                    ui.separator();
                    ui.colored_label(
                        ui.visuals().warn_fg_color,
                        "次の条件はファイル内容の確認または現在表示固有のため保存されません:",
                    );
                    for ignored in &draft.ignored_conditions {
                        ui.label(format!("・{ignored}"));
                    }
                }
                ui.separator();
                ui.horizontal(|ui| {
                    if ui.button("追加").clicked() {
                        confirm = true;
                    }
                    if ui.button("キャンセル").clicked() {
                        cancel = true;
                    }
                });
            });
        if confirm {
            if let Some(definition) = self
                .settings
                .smart_folders
                .iter_mut()
                .find(|definition| definition.id == draft.target_id)
            {
                draft.filter.name_contains = draft.filter.name_contains.trim().to_string();
                for source in draft.sources {
                    definition.rules.push(SmartFolderRule::new(
                        source,
                        draft.include_descendants,
                        draft.filter.clone(),
                    ));
                }
                self.settings.save();
                self.invalidate_smart_folder_definition(draft.target_id);
            }
            self.smart_folder_rule_draft = None;
        } else if cancel || escape_pressed || !open {
            self.smart_folder_rule_draft = None;
        } else {
            self.smart_folder_rule_draft = Some(draft);
        }
    }

    pub(crate) fn show_smart_folder_editor_dialog(&mut self, ctx: &egui::Context) {
        self.show_smart_folder_create_dialog(ctx);
        self.show_smart_folder_rule_dialog(ctx);
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

        let mut open = true;
        let mut close_requested = false;
        let mut create_requested = false;
        let mut select_requested = None;
        let mut definition_swap = None;
        let mut delete_requested = None;
        let mut rule_remove = None;
        let mut rule_swap = None;
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
                // 一覧部分だけが可変高を所有する。ScrollArea に高さ上限を渡さないと、
                // 内容高が Window の最小高になり、縦方向へ縮められなくなる。
                let footer_height = 42.0;
                let body_height = (ui.available_height() - footer_height).max(180.0);
                ui.horizontal(|ui| {
                    ui.allocate_ui_with_layout(
                        egui::vec2(220.0, body_height),
                        egui::Layout::top_down(egui::Align::Min),
                        |ui| {
                        ui.set_max_size(egui::vec2(220.0, body_height));
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
                                    selected_index.is_some_and(|index| {
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
                        let list_height = ui.available_height().max(0.0);
                        egui::ScrollArea::vertical()
                            .id_salt("smart_folder_definition_list")
                            .auto_shrink([false, false])
                            .max_height(list_height)
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
                                            "{} 件の表示条件",
                                            definition.rules.len()
                                        ))
                                        .clicked()
                                    {
                                        select_requested = Some(definition.id);
                                    }
                                }
                            });
                        },
                    );

                    ui.separator();
                    let right_size = egui::vec2(ui.available_width(), body_height);
                    ui.allocate_ui_with_layout(
                        right_size,
                        egui::Layout::top_down(egui::Align::Min),
                        |ui| {
                        ui.set_max_size(right_size);
                        if let Some(definition) = draft.as_mut() {
                            ui.horizontal(|ui| {
                                ui.label("名前:");
                                ui.text_edit_singleline(&mut definition.name);
                                if ui
                                    .add_enabled(
                                        !definition.rules.is_empty(),
                                        egui::Button::new("開く"),
                                    )
                                    .on_hover_text(if definition.rules.is_empty() {
                                        "現在の一覧から表示条件を追加してください"
                                    } else {
                                        "スマートフォルダを開きます"
                                    })
                                    .clicked()
                                {
                                    open_definition = Some(definition.id);
                                }
                            });
                            ui.add_space(4.0);
                            ui.label(egui::RichText::new("並び方").strong());
                            ui.horizontal_wrapped(|ui| {
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
                                ui.label(
                                    egui::RichText::new(
                                        "表示形式とソート順は現在の一覧設定を使います",
                                    )
                                    .weak(),
                                );
                            });

                            ui.separator();
                            ui.label(egui::RichText::new("保存した表示条件").strong());
                            ui.label(
                                egui::RichText::new(
                                    "条件はスマートフォルダメニューの「現在のアイテム表示条件を追加」から登録します。複数条件はORで結合されます。",
                                )
                                .weak(),
                            );
                            let rule_list_height = ui.available_height().max(0.0);
                            egui::ScrollArea::vertical()
                                .id_salt("smart_folder_rule_list")
                                .auto_shrink([false, false])
                                .max_height(rule_list_height)
                                .show(ui, |ui| {
                                    if definition.rules.is_empty() {
                                        ui.add_space(20.0);
                                        ui.centered_and_justified(|ui| {
                                            ui.label("まだ表示条件がありません");
                                        });
                                    }
                                    let rule_count = definition.rules.len();
                                    for (index, rule) in definition.rules.iter_mut().enumerate() {
                                        egui::Frame::group(ui.style()).show(ui, |ui| {
                                            ui.horizontal(|ui| {
                                                ui.checkbox(&mut rule.enabled, "有効");
                                                ui.label(
                                                    egui::RichText::new(
                                                        rule.source.to_string_lossy(),
                                                    )
                                                    .strong(),
                                                )
                                                .on_hover_text(rule.source.to_string_lossy());
                                                if ui.small_button("↑").clicked() && index > 0 {
                                                    rule_swap = Some((index, index - 1));
                                                }
                                                if ui.small_button("↓").clicked()
                                                    && index + 1 < rule_count
                                                {
                                                    rule_swap = Some((index, index + 1));
                                                }
                                                if ui.small_button("削除").clicked() {
                                                    rule_remove = Some(index);
                                                }
                                            });
                                            ui.checkbox(
                                                &mut rule.include_descendants,
                                                "サブフォルダを含む",
                                            );
                                            ui.horizontal(|ui| {
                                                ui.label("名前に含む:");
                                                ui.add(
                                                    egui::TextEdit::singleline(
                                                        &mut rule.filter.name_contains,
                                                    )
                                                    .desired_width(240.0),
                                                );
                                            });
                                            ui.label(
                                                egui::RichText::new(rule_summary(rule))
                                                    .small()
                                                    .weak(),
                                            );
                                        });
                                        ui.add_space(4.0);
                                    }
                                });
                        } else {
                            ui.centered_and_justified(|ui| {
                                ui.label("左の一覧から選ぶか、「新規」を押してください");
                            });
                        }
                        },
                    );
                });

                ui.separator();
                ui.horizontal(|ui| {
                    if ui.button("閉じる").clicked() {
                        close_requested = true;
                    }
                    ui.label(
                        egui::RichText::new("開く／更新時に実フォルダを再走査します").weak(),
                    );
                });
            });

        let mut dirty = false;
        if let (Some(index), Some(mut updated)) = (selected_index, draft) {
            if let Some((a, b)) = rule_swap {
                updated.rules.swap(a, b);
            }
            if let Some(index) = rule_remove
                && index < updated.rules.len()
            {
                updated.rules.remove(index);
            }
            updated.name = updated.name.trim().to_string();
            if updated.name.is_empty() {
                updated.name = "スマートフォルダ".to_string();
            }
            for rule in &mut updated.rules {
                rule.filter.name_contains = rule.filter.name_contains.trim().to_string();
            }
            if self.settings.smart_folders.get(index) != Some(&updated) {
                invalidated_definition = Some(updated.id);
                self.settings.smart_folders[index] = updated;
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
    fn rule_summary_distinguishes_recursive_video_rule() {
        let mut filter = SmartFolderFilter::default();
        filter.kinds.insert(FacetItemKind::Video);
        filter.extensions.insert("mp4".into());
        let rule = SmartFolderRule::new(PathBuf::from(r"C:\Videos"), true, filter);
        let summary = rule_summary(&rule);
        assert!(summary.contains("サブフォルダを含む"));
        assert!(summary.contains("動画"));
        assert!(summary.contains(".mp4"));
    }

    #[test]
    fn captured_filter_keeps_only_explicit_kind_conditions() {
        let unrestricted = capture_smart_folder_filter(&Default::default(), [true; 6]);
        assert!(unrestricted.kinds.is_empty());

        let mut facet = crate::settings::FacetFilter::default();
        facet.kinds.insert(FacetItemKind::Video);
        facet.exts.insert("mp4".into());
        let videos = capture_smart_folder_filter(&facet, [true; 6]);
        assert_eq!(videos.kinds, [FacetItemKind::Video].into_iter().collect());
        assert_eq!(videos.extensions, ["mp4".to_string()].into_iter().collect());
    }
}
