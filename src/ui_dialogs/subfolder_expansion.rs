//! サブ展開を開始する前に、対象・走査階層・走査時フィルタを確認するモーダル。

use eframe::egui;

use crate::app::{
    App, SUBFOLDER_EXPANSION_FILTER_KINDS, SubfolderExpansionDepthChoice,
    SubfolderExpansionScanFilter,
};
use crate::settings::{
    FacetCalendarDate, FacetDatePreset, FacetSizePreset, FacetSizeUnit, FacetSizeValue,
};

fn draw_calendar_date_row(ui: &mut egui::Ui, label: &str, date: &mut Option<FacetCalendarDate>) {
    let mut enabled = date.is_some();
    let mut value = date.unwrap_or_else(FacetCalendarDate::today_local);
    ui.horizontal(|ui| {
        ui.checkbox(&mut enabled, label);
        ui.add_enabled_ui(enabled, |ui| {
            ui.add(
                egui::DragValue::new(&mut value.year)
                    .range(1970..=9999)
                    .suffix("年"),
            );
            ui.add(
                egui::DragValue::new(&mut value.month)
                    .range(1..=12)
                    .suffix("月"),
            );
            ui.add(
                egui::DragValue::new(&mut value.day)
                    .range(1..=31)
                    .suffix("日"),
            );
        });
    });
    value.sanitize();
    *date = enabled.then_some(value);
}

fn draw_kind_filter(ui: &mut egui::Ui, filter: &mut SubfolderExpansionScanFilter) {
    ui.horizontal_wrapped(|ui| {
        ui.label("種類:");
        if ui
            .selectable_label(filter.kinds.is_empty(), "すべて")
            .clicked()
        {
            filter.kinds.clear();
        }
        for kind in SUBFOLDER_EXPANSION_FILTER_KINDS {
            let mut selected = filter.kinds.contains(&kind);
            if ui.checkbox(&mut selected, kind.label()).changed() {
                if selected {
                    filter.kinds.insert(kind);
                } else {
                    filter.kinds.remove(&kind);
                }
            }
        }
    });
}

/// サイズ範囲の 1 行は「絞り込み」バーと同じ描画関数を使う。
/// 見た目と文言が 2 箇所に分かれていると、片方だけ直して食い違う。
use crate::ui_main::draw_facet_size_value_row as draw_size_value_row;

fn draw_size_filter(ui: &mut egui::Ui, filter: &mut SubfolderExpansionScanFilter) {
    ui.horizontal(|ui| {
        ui.label("サイズ:");
        egui::ComboBox::from_id_salt("subfolder_expansion_dialog_size")
            .selected_text(
                filter
                    .size_preset
                    .map_or_else(|| "すべて".to_string(), |preset| preset.label().to_string()),
            )
            .show_ui(ui, |ui| {
                ui.selectable_value(&mut filter.size_preset, None, "すべて");
                for &preset in FacetSizePreset::all() {
                    ui.selectable_value(&mut filter.size_preset, Some(preset), preset.label());
                }
                ui.separator();
                let range_selected =
                    matches!(filter.size_preset, Some(FacetSizePreset::Range { .. }));
                if ui.selectable_label(range_selected, "範囲指定").clicked() && !range_selected
                {
                    // 両端ともチェックを外した状態から始める (「絞り込み」バーと同じ)。
                    filter.size_preset = Some(FacetSizePreset::Range {
                        min: None,
                        max: None,
                    });
                }
            });
        ui.label(egui::RichText::new("ファイルのみ").weak());
    });
    if let Some(FacetSizePreset::Range { mut min, mut max }) = filter.size_preset {
        ui.indent("subfolder_expansion_size_range", |ui| {
            draw_size_value_row(
                ui,
                "下限",
                "以上",
                "subexpand_min",
                &mut min,
                FacetSizeValue::new(100, FacetSizeUnit::KB),
            );
            draw_size_value_row(
                ui,
                "上限",
                "未満",
                "subexpand_max",
                &mut max,
                FacetSizeValue::new(1, FacetSizeUnit::MB),
            );
        });
        filter.size_preset = Some(FacetSizePreset::Range { min, max }.sanitized());
    }
}

fn draw_date_filter(ui: &mut egui::Ui, filter: &mut SubfolderExpansionScanFilter) {
    ui.horizontal(|ui| {
        ui.label("日付:");
        egui::ComboBox::from_id_salt("subfolder_expansion_dialog_date")
            .selected_text(
                filter
                    .date_preset
                    .map_or_else(|| "すべて".to_string(), FacetDatePreset::label),
            )
            .show_ui(ui, |ui| {
                ui.selectable_value(&mut filter.date_preset, None, "すべて");
                for &preset in FacetDatePreset::all() {
                    ui.selectable_value(&mut filter.date_preset, Some(preset), preset.label());
                }
                ui.separator();
                let custom_selected =
                    matches!(filter.date_preset, Some(FacetDatePreset::CustomDays(_)));
                if ui.selectable_label(custom_selected, "日数を指定").clicked() {
                    let days = match filter.date_preset {
                        Some(FacetDatePreset::CustomDays(days)) => days,
                        _ => 30,
                    };
                    filter.date_preset = Some(FacetDatePreset::CustomDays(days));
                }
                let range_selected =
                    matches!(filter.date_preset, Some(FacetDatePreset::Range { .. }));
                if ui.selectable_label(range_selected, "期間を指定").clicked() {
                    let today = FacetCalendarDate::today_local();
                    filter.date_preset = Some(FacetDatePreset::Range {
                        start: Some(today),
                        end: Some(today),
                    });
                }
            });
        ui.label(egui::RichText::new("ファイルのみ").weak());
    });

    match filter.date_preset {
        Some(FacetDatePreset::CustomDays(mut days)) => {
            ui.indent("subfolder_expansion_custom_days", |ui| {
                if ui
                    .add(
                        egui::DragValue::new(&mut days)
                            .range(1..=36_500)
                            .suffix(" 日以内"),
                    )
                    .changed()
                {
                    filter.date_preset = Some(FacetDatePreset::CustomDays(days));
                }
            });
        }
        Some(FacetDatePreset::Range { mut start, mut end }) => {
            ui.indent("subfolder_expansion_date_range", |ui| {
                draw_calendar_date_row(ui, "開始", &mut start);
                draw_calendar_date_row(ui, "終了", &mut end);
            });
            filter.date_preset = Some(FacetDatePreset::Range { start, end }.sanitized());
        }
        _ => {}
    }
}

impl App {
    pub(crate) fn show_subfolder_expansion_dialog_window(&mut self, ctx: &egui::Context) {
        if !self.show_subfolder_expansion_dialog {
            return;
        }

        let roots = self.selected_subfolder_expansion_roots();
        let target_description = if roots.is_empty() {
            match self.current_folder.as_ref() {
                Some(folder) => format!("現在のフォルダ全体を展開します:\n{}", folder.display()),
                None => "現在のフォルダ全体を展開します。".to_string(),
            }
        } else {
            format!(
                "チェックした {} 個のフォルダだけを展開します。",
                roots.len()
            )
        };

        let previous_depth = SubfolderExpansionDepthChoice::from_setting(
            self.settings.subfolder_expansion_max_depth,
        );
        let previous_filter = SubfolderExpansionScanFilter::from_settings(&self.settings);
        let mut selected_depth = previous_depth;
        let mut selected_filter = previous_filter.clone();
        let mut execute = false;
        let mut cancel = false;
        let enter_pressed = self.dialog_enter_pressed(ctx);

        let response =
            egui::Modal::new(egui::Id::new("subfolder_expansion_dialog")).show(ctx, |ui| {
                ui.set_min_width(460.0);
                ui.heading("サブ展開");
                ui.add_space(8.0);
                ui.label(target_description);
                ui.add_space(10.0);

                ui.horizontal(|ui| {
                    ui.label("走査する階層:");
                    egui::ComboBox::from_id_salt("subfolder_expansion_dialog_depth")
                        .selected_text(selected_depth.label())
                        .show_ui(ui, |ui| {
                            for choice in SubfolderExpansionDepthChoice::ALL {
                                ui.selectable_value(&mut selected_depth, choice, choice.label());
                            }
                        });
                });

                ui.add_space(8.0);
                ui.label(egui::RichText::new("走査時の絞り込み").strong());
                ui.label(
                    egui::RichText::new(
                        "条件に合わない項目は走査結果へ集めず、一覧にも表示しません。",
                    )
                    .weak(),
                );
                ui.add_space(4.0);
                draw_kind_filter(ui, &mut selected_filter);
                draw_size_filter(ui, &mut selected_filter);
                draw_date_filter(ui, &mut selected_filter);

                ui.add_space(8.0);
                ui.separator();
                ui.add_space(4.0);
                ui.horizontal(|ui| {
                    if ui.button("サブ展開").clicked() {
                        execute = true;
                    }
                    if ui.button("キャンセル").clicked() {
                        cancel = true;
                    }
                });

                if enter_pressed {
                    execute = true;
                }
            });

        if selected_depth != previous_depth || selected_filter != previous_filter {
            self.settings.subfolder_expansion_max_depth = selected_depth.setting_value();
            selected_filter.apply_to_settings(&mut self.settings);
            self.settings.save();
        }

        if cancel || response.should_close() {
            self.show_subfolder_expansion_dialog = false;
        } else if execute {
            self.show_subfolder_expansion_dialog = false;
            self.toggle_subfolder_expansion_view();
        }
    }
}
