use super::*;

const MAX_PRESETS: usize = 50;

fn name_field_id() -> egui::Id {
    egui::Id::unique("batch_export_preset_name")
}

/// Reserve the actual trailing controls before giving an editor its width.
fn trailing_buttons_width(ui: &egui::Ui, labels: &[&str]) -> f32 {
    labels
        .iter()
        .map(|label| {
            let text = egui::WidgetText::from(*label).into_galley(
                ui,
                Some(egui::TextWrapMode::Extend),
                f32::INFINITY,
                egui::TextStyle::Button,
            );
            (text.size().x + 2.0 * ui.spacing().button_padding.x).max(ui.spacing().interact_size.x)
                + ui.spacing().item_spacing.x
        })
        .sum()
}

#[derive(Default)]
pub(super) struct PresetUi {
    // Selection only identifies the delete target, never a live options binding.
    pub(super) selected: Option<String>,
    pub(super) editing: Option<String>,
}

#[derive(Clone, Copy)]
pub(super) enum PresetAction {
    Save,
    Delete,
}

fn preset_name(name: &str, presets: &[BatchExportPreset]) -> Result<String, String> {
    let name = name.trim();
    if !(1..=40).contains(&name.chars().count()) {
        return Err("プリセット名は前後の空白を除いて 1〜40 文字で入力してください".into());
    }
    if presets.len() >= MAX_PRESETS && !presets.iter().any(|preset| preset.name == name) {
        return Err("プリセットは 50 件まで保存できます".into());
    }
    Ok(name.to_owned())
}

impl ExportBatchDialogState {
    pub(super) fn recall_preset(&mut self, preset: &BatchExportPreset, ctx: &egui::Context) {
        self.options = preset.options.clone();
        self.preset.selected = Some(preset.name.clone());
        self.error = None;
        // A recall, even of the same path, owns a fresh receiver (design §7.4).
        self.begin_logo_check(ctx);
    }

    pub(super) fn edit_presets(
        &mut self,
        presets: &mut Vec<BatchExportPreset>,
        action: PresetAction,
    ) -> Result<bool, String> {
        match action {
            PresetAction::Save => {
                let Some(editing) = &self.preset.editing else {
                    return Ok(false);
                };
                let name = preset_name(editing, presets)?;
                let preset = BatchExportPreset {
                    name: name.clone(),
                    options: self.options.clone(),
                };
                if let Some(existing) = presets.iter_mut().find(|preset| preset.name == name) {
                    *existing = preset;
                } else {
                    presets.push(preset);
                }
                self.preset.selected = Some(name);
                self.preset.editing = None;
            }
            PresetAction::Delete => {
                let Some(index) = presets.iter().position(|preset| {
                    self.preset.selected.as_deref() == Some(preset.name.as_str())
                }) else {
                    return Ok(false);
                };
                presets.remove(index);
                self.preset.selected = None;
            }
        }
        self.error = None;
        Ok(true)
    }
}

impl App {
    pub(super) fn apply_batch_preset_action(
        &mut self,
        state: &mut ExportBatchDialogState,
        action: PresetAction,
    ) {
        match state.edit_presets(&mut self.settings.export_batch_presets, action) {
            Ok(true) => {
                // Explicit settings saves share SETTINGS_WRITE_GATE with other writers.
                if !self.settings.save_checked() {
                    crate::logger::log("batch export: cannot persist presets");
                    self.show_feedback_error("プリセットを保存できませんでした".into());
                }
            }
            Ok(false) => {}
            Err(error) => state.error = Some(error),
        }
    }
}

pub(super) fn draw_presets(
    ui: &mut egui::Ui,
    state: &mut ExportBatchDialogState,
    presets: &[BatchExportPreset],
    owner: &mut EnterOwner,
    action: &mut BatchDialogAction,
) {
    if state.preset.editing.is_some() {
        draw_preset_name(ui, state, presets, owner, action);
        return;
    }
    ui.horizontal(|ui| {
        let label = ui.label("プリセット");
        let width =
            (ui.available_width() - trailing_buttons_width(ui, &["保存...", "削除"])).max(1.0);
        // ComboBox::width is a minimum, so also constrain its parent: a 40-character
        // name must truncate inside this slot rather than push the buttons out.
        let combo = ui
            .allocate_ui_with_layout(
                egui::vec2(width, ui.spacing().interact_size.y),
                egui::Layout::left_to_right(egui::Align::Center),
                |ui| {
                    egui::ComboBox::from_id_salt("batch_export_presets")
                        .selected_text(state.preset.selected.as_deref().unwrap_or("(選ぶ)"))
                        .width(width)
                        .truncate()
                        .popup_style(crate::os_theme::dark_popup_style(ui.ctx()))
                        .show_ui(ui, |ui| {
                            for preset in presets {
                                let response = ui.selectable_label(
                                    state.preset.selected.as_deref() == Some(preset.name.as_str()),
                                    &preset.name,
                                );
                                if response.clicked() {
                                    state.recall_preset(preset, ui.ctx());
                                    ui.close();
                                }
                                owner.widget(&response);
                            }
                        })
                },
            )
            .inner;
        owner.widget(&combo.response.labelled_by(label.id));
        let response = ui.button("保存...");
        if response.clicked() {
            state.preset.editing = Some(state.preset.selected.clone().unwrap_or_default());
            ui.memory_mut(|memory| memory.request_focus(name_field_id()));
        }
        owner.widget(&response);
        let selected = presets
            .iter()
            .any(|preset| state.preset.selected.as_deref() == Some(preset.name.as_str()));
        let response = ui.add_enabled(selected, egui::Button::new("削除"));
        if response.clicked() {
            action.preset = Some(PresetAction::Delete);
        }
        owner.widget(&response);
    });
}

fn draw_preset_name(
    ui: &mut egui::Ui,
    state: &mut ExportBatchDialogState,
    presets: &[BatchExportPreset],
    owner: &mut EnterOwner,
    action: &mut BatchDialogAction,
) {
    let mut back = false;
    ui.horizontal(|ui| {
        let label = ui.label("プリセット名");
        let name = state.preset.editing.as_mut().expect("preset name editor");
        let width =
            (ui.available_width() - trailing_buttons_width(ui, &["上書き保存", "戻る"])).max(1.0);
        let mut output = crate::ime_focus::show_singleline(ui, name, None, |edit| {
            edit.id(name_field_id())
                .desired_width(width)
                .hint_text("1〜40 文字")
        });
        output.response.response = output.response.response.labelled_by(label.id);
        crate::ui_helpers::singleline_text_edit_context_menu(ui, &mut output, name);
        owner.text(&output.response);
        owner.preset_submitted |=
            owner.before == Some(output.response.id) && output.response.lost_focus();
        let overwrites = presets.iter().any(|preset| preset.name == name.trim());
        let validation = preset_name(name, presets);
        let response = ui.add_enabled(
            validation.is_ok(),
            egui::Button::new(if overwrites {
                "上書き保存"
            } else {
                "保存"
            }),
        );
        if response.clicked() {
            action.preset = Some(PresetAction::Save);
        }
        if let Err(error) = validation {
            response.clone().on_hover_text(error);
        }
        owner.widget(&response);
        let response = ui.button("戻る");
        back = response.clicked();
        owner.widget(&response);
    });
    if back {
        state.preset.editing = None;
    }
}
