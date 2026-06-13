//! 選択中アイテムへ自由記入タグを付ける/外すダイアログ。
//!
//! 「ピン留めタグの管理」はピン留めタグの語彙管理、このダイアログは現在の選択への
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
    pinned: bool,
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
        self.invalidate_tag_apply_suggestions();
        self.show_tag_apply = true;
    }

    pub(crate) fn invalidate_tag_apply_suggestions(&mut self) {
        self.tag_apply_suggestion_key = None;
        self.tag_apply_suggestions.clear();
        self.tag_apply_selection_cache = None;
        // facet メニュー側のキャッシュも同じタイミング (タグデータ変更) で破棄する。
        self.facet_tag_suggestion_cache = None;
        self.facet_tag_counts_cache = None;
    }

    pub(crate) fn show_tag_apply_dialog(&mut self, ctx: &egui::Context) {
        if !self.show_tag_apply {
            return;
        }

        // ダイアログは開いている間毎フレーム描画される。tag_target_paths (選択全件の
        // PathBuf clone) と current_selection_tags (パスごとの item_key 正規化 + タグ
        // ごとの NFKC + ソート) を毎フレーム回すと、5k 件チェック時に入力がもたつく。
        // 選択フィンガープリントが変わるか、タグデータが変わる
        // (invalidate_tag_apply_suggestions) まではキャッシュを使う。
        let fingerprint = (self.checked.len(), self.selected, self.fullscreen_idx);
        let cache_valid = self
            .tag_apply_selection_cache
            .as_ref()
            .is_some_and(|(fp, _, _)| *fp == fingerprint);
        if !cache_valid {
            let paths = self.tag_target_paths();
            self.hydrate_tags_cache_for_paths(&paths);
            let current = current_selection_tags(self, &paths)
                .into_iter()
                .map(|choice| (choice.name, choice.tag_key, choice.count))
                .collect();
            self.tag_apply_selection_cache = Some((fingerprint, paths, current));
        }
        let (_, cached_paths, cached_tags) = self
            .tag_apply_selection_cache
            .as_ref()
            .expect("tag_apply_selection_cache set above");
        if cached_paths.is_empty() {
            self.show_tag_apply = false;
            self.tag_apply_selection_cache = None;
            return;
        }
        let target_count = cached_paths.len();
        let current_tags: Vec<TagChoice> = cached_tags
            .iter()
            .map(|(name, tag_key, count)| TagChoice {
                name: name.clone(),
                tag_key: tag_key.clone(),
                count: *count,
                pinned: false,
            })
            .collect();
        let current_keys: HashSet<String> = current_tags
            .iter()
            .map(|choice| choice.tag_key.clone())
            .collect();

        let mut open = true;
        let mut close = false;
        let mut add_tag: Option<String> = None;
        let mut remove_tag: Option<String> = None;
        let enter_pressed = self.dialog_enter_pressed(ctx);
        let escape_pressed = self.dialog_escape_pressed(ctx);
        let ime_active = self.ime_input_active();
        let dialog_pos = ctx.content_rect().min + egui::vec2(70.0, 60.0);

        egui::Window::new("タグを付ける/外す")
            .open(&mut open)
            .resizable(true)
            .collapsible(false)
            .default_pos(dialog_pos)
            .min_width(580.0)
            .default_size(egui::vec2(640.0, 520.0))
            .show(ctx, |ui| {
                ui.set_min_width(560.0);
                ui.set_min_height(420.0);
                ui.label(
                    egui::RichText::new(format!("対象: {target_count} 件"))
                        .size(11.0)
                        .weak(),
                );
                ui.add_space(6.0);

                ui.horizontal(|ui| {
                    ui.label("タグ:");
                    let input_resp = ui.add_sized(
                        [280.0, 22.0],
                        egui::TextEdit::singleline(&mut self.tag_apply_input)
                            .hint_text("作品名・作者名など"),
                    );
                    if !input_resp.has_focus()
                        && ctx.input(|i| i.focused)
                        && !ui.memory(|m| m.focused().is_some())
                    {
                        input_resp.request_focus();
                    }
                    if ime_active && input_resp.lost_focus() {
                        input_resp.request_focus();
                    }
                    let normalized_input =
                        crate::tags_db::normalize_tag_display_name(self.tag_apply_input.trim());
                    let input_len = normalized_input.chars().count();
                    let input_valid = !normalized_input.is_empty() && input_len <= 64;
                    if input_valid
                        && enter_pressed
                        && (input_resp.has_focus() || input_resp.lost_focus())
                    {
                        add_tag = Some(normalized_input.clone());
                    }
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
                let normalized_input =
                    crate::tags_db::normalize_tag_display_name(self.tag_apply_input.trim());
                let input_len = normalized_input.chars().count();
                let input_too_long = input_len > 64;
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
                let suggestions = self.cached_tag_apply_suggestions(&normalized_input);
                let pinned_choices: Vec<_> = suggestions
                    .iter()
                    .filter(|choice| choice.pinned)
                    .cloned()
                    .collect();
                let recent_choices: Vec<_> = suggestions
                    .iter()
                    .filter(|choice| !choice.pinned)
                    .cloned()
                    .collect();
                let suggestion_title = if normalized_input.is_empty() {
                    "タグ候補"
                } else {
                    "候補"
                };
                ui.label(egui::RichText::new(suggestion_title).strong());
                ui.add_space(2.0);
                let available_for_suggestions = ui.available_height();
                let suggestion_height = if available_for_suggestions.is_finite() {
                    (available_for_suggestions - 52.0).max(280.0)
                } else {
                    360.0
                };
                if normalized_input.is_empty() {
                    egui::ScrollArea::vertical()
                        .id_salt("tag_apply_suggestions")
                        .max_height(suggestion_height)
                        .auto_shrink([false, false])
                        .show(ui, |ui| {
                            ui.columns(2, |columns| {
                                draw_tag_choice_pane(
                                    &mut columns[0],
                                    "ピン留めしたタグ",
                                    "（ピン留めタグなし）",
                                    "tag_apply_pinned_rows",
                                    &pinned_choices,
                                    &current_keys,
                                    &mut add_tag,
                                    &mut remove_tag,
                                );
                                draw_tag_choice_pane(
                                    &mut columns[1],
                                    "最近使ったタグ",
                                    "（最近使ったタグなし）",
                                    "tag_apply_recent_rows",
                                    &recent_choices,
                                    &current_keys,
                                    &mut add_tag,
                                    &mut remove_tag,
                                );
                            });
                        });
                } else if suggestions.is_empty() {
                    ui.label(egui::RichText::new("（候補なし）").weak());
                } else {
                    egui::ScrollArea::vertical()
                        .id_salt("tag_apply_suggestions")
                        .max_height(suggestion_height)
                        .auto_shrink([false, false])
                        .show(ui, |ui| {
                            draw_tag_choice_rows(
                                ui,
                                "tag_apply_suggestion_rows",
                                &suggestions,
                                &current_keys,
                                &mut add_tag,
                                &mut remove_tag,
                            );
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

                if escape_pressed {
                    close = true;
                }
            });

        if let Some(name) = add_tag {
            self.request_tag_add_for_selection(&name);
            self.tag_apply_input.clear();
            self.invalidate_tag_apply_suggestions();
        }
        if let Some(name) = remove_tag {
            self.request_tag_remove_for_selection(&name);
            self.invalidate_tag_apply_suggestions();
        }
        if close || !open {
            self.show_tag_apply = false;
            self.tag_apply_input.clear();
            self.invalidate_tag_apply_suggestions();
        }
    }

    fn cached_tag_apply_suggestions(&mut self, normalized_input: &str) -> Vec<TagChoice> {
        let cache_key = crate::tags_db::normalize_tag_key(normalized_input);
        if self.tag_apply_suggestion_key.as_deref() != Some(cache_key.as_str()) {
            let suggestions = tag_suggestions(self, normalized_input);
            self.tag_apply_suggestions = suggestions
                .iter()
                .map(|choice| {
                    (
                        choice.name.clone(),
                        choice.tag_key.clone(),
                        choice.count,
                        choice.pinned,
                    )
                })
                .collect();
            self.tag_apply_suggestion_key = Some(cache_key);
        }
        self.tag_apply_suggestions
            .iter()
            .map(|(name, tag_key, count, pinned)| TagChoice {
                name: name.clone(),
                tag_key: tag_key.clone(),
                count: *count,
                pinned: *pinned,
            })
            .collect()
    }
}

fn draw_tag_choice_pane(
    ui: &mut egui::Ui,
    title: &str,
    empty_text: &str,
    id_salt: &'static str,
    choices: &[TagChoice],
    current_keys: &HashSet<String>,
    add_tag: &mut Option<String>,
    remove_tag: &mut Option<String>,
) {
    ui.label(egui::RichText::new(title).strong());
    ui.add_space(2.0);
    if choices.is_empty() {
        ui.label(egui::RichText::new(empty_text).weak());
    } else {
        draw_tag_choice_rows(ui, id_salt, choices, current_keys, add_tag, remove_tag);
    }
}

fn draw_tag_choice_rows(
    ui: &mut egui::Ui,
    id_salt: &'static str,
    choices: &[TagChoice],
    current_keys: &HashSet<String>,
    add_tag: &mut Option<String>,
    remove_tag: &mut Option<String>,
) {
    ui.push_id(id_salt, |ui| {
        let row_h = ui.spacing().interact_size.y;
        for choice in choices {
            let assigned = current_keys.contains(&choice.tag_key);
            let tag = format!("#{}", choice.name);
            ui.horizontal(|ui| {
                let spacing = ui.spacing().item_spacing.x * 3.0;
                let label_w = (ui.available_width() - 144.0 - spacing).max(96.0);
                ui.add_sized(
                    [label_w, row_h],
                    egui::Label::new(egui::RichText::new(&tag).monospace()).truncate(),
                )
                .on_hover_text(tag);

                let count_text = if choice.count > 0 {
                    format!("{}件", choice.count)
                } else if choice.pinned {
                    "ピン".to_string()
                } else {
                    String::new()
                };
                ui.add_sized(
                    [36.0, row_h],
                    egui::Label::new(egui::RichText::new(count_text).size(11.0).weak()),
                );

                if ui
                    .add(egui::Button::new("付ける").min_size(egui::vec2(50.0, row_h)))
                    .clicked()
                {
                    *add_tag = Some(choice.name.clone());
                }
                if ui
                    .add_enabled(
                        assigned,
                        egui::Button::new("外す").min_size(egui::vec2(42.0, row_h)),
                    )
                    .clicked()
                {
                    *remove_tag = Some(choice.name.clone());
                }
            });
        }
    });
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
                    pinned: false,
                });
        }
    }
    let mut out: Vec<_> = by_key.into_values().collect();
    out.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
    out
}

fn tag_suggestions(app: &App, query: &str) -> Vec<TagChoice> {
    const QUERY_LIMIT: usize = 24;
    const RECENT_LIMIT: usize = 24;

    let query_key = crate::tags_db::normalize_tag_key(query);
    let registered: HashMap<String, (String, bool)> = app
        .settings
        .tags
        .iter()
        .map(|tag| (tag.tag_key.clone(), (tag.name.clone(), tag.show_shortcut)))
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

    let add_registered =
        |out: &mut Vec<TagChoice>, seen: &mut HashSet<String>, pinned_only: bool| {
            for tag in &app.settings.tags {
                if pinned_only && !tag.show_shortcut {
                    continue;
                }
                if !query_key.is_empty() && !tag.tag_key.starts_with(&query_key) {
                    continue;
                }
                if seen.insert(tag.tag_key.clone()) {
                    out.push(TagChoice {
                        name: tag.name.clone(),
                        tag_key: tag.tag_key.clone(),
                        count: 0,
                        pinned: tag.show_shortcut,
                    });
                }
            }
        };
    let add_summaries = |out: &mut Vec<TagChoice>,
                         seen: &mut HashSet<String>,
                         summaries: &mut Vec<crate::tags_db::TagSummary>,
                         limit: usize| {
        let mut added = 0usize;
        for summary in summaries.drain(..) {
            if added >= limit {
                break;
            }
            if !seen.insert(summary.tag_key.clone()) {
                if let Some(choice) = out.iter_mut().find(|c| c.tag_key == summary.tag_key) {
                    choice.count = summary.count;
                }
                continue;
            }
            let (name, pinned) = registered
                .get(&summary.tag_key)
                .cloned()
                .unwrap_or_else(|| (summary.tag.clone(), false));
            out.push(TagChoice {
                name,
                tag_key: summary.tag_key,
                count: summary.count,
                pinned,
            });
            added += 1;
        }
    };

    if query_key.is_empty() {
        add_registered(&mut out, &mut seen, true);
        add_summaries(&mut out, &mut seen, &mut summaries, RECENT_LIMIT);
    } else {
        add_registered(&mut out, &mut seen, false);
        let remaining = QUERY_LIMIT.saturating_sub(out.len());
        add_summaries(&mut out, &mut seen, &mut summaries, remaining);
        out.truncate(QUERY_LIMIT);
    }

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
