//! OS 側でコピー・移動された同一内容ファイルへ、既存の編集内容を複製する確認 UI。

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ContentRestoreUiSource {
    pub path: String,
    pub source_exists: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ContentRestoreUiRow {
    pub file_name: String,
    pub selected: bool,
    pub source_index: usize,
    pub sources: Vec<ContentRestoreUiSource>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ContentRestoreUiAction {
    Restore,
    Close,
}

const RESTORE_FOOTER_RESERVE: f32 = 64.0;
const RESTORE_LIST_FALLBACK_MAX_HEIGHT: f32 = 360.0;

pub fn set_all_restore_rows_selected(rows: &mut [ContentRestoreUiRow], selected: bool) {
    rows.iter_mut().for_each(|row| row.selected = selected);
}

fn relation_label(source_exists: bool) -> &'static str {
    if source_exists {
        "コピー元は残っています"
    } else {
        "移動"
    }
}

fn source_option_text(source: &ContentRestoreUiSource) -> String {
    format!("{} ({})", source.path, relation_label(source.source_exists))
}

fn single_source_row_text(row: &ContentRestoreUiRow) -> Option<String> {
    let [source] = row.sources.as_slice() else {
        return None;
    };
    Some(format!("← {}", source_option_text(source)))
}

fn multiple_source_warning_text(rows: &[ContentRestoreUiRow]) -> Option<String> {
    let count = rows.iter().filter(|row| row.sources.len() > 1).count();
    (count > 0)
        .then(|| format!("{count} 件、複数のコピー元があります。コピー元を選択してください。"))
}

pub(crate) fn resolve_content_restore_prompt_action(
    escape_pressed: bool,
    action: Option<ContentRestoreUiAction>,
) -> Option<crate::app::content_identity_restore::ContentRestorePromptAction> {
    let action = if escape_pressed {
        Some(ContentRestoreUiAction::Close)
    } else {
        action
    }?;
    Some(match action {
        ContentRestoreUiAction::Restore => {
            crate::app::content_identity_restore::ContentRestorePromptAction::Restore
        }
        ContentRestoreUiAction::Close => {
            crate::app::content_identity_restore::ContentRestorePromptAction::Close
        }
    })
}

pub fn render_content_restore_contents(
    ui: &mut egui::Ui,
    rows: &mut [ContentRestoreUiRow],
    dont_ask_again: &mut bool,
) -> Option<ContentRestoreUiAction> {
    render_content_restore_contents_with_list_height(ui, rows, dont_ask_again).0
}

pub fn render_content_restore_modal(
    ctx: &egui::Context,
    rows: &mut [ContentRestoreUiRow],
    dont_ask_again: &mut bool,
) -> Option<ContentRestoreUiAction> {
    let modal_width = (ctx.content_rect().width() - 80.0).clamp(420.0, 780.0);
    egui::Modal::new(egui::Id::new("content_restore_modal"))
        .show(ctx, |ui| {
            ui.set_min_width(modal_width);
            ui.heading("編集内容の復元");
            ui.add_space(8.0);
            render_content_restore_contents(ui, rows, dont_ask_again)
        })
        .inner
}

fn render_content_restore_contents_with_list_height(
    ui: &mut egui::Ui,
    rows: &mut [ContentRestoreUiRow],
    dont_ask_again: &mut bool,
) -> (Option<ContentRestoreUiAction>, f32) {
    ui.label(format!(
        "このフォルダに、以前編集したファイルと内容が同じファイルが {} 件あります。",
        rows.len()
    ));
    ui.label("編集内容 (補正・消しゴム・モザイク・注釈・トリミング・★・タグ) を複製しますか?");
    if let Some(warning) = multiple_source_warning_text(rows) {
        ui.label(egui::RichText::new(warning).color(ui.visuals().warn_fg_color));
    }
    ui.add_space(8.0);
    ui.horizontal(|ui| {
        if ui.button("すべて選ぶ").clicked() {
            set_all_restore_rows_selected(rows, true);
        }
        if ui.button("すべて解除").clicked() {
            set_all_restore_rows_selected(rows, false);
        }
    });
    ui.add_space(4.0);

    let available_height = ui.available_height();
    let list_max_height = if available_height.is_finite() {
        (available_height - RESTORE_FOOTER_RESERVE).max(ui.spacing().interact_size.y)
    } else {
        RESTORE_LIST_FALLBACK_MAX_HEIGHT
    };
    let list_top = ui.cursor().top();
    egui::ScrollArea::vertical()
        .id_salt("content_restore_candidate_list")
        .max_height(list_max_height)
        .auto_shrink([false, true])
        .show(ui, |ui| render_restore_rows(ui, rows));
    let list_height = (ui.cursor().top() - list_top).max(0.0);

    ui.add_space(6.0);
    ui.checkbox(dont_ask_again, "次から確認しない (環境設定で元に戻せます)");
    ui.add_space(8.0);
    let mut action = None;
    ui.allocate_ui_with_layout(
        egui::vec2(ui.available_width(), ui.spacing().interact_size.y),
        egui::Layout::right_to_left(egui::Align::Center),
        |ui| {
            if ui.button("閉じる").clicked() {
                action = Some(ContentRestoreUiAction::Close);
            }
            if ui.button("復元する").clicked() {
                action = Some(ContentRestoreUiAction::Restore);
            }
        },
    );
    (action, list_height)
}

fn render_restore_rows(ui: &mut egui::Ui, rows: &mut [ContentRestoreUiRow]) {
    for (row_index, row) in rows.iter_mut().enumerate() {
        row.source_index = row.source_index.min(row.sources.len().saturating_sub(1));
        let Some(source) = row.sources.get(row.source_index) else {
            continue;
        };
        let selected_source_text = source_option_text(source);
        let source_count = row.sources.len();
        let single_source_text = single_source_row_text(row);
        ui.horizontal_wrapped(|ui| {
            ui.checkbox(&mut row.selected, &row.file_name);
            if let Some(single_source_text) = single_source_text {
                // 単一候補は大多数を占めるため、A3b からの行表示を変えない。
                ui.label(single_source_text);
            } else {
                ui.label(format!("← コピー元を選択 ({source_count} 件)"));
                egui::ComboBox::from_id_salt(("content_restore_source", row_index))
                    .selected_text(selected_source_text)
                    .show_ui(ui, |ui| {
                        for (source_index, source) in row.sources.iter().enumerate() {
                            ui.selectable_value(
                                &mut row.source_index,
                                source_index,
                                source_option_text(source),
                            );
                        }
                    });
            }
        });
        ui.add_space(3.0);
    }
}

impl crate::app::App {
    pub(crate) fn show_content_restore_dialog(&mut self, ctx: &egui::Context) {
        if !self.content_restore_window_visible() {
            return;
        }
        // Modal::should_close は Escape を直接消費するため使わない。IME 変換中の Escape を
        // 奪わない共通 helper の結果を、mutable prompt borrow より先に確定する。
        let escape_pressed = self.dialog_escape_pressed(ctx);
        let prompt = self
            .content_restore_prompt
            .as_mut()
            .expect("visibility predicate checked restore prompt");
        let action =
            render_content_restore_modal(ctx, &mut prompt.ui_rows, &mut prompt.dont_ask_again);
        if let Some(action) = resolve_content_restore_prompt_action(escape_pressed, action) {
            self.handle_content_restore_prompt_action(ctx, action);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rows() -> Vec<ContentRestoreUiRow> {
        [true, false, true]
            .into_iter()
            .enumerate()
            .map(|(index, selected)| ContentRestoreUiRow {
                file_name: format!("{index}.png"),
                selected,
                source_index: 0,
                sources: vec![ContentRestoreUiSource {
                    path: format!("C:/source/{index}.png"),
                    source_exists: true,
                }],
            })
            .collect()
    }

    fn measured_list_height(row_count: usize, screen_height: f32) -> f32 {
        let ctx = egui::Context::default();
        let mut rows = (0..row_count)
            .map(|index| ContentRestoreUiRow {
                file_name: format!("file-{index}.png"),
                selected: true,
                source_index: 0,
                sources: vec![ContentRestoreUiSource {
                    path: format!("C:/source/file-{index}.png"),
                    source_exists: true,
                }],
            })
            .collect::<Vec<_>>();
        let mut dont_ask_again = false;
        let mut height = 0.0;
        for _ in 0..3 {
            let _ = ctx.run(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(860.0, screen_height),
                    )),
                    ..egui::RawInput::default()
                },
                |ctx| {
                    egui::CentralPanel::default().show(ctx, |ui| {
                        ui.set_width(820.0);
                        height = render_content_restore_contents_with_list_height(
                            ui,
                            &mut rows,
                            &mut dont_ask_again,
                        )
                        .1;
                    });
                },
            );
        }
        height
    }

    fn row_with_source_count(source_count: usize) -> ContentRestoreUiRow {
        ContentRestoreUiRow {
            file_name: "target.png".to_string(),
            selected: true,
            source_index: 0,
            sources: (0..source_count)
                .map(|index| ContentRestoreUiSource {
                    path: format!("C:/source/{index}.png"),
                    source_exists: index % 2 == 0,
                })
                .collect(),
        }
    }

    #[test]
    fn select_all_and_clear_all_cover_every_row() {
        let mut rows = rows();
        set_all_restore_rows_selected(&mut rows, true);
        assert!(rows.iter().all(|row| row.selected));

        set_all_restore_rows_selected(&mut rows, false);
        assert!(rows.iter().all(|row| !row.selected));
    }

    #[test]
    fn multiple_source_warning_is_hidden_for_the_single_source_case() {
        assert_eq!(multiple_source_warning_text(&rows()), None);
    }

    #[test]
    fn multiple_source_warning_counts_rows_instead_of_source_candidates() {
        assert_eq!(
            multiple_source_warning_text(&[
                row_with_source_count(1),
                row_with_source_count(4),
                row_with_source_count(1),
            ]),
            Some("1 件、複数のコピー元があります。コピー元を選択してください。".to_string())
        );
        assert_eq!(
            multiple_source_warning_text(&[
                row_with_source_count(2),
                row_with_source_count(5),
                row_with_source_count(3),
                row_with_source_count(1),
            ]),
            Some("3 件、複数のコピー元があります。コピー元を選択してください。".to_string())
        );
    }

    #[test]
    fn single_source_row_keeps_the_existing_text_and_has_no_selector_presentation() {
        let row = row_with_source_count(1);
        assert_eq!(
            single_source_row_text(&row).as_deref(),
            Some("← C:/source/0.png (コピー元は残っています)")
        );
        assert!(single_source_row_text(&row_with_source_count(2)).is_none());
    }

    #[test]
    fn escape_maps_to_close_and_overrides_a_restore_click() {
        assert_eq!(
            resolve_content_restore_prompt_action(true, None),
            Some(crate::app::content_identity_restore::ContentRestorePromptAction::Close)
        );
        assert_eq!(
            resolve_content_restore_prompt_action(true, Some(ContentRestoreUiAction::Restore)),
            Some(crate::app::content_identity_restore::ContentRestorePromptAction::Close)
        );
    }

    #[test]
    fn list_shrinks_to_short_content_and_grows_with_available_height() {
        let one = measured_list_height(1, 520.0);
        let three = measured_list_height(3, 520.0);
        let three_hundred = measured_list_height(300, 520.0);
        let three_hundred_tall = measured_list_height(300, 760.0);
        eprintln!(
            "content restore list heights: one={one}, three={three}, three_hundred={three_hundred}, three_hundred_tall={three_hundred_tall}"
        );

        assert!(
            one < 40.0,
            "1 row should not leave a fixed-height void: {one}"
        );
        assert!(
            three > one * 2.5 && three < 100.0,
            "3 rows should track their content height: {three}"
        );
        assert!(
            three_hundred > 300.0,
            "large sets should use the available window height: {three_hundred}"
        );
        assert!(
            three_hundred_tall > three_hundred + 150.0,
            "enlarging the window should enlarge the viewport: {three_hundred} -> {three_hundred_tall}"
        );
    }
}
