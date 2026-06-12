//! 選択中アイテムへ自由記入タグを付ける/外すダイアログ。
//!
//! 「タグの管理」はピン留めタグの語彙管理、このダイアログは現在の選択への
//! 付与/削除操作に限定する。

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;

use eframe::egui;

use crate::app::App;

#[derive(Clone)]
struct TagChoice {
    name: String,
    tag_key: String,
    count: usize,
}

impl App {
    pub(crate) fn open_tag_apply_dialog(&mut self) {
        let paths = self.tag_target_paths();
        if paths.is_empty() {
            self.show_feedback_toast("[タグ対象なし]".to_string());
            return;
        }
        self.hydrate_tags_cache_for_paths(&paths);
        self.tag_apply_input.clear();
        self.show_tag_apply = true;
    }

    pub(crate) fn show_tag_apply_dialog(&mut self, ctx: &egui::Context) {
        if !self.show_tag_apply {
            return;
        }

        let paths = self.tag_target_paths();
        if paths.is_empty() {
            self.show_tag_apply = false;
            return;
        }
        self.hydrate_tags_cache_for_paths(&paths);

        let current_tags = current_selection_tags(self, &paths);
        let current_keys: HashSet<String> = current_tags
            .iter()
            .map(|choice| choice.tag_key.clone())
            .collect();
        let suggestions = tag_suggestions(self, &self.tag_apply_input);
        let normalized_input =
            crate::tags_db::normalize_tag_display_name(self.tag_apply_input.trim());
        let input_len = normalized_input.chars().count();
        let input_valid = !normalized_input.is_empty() && input_len <= 64;
        let input_too_long = input_len > 64;

        let mut open = true;
        let mut close = false;
        let mut add_tag: Option<String> = None;
        let mut remove_tag: Option<String> = None;
        let enter_pressed = self.dialog_enter_pressed(ctx);
        let escape_pressed = self.dialog_escape_pressed(ctx);
        let target_count = paths.len();
        let dialog_pos = ctx.content_rect().min + egui::vec2(70.0, 60.0);

        egui::Window::new("タグを付ける/外す")
            .open(&mut open)
            .resizable(false)
            .collapsible(false)
            .default_pos(dialog_pos)
            .show(ctx, |ui| {
                ui.set_min_width(540.0);
                ui.label(
                    egui::RichText::new(format!("対象: {target_count} 件"))
                        .size(11.0)
                        .weak(),
                );
                ui.add_space(6.0);

                ui.horizontal(|ui| {
                    ui.label("タグ:");
                    ui.add_sized(
                        [280.0, 22.0],
                        egui::TextEdit::singleline(&mut self.tag_apply_input)
                            .hint_text("作品名・作者名など"),
                    );
                    if ui
                        .add_enabled(input_valid, egui::Button::new("付ける"))
                        .clicked()
                    {
                        add_tag = Some(normalized_input.clone());
                    }
                    if ui
                        .add_enabled(input_valid, egui::Button::new("外す"))
                        .clicked()
                    {
                        remove_tag = Some(normalized_input.clone());
                    }
                });
                if input_too_long {
                    ui.label(
                        egui::RichText::new("タグ名は 64 文字以内にしてください。")
                            .size(11.0)
                            .color(egui::Color32::from_rgb(200, 80, 60)),
                    );
                }

                ui.add_space(8.0);
                ui.label(egui::RichText::new("現在の選択タグ").strong());
                ui.add_space(2.0);
                if current_tags.is_empty() {
                    ui.label(egui::RichText::new("（タグなし）").weak());
                } else {
                    ui.horizontal_wrapped(|ui| {
                        for choice in &current_tags {
                            let label = if choice.count == target_count {
                                format!("× #{}", choice.name)
                            } else {
                                format!("× #{} ({}/{target_count})", choice.name, choice.count)
                            };
                            if ui.small_button(label).clicked() {
                                remove_tag = Some(choice.name.clone());
                            }
                        }
                    });
                }

                ui.add_space(10.0);
                let suggestion_title = if normalized_input.is_empty() {
                    "最近使ったタグ"
                } else {
                    "候補"
                };
                ui.label(egui::RichText::new(suggestion_title).strong());
                ui.add_space(2.0);
                if suggestions.is_empty() {
                    ui.label(egui::RichText::new("（候補なし）").weak());
                } else {
                    egui::ScrollArea::vertical()
                        .id_salt("tag_apply_suggestions")
                        .max_height(180.0)
                        .auto_shrink([false, true])
                        .show(ui, |ui| {
                            egui::Grid::new("tag_apply_suggestion_grid")
                                .striped(true)
                                .num_columns(4)
                                .spacing([8.0, 4.0])
                                .show(ui, |ui| {
                                    for choice in &suggestions {
                                        let pinned = current_keys.contains(&choice.tag_key);
                                        let tag = format!("#{}", choice.name);
                                        ui.label(egui::RichText::new(tag).monospace());
                                        if choice.count > 0 {
                                            ui.label(format!("{} 件", choice.count));
                                        } else {
                                            ui.label("");
                                        }
                                        if ui.button("付ける").clicked() {
                                            add_tag = Some(choice.name.clone());
                                        }
                                        if ui
                                            .add_enabled(pinned, egui::Button::new("外す"))
                                            .clicked()
                                        {
                                            remove_tag = Some(choice.name.clone());
                                        }
                                        ui.end_row();
                                    }
                                });
                        });
                }

                ui.add_space(8.0);
                ui.separator();
                ui.add_space(4.0);
                ui.horizontal(|ui| {
                    if ui.button("閉じる").clicked() {
                        close = true;
                    }
                });

                if enter_pressed && input_valid {
                    add_tag = Some(normalized_input.clone());
                }
                if escape_pressed {
                    close = true;
                }
            });

        if let Some(name) = add_tag {
            self.request_tag_add_for_selection(&name);
            self.tag_apply_input.clear();
        }
        if let Some(name) = remove_tag {
            self.request_tag_remove_for_selection(&name);
        }
        if close || !open {
            self.show_tag_apply = false;
            self.tag_apply_input.clear();
        }
    }
}

fn current_selection_tags(app: &App, paths: &[PathBuf]) -> Vec<TagChoice> {
    let mut by_key: HashMap<String, TagChoice> = HashMap::new();
    for path in paths {
        let item_key = crate::tags_db::item_key_for_path(path);
        let Some(tags) = app.tags_cache.get(&item_key) else {
            continue;
        };
        for tag in tags {
            let tag_key = crate::tags_db::normalize_tag_key(tag);
            if tag_key.is_empty() {
                continue;
            }
            let name = tag_display_name(app, &tag_key, tag);
            by_key
                .entry(tag_key.clone())
                .and_modify(|choice| choice.count += 1)
                .or_insert(TagChoice {
                    name,
                    tag_key,
                    count: 1,
                });
        }
    }
    let mut out: Vec<_> = by_key.into_values().collect();
    out.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
    out
}

fn tag_suggestions(app: &App, query: &str) -> Vec<TagChoice> {
    let query_key = crate::tags_db::normalize_tag_key(query);
    let registered: HashMap<String, String> = app
        .settings
        .tags
        .iter()
        .map(|tag| (tag.tag_key.clone(), tag.name.clone()))
        .collect();
    let mut summaries = if let Some(db) = app.tags_db.as_ref() {
        if query_key.is_empty() {
            let mut summaries = db.tag_summaries();
            summaries.sort_by(|a, b| {
                b.last_applied_at
                    .cmp(&a.last_applied_at)
                    .then_with(|| a.tag.to_lowercase().cmp(&b.tag.to_lowercase()))
            });
            summaries
        } else {
            db.find_by_prefix(query, 24)
        }
    } else {
        Vec::new()
    };

    let mut out: Vec<TagChoice> = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();

    let add_registered = |out: &mut Vec<TagChoice>, seen: &mut HashSet<String>| {
        for tag in &app.settings.tags {
            if !query_key.is_empty() && !tag.tag_key.starts_with(&query_key) {
                continue;
            }
            if seen.insert(tag.tag_key.clone()) {
                out.push(TagChoice {
                    name: tag.name.clone(),
                    tag_key: tag.tag_key.clone(),
                    count: 0,
                });
            }
        }
    };
    let add_summaries = |out: &mut Vec<TagChoice>,
                         seen: &mut HashSet<String>,
                         summaries: &mut Vec<crate::tags_db::TagSummary>| {
        for summary in summaries.drain(..) {
            if out.len() >= 24 {
                break;
            }
            if !seen.insert(summary.tag_key.clone()) {
                if let Some(choice) = out.iter_mut().find(|c| c.tag_key == summary.tag_key) {
                    choice.count = summary.count;
                }
                continue;
            }
            let name = registered
                .get(&summary.tag_key)
                .cloned()
                .unwrap_or_else(|| summary.tag.clone());
            out.push(TagChoice {
                name,
                tag_key: summary.tag_key,
                count: summary.count,
            });
        }
    };

    if query_key.is_empty() {
        add_summaries(&mut out, &mut seen, &mut summaries);
        add_registered(&mut out, &mut seen);
    } else {
        add_registered(&mut out, &mut seen);
        add_summaries(&mut out, &mut seen, &mut summaries);
    }

    out.truncate(24);
    out
}

fn tag_display_name(app: &App, tag_key: &str, fallback: &str) -> String {
    app.settings
        .tags
        .iter()
        .find(|tag| tag.tag_key == tag_key)
        .map(|tag| tag.name.clone())
        .unwrap_or_else(|| crate::tags_db::strip_display_hash(fallback).to_string())
}
