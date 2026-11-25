use super::*;
use std::{path::PathBuf, sync::mpsc};

pub(super) struct LogoCheck {
    pub(super) path: Option<PathBuf>,
    pub(super) result: LogoCheckResult,
}

pub(super) enum LogoCheckResult {
    Pending(mpsc::Receiver<bool>),
    Found,
    Missing,
    Unsupported,
}

impl ExportBatchDialogState {
    fn image_watermark_active(&self) -> bool {
        self.options.watermark.enabled && self.options.watermark.kind == WatermarkKind::Image
    }

    pub(super) fn begin_logo_check(&mut self, ctx: &egui::Context) {
        if !self.image_watermark_active() {
            self.logo_check = None;
            return;
        }
        let path = self.options.watermark.image_path.clone();
        let result = match path.as_deref() {
            None => LogoCheckResult::Missing,
            Some(path) if !supported_logo_path(path) => LogoCheckResult::Unsupported,
            Some(path) => {
                let (tx, rx) = mpsc::channel();
                let path = path.to_owned();
                let ctx = ctx.clone();
                match std::thread::Builder::new()
                    .name("export-logo-check".into())
                    .spawn(move || {
                        let found =
                            std::fs::metadata(path).is_ok_and(|metadata| metadata.is_file());
                        let _ = tx.send(found);
                        ctx.request_repaint();
                    }) {
                    Ok(_) => LogoCheckResult::Pending(rx),
                    Err(error) => {
                        crate::logger::log(format!("cannot start export logo check: {error}"));
                        LogoCheckResult::Missing
                    }
                }
            }
        };
        // Replacing the receiver also rejects results from an earlier A -> B -> A.
        self.logo_check = Some(LogoCheck { path, result });
    }

    pub(super) fn poll_logo_check(&mut self, ctx: &egui::Context) {
        if !self.image_watermark_active() {
            self.logo_check = None;
            return;
        }
        if self.logo_check.is_none() {
            self.begin_logo_check(ctx);
        }
        if let Some(LogoCheck { result, .. }) = &mut self.logo_check {
            if let LogoCheckResult::Pending(rx) = result {
                match rx.try_recv() {
                    Ok(true) => *result = LogoCheckResult::Found,
                    Ok(false) | Err(mpsc::TryRecvError::Disconnected) => {
                        *result = LogoCheckResult::Missing
                    }
                    Err(mpsc::TryRecvError::Empty) => {}
                }
            }
        }
    }

    pub(super) fn logo_error(&self) -> Option<String> {
        if !self.image_watermark_active() {
            return None;
        }
        let path = self.options.watermark.image_path.as_deref();
        if path.is_some_and(|path| !supported_logo_path(path)) {
            return Some("この形式は透かしに使えません".into());
        }
        let check = self.logo_check.as_ref()?;
        if check.path.as_deref() != path {
            return None;
        }
        match check.result {
            LogoCheckResult::Missing => Some(format!(
                "透かしの画像が見つかりません: {}",
                path.unwrap_or(std::path::Path::new("")).display()
            )),
            LogoCheckResult::Unsupported => Some("この形式は透かしに使えません".into()),
            _ => None,
        }
    }
}

pub(super) fn draw_watermark(
    ui: &mut egui::Ui,
    state: &mut ExportBatchDialogState,
    owner: &mut EnterOwner,
    action: &mut BatchDialogAction,
) {
    ui.separator();
    ui.strong("透かし");
    owner.widget(&ui.checkbox(&mut state.options.watermark.enabled, "透かしを入れる"));
    if !state.options.watermark.enabled {
        return;
    }
    let old_kind = state.options.watermark.kind;
    ui.horizontal(|ui| {
        owner.widget(&ui.radio_value(
            &mut state.options.watermark.kind,
            WatermarkKind::Text,
            "文字",
        ));
        owner.widget(&ui.radio_value(
            &mut state.options.watermark.kind,
            WatermarkKind::Image,
            "画像",
        ));
    });
    if old_kind != state.options.watermark.kind {
        state.begin_logo_check(ui.ctx());
    }
    match state.options.watermark.kind {
        WatermarkKind::Text => draw_text_watermark(ui, &mut state.options.watermark, owner),
        WatermarkKind::Image => draw_image_watermark(ui, state, owner, action),
    }
    draw_watermark_placement(ui, &mut state.options.watermark, owner);
}

fn percent_control(
    ui: &mut egui::Ui,
    label: &str,
    value: &mut u32,
    range: std::ops::RangeInclusive<u32>,
    owner: &mut EnterOwner,
) {
    ui.horizontal(|ui| {
        let label = ui.label(label);
        owner.widget(
            &ui.add(egui::DragValue::new(value).range(range).suffix("%"))
                .labelled_by(label.id),
        );
    });
}

fn draw_text_watermark(ui: &mut egui::Ui, options: &mut BatchWatermark, owner: &mut EnterOwner) {
    let label = ui.label("透かしの文字");
    let mut output = crate::ime_focus::show_singleline(ui, &mut options.text, None, |edit| {
        edit.desired_width(f32::INFINITY)
    });
    output.response.response = output.response.response.labelled_by(label.id);
    crate::ui_helpers::singleline_text_edit_context_menu(ui, &mut output, &mut options.text);
    owner.text(&output.response);
    percent_control(
        ui,
        "文字の高さ (短辺に対する割合)",
        &mut options.text_size_percent,
        1..=50,
        owner,
    );
    let [r, g, b] = options.text_color;
    let mut color = egui::Color32::from_rgb(r, g, b);
    let response = ui.button(format!("文字色 #{r:02X}{g:02X}{b:02X}"));
    owner.widget(&response);
    egui::Popup::menu(&response)
        .style(crate::os_theme::dark_popup_style(ui.ctx()))
        .frame(crate::os_theme::dark_popup_frame(ui.ctx()))
        .close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside)
        .show(|ui| {
            if egui::color_picker::color_picker_color32(
                ui,
                &mut color,
                egui::color_picker::Alpha::Opaque,
            ) {
                options.text_color = [color.r(), color.g(), color.b()];
                owner.operated = true;
            }
            crate::wheel_input::consume_popup_wheel_input(ui.ctx());
        });
    owner.widget(&ui.checkbox(&mut options.text_outline, "黒の縁取り"));
}

fn draw_image_watermark(
    ui: &mut egui::Ui,
    state: &mut ExportBatchDialogState,
    owner: &mut EnterOwner,
    action: &mut BatchDialogAction,
) {
    let label = ui.label("ロゴのパス");
    let mut path_text = state
        .options
        .watermark
        .image_path
        .as_deref()
        .map(|p| p.display().to_string())
        .unwrap_or_default();
    let mut output = crate::ime_focus::show_singleline(ui, &mut path_text, None, |edit| {
        edit.desired_width(f32::INFINITY)
    });
    output.response.response = output.response.response.labelled_by(label.id);
    crate::ui_helpers::singleline_text_edit_context_menu(ui, &mut output, &mut path_text);
    owner.text(&output.response);
    owner.logo_submitted |=
        owner.before == Some(output.response.id) && output.response.lost_focus();
    if output.response.changed() {
        state.options.watermark.image_path =
            (!path_text.is_empty()).then(|| PathBuf::from(path_text));
    }
    if output.response.lost_focus() {
        state.begin_logo_check(ui.ctx());
    }
    let response = ui.button("画像を選ぶ...");
    action.pick_logo |= response.clicked();
    owner.widget(&response);
    if let Some(error) = state.logo_error() {
        ui.add(
            egui::Label::new(egui::RichText::new(error).color(ui.visuals().error_fg_color)).wrap(),
        );
    }
    let options = &mut state.options.watermark;
    let mut original = options.image_size == ImageWatermarkSize::Original;
    let old = original;
    let combo = egui::ComboBox::from_label("ロゴの大きさ")
        .selected_text(if original {
            "実寸"
        } else {
            "出力の幅に対する割合"
        })
        .popup_style(crate::os_theme::dark_popup_style(ui.ctx()))
        .show_ui(ui, |ui| {
            owner.widget(&ui.selectable_value(&mut original, false, "出力の幅に対する割合"));
            owner.widget(&ui.selectable_value(&mut original, true, "実寸"));
        });
    owner.widget(&combo.response);
    if original != old {
        options.image_size = if original {
            ImageWatermarkSize::Original
        } else {
            ImageWatermarkSize::PercentOfWidth(20)
        };
    }
    if let ImageWatermarkSize::PercentOfWidth(percent) = &mut options.image_size {
        percent_control(ui, "ロゴの幅", percent, 1..=100, owner);
    }
}

fn draw_watermark_placement(
    ui: &mut egui::Ui,
    options: &mut BatchWatermark,
    owner: &mut EnterOwner,
) {
    let combo = egui::ComboBox::from_label("透かしの位置")
        .selected_text(options.anchor.label())
        .popup_style(crate::os_theme::dark_popup_style(ui.ctx()))
        .show_ui(ui, |ui| {
            for anchor in WatermarkAnchor::ALL {
                owner.widget(&ui.selectable_value(&mut options.anchor, anchor, anchor.label()));
            }
        });
    owner.widget(&combo.response);
    percent_control(
        ui,
        "余白 (短辺に対する割合)",
        &mut options.margin_percent,
        0..=20,
        owner,
    );
    percent_control(ui, "不透明度", &mut options.opacity_percent, 0..=100, owner);
}

#[cfg(test)]
mod tests;
