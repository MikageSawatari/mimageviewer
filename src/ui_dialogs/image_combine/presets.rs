use super::*;

#[derive(Default)]
pub(super) struct PresetUi {
    selected: Option<String>,
    editing: Option<String>,
}
#[derive(Clone, Copy)]
pub(super) enum PresetAction {
    Save,
    Delete,
}

pub(super) fn checked_name(name: &str, presets: &[CombinePreset]) -> Result<String, String> {
    let name = name.trim();
    if !(1..=40).contains(&name.chars().count()) {
        return Err("プリセット名は前後の空白を除いて 1〜40 文字で入力してください".into());
    }
    if presets.len() >= 50 && !presets.iter().any(|p| p.name == name) {
        return Err("プリセットは 50 件まで保存できます".into());
    }
    Ok(name.to_owned())
}

impl App {
    pub(super) fn apply_combine_preset(
        &mut self,
        state: &mut CombineDialogState,
        action: PresetAction,
    ) {
        let presets = &mut self.settings.combine_presets;
        match action {
            PresetAction::Save => {
                let Some(editing) = &state.preset.editing else {
                    return;
                };
                let name = match checked_name(editing, presets) {
                    Ok(name) => name,
                    Err(error) => {
                        state.error = Some(error);
                        return;
                    }
                };
                let preset = CombinePreset {
                    name: name.clone(),
                    options: state.options.clone(),
                };
                if let Some(existing) = presets.iter_mut().find(|p| p.name == name) {
                    *existing = preset;
                } else {
                    presets.push(preset);
                }
                state.preset.selected = Some(name);
                state.preset.editing = None;
            }
            PresetAction::Delete => {
                let Some(index) = presets
                    .iter()
                    .position(|p| Some(p.name.as_str()) == state.preset.selected.as_deref())
                else {
                    return;
                };
                presets.remove(index);
                state.preset.selected = None;
            }
        }
        state.error = None;
        if !self.settings.save_checked() {
            crate::logger::log("image combine: cannot persist presets");
            self.show_feedback_error("プリセットを保存できませんでした".into());
        }
    }
}

impl CombineDialogState {
    fn recall_combine_preset(&mut self, preset: &CombinePreset) {
        self.options = preset.options.normalized();
        self.custom_ratio_text = options_ui::custom_ratio_text(&self.options);
        self.split_text = match &self.options.split {
            Split::Auto => String::new(),
            Split::Counts(counts) => counts
                .iter()
                .map(u32::to_string)
                .collect::<Vec<_>>()
                .join("-"),
        };
        self.folder_text = match &self.options.destination {
            Destination::Folder(path) => path.display().to_string(),
            _ => String::new(),
        };
        self.preset.selected = Some(preset.name.clone());
        self.error = None;
    }
}

pub(super) fn draw(
    ui: &mut egui::Ui,
    state: &mut CombineDialogState,
    presets: &[CombinePreset],
    action: &mut Action,
) {
    if let Some(name) = &mut state.preset.editing {
        let mut back = false;
        ui.horizontal_wrapped(|ui| {
            ui.label("プリセット名");
            let response = text(
                ui,
                name,
                "image_combine_preset_name",
                (ui.available_width() - trailing_width(ui, &["上書き保存", "戻る"])).max(1.0),
            );
            let valid = checked_name(name, presets);
            let overwrites = presets.iter().any(|p| p.name == name.trim());
            let save = ui.add_enabled(
                valid.is_ok(),
                egui::Button::new(if overwrites {
                    "上書き保存"
                } else {
                    "保存"
                }),
            );
            if save.clicked() || (valid.is_ok() && response.lost_focus() && action.field_enter) {
                action.preset = Some(PresetAction::Save);
            }
            if let Err(error) = valid {
                save.on_hover_text(error);
            }
            back = ui.button("戻る").clicked();
        });
        if back {
            state.preset.editing = None;
            state.error = None;
        }
        return;
    }
    ui.horizontal_wrapped(|ui| {
        let label = ui.label("プリセット");
        egui::ComboBox::from_id_salt("image_combine_presets")
            .selected_text(state.preset.selected.as_deref().unwrap_or("(選ぶ)"))
            .width((ui.available_width() - trailing_width(ui, &["保存...", "削除"])).max(1.0))
            .truncate()
            .popup_style(crate::os_theme::dark_popup_style(ui.ctx()))
            .show_ui(ui, |ui| {
                for preset in presets {
                    if ui
                        .selectable_label(
                            state.preset.selected.as_deref() == Some(preset.name.as_str()),
                            &preset.name,
                        )
                        .clicked()
                    {
                        state.recall_combine_preset(preset);
                        ui.close();
                    }
                }
            })
            .response
            .labelled_by(label.id);
        if ui.button("保存...").clicked() {
            state.preset.editing = Some(state.preset.selected.clone().unwrap_or_default());
            ui.memory_mut(|memory| {
                memory.request_focus(egui::Id::unique("image_combine_preset_name"));
            });
        }
        if ui
            .add_enabled(state.preset.selected.is_some(), egui::Button::new("削除"))
            .clicked()
        {
            action.preset = Some(PresetAction::Delete);
        }
    });
}

#[cfg(test)]
mod image_combine_tests {
    use super::*;
    #[test]
    fn image_combine_presets_save_replace_recall_delete_and_save_failure() {
        let mut app = crate::app::tests::phase_c_support::setup_app_with_fresh_data_dir();
        let ctx = egui::Context::default();
        let mut state = fixture_state(&ctx, true);
        state.preset.editing = Some(" 商品用 ".into());
        app.apply_combine_preset(&mut state, PresetAction::Save);
        assert_eq!(app.settings.combine_presets.len(), 1);
        assert_eq!(app.settings.combine_presets[0].name, "商品用");
        let db = crate::settings_db::SettingsDb::open(app.tmp.path()).unwrap();
        assert_eq!(
            db.load_into_settings().unwrap().combine_presets,
            app.settings.combine_presets
        );
        state.options.margin = 40;
        state.preset.editing = Some("商品用".into());
        app.apply_combine_preset(&mut state, PresetAction::Save);
        assert_eq!(app.settings.combine_presets.len(), 1);
        assert_eq!(app.settings.combine_presets[0].options.margin, 40);
        let name = state.name.clone();
        let labels = state
            .items
            .iter()
            .map(|item| item.spec.label_override.clone())
            .collect::<Vec<_>>();
        state.options.margin = 0;
        state.recall_combine_preset(&app.settings.combine_presets[0]);
        assert_eq!(state.options.margin, 40);
        assert_eq!(state.name, name);
        assert_eq!(
            state
                .items
                .iter()
                .map(|item| item.spec.label_override.clone())
                .collect::<Vec<_>>(),
            labels
        );
        app.apply_combine_preset(&mut state, PresetAction::Delete);
        assert!(app.settings.combine_presets.is_empty());
        assert!(db.load_into_settings().unwrap().combine_presets.is_empty());
        crate::settings_db::set_save_suppressed(true);
        state.preset.editing = Some("未保存".into());
        app.apply_combine_preset(&mut state, PresetAction::Save);
        assert_eq!(app.settings.combine_presets[0].name, "未保存");
        assert!(
            app.fs_feedback_toast
                .as_ref()
                .unwrap()
                .0
                .contains("プリセットを保存できませんでした")
        );
        crate::settings_db::set_save_suppressed(false);
        assert!(db.load_into_settings().unwrap().combine_presets.is_empty());
    }
}
