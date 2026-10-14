//! Shared Everything controls and progress drawing. Request ownership stays in App.

#[derive(Default)]
pub(super) struct EverythingControls {
    pub(super) send: bool,
    pub(super) close: bool,
    pub(super) has_focus: bool,
    pub(super) lost_focus: bool,
}

pub(super) fn draw_everything_controls(
    ui: &mut egui::Ui,
    query: &mut String,
    focus_request: &mut bool,
) -> EverythingControls {
    let mut controls = EverythingControls::default();
    ui.horizontal_wrapped(|ui| {
        ui.label("Everything:");
        let button_width = |label: &str| {
            ui.fonts_mut(|fonts| {
                fonts
                    .layout_no_wrap(
                        label.into(),
                        egui::TextStyle::Button.resolve(ui.style()),
                        ui.visuals().text_color(),
                    )
                    .size()
                    .x
            }) + ui.spacing().button_padding.x * 2.0
        };
        let reserved = button_width("検索") + button_width("×") + ui.spacing().item_spacing.x * 3.0;
        let width = (ui.available_width() - reserved).clamp(80.0, 420.0);
        // The caller latch requests opening focus; recovery retries belong to
        // the helper. Explicit requests preserve an active preedit selection.
        let select_query = *focus_request && !crate::ime_focus::ime_input_active(ui.ctx());
        let mut output =
            crate::ime_focus::show_singleline(ui, query, Some(focus_request), |edit| {
                edit.hint_text("ファイル名・Everything 構文 (ext:jpg;png など)")
                    .desired_width(width)
            });
        if select_query {
            let range = egui::text::CCursorRange::two(
                egui::text::CCursor::new(0),
                egui::text::CCursor::new(query.chars().count()),
            );
            output.state.cursor.set_char_range(Some(range));
            output.state.clone().store(ui.ctx(), output.response.id);
            output.cursor_range = Some(range);
            ui.ctx().request_repaint();
        }
        crate::ui_helpers::singleline_text_edit_context_menu(ui, &mut output, query);
        controls.has_focus = output.response.has_focus();
        controls.lost_focus = output.response.lost_focus();
        controls.send = ui.button("検索").clicked();
        controls.close = ui.small_button("×").on_hover_text("検索を閉じる").clicked();
    });
    ui.label(
        egui::RichText::new(
            "Everything クライアントの起動が必要です。先頭10,000件を取得し、対応形式だけ表示します。",
        )
        .small()
        .color(ui.visuals().weak_text_color()),
    );
    controls
}

pub(super) struct EverythingStatus<'a> {
    pub(super) total: u32,
    pub(super) raw_returned: usize,
    pub(super) displayed: usize,
    pub(super) unreadable: usize,
    pub(super) limit_reached: bool,
    pub(super) query_changed: bool,
    pub(super) error: Option<&'a str>,
}

pub(super) fn draw_everything_status(ui: &mut egui::Ui, status: EverythingStatus<'_>) {
    if let Some(error) = status.error {
        ui.colored_label(ui.visuals().error_fg_color, error);
    }
    ui.label(format!(
        "{} 件中 {} 件取得 / mIV 表示対象 {} 件 / 読めなかった項目 {} 件",
        status.total, status.raw_returned, status.displayed, status.unreadable
    ));
    if status.displayed == 0 {
        if status.raw_returned == 0 {
            ui.label("検索条件に一致するファイルがありません。");
        } else {
            ui.label("取得した結果に表示できるファイルがありません。ファイル種類の設定や検索条件を確認してください。");
            if status.limit_reached {
                ui.label("フォルダや非対応ファイルも取得上限に含まれます。ext:jpg;png などで条件を絞って再検索してください。");
            }
        }
    }
    if status.limit_reached {
        ui.label("取得上限に達しました。必要なら検索条件を絞ってください。");
    }
    if status.query_changed {
        ui.label("表示中の結果には、新しい条件がまだ適用されていません。");
    }
}

/// Draw only: App handles IME-safe Escape and retires the request on cancellation.
pub(super) fn draw_everything_progress(ctx: &egui::Context) -> bool {
    let mut open = true;
    let mut cancel = false;
    let viewport = ctx.content_rect();
    egui::Window::new("Everything ファイル名検索")
        .id(egui::Id::new("everything_search_progress"))
        .default_pos(viewport.min + egui::vec2(60.0, 40.0))
        .default_width(340.0_f32.min((viewport.width() - 80.0).max(160.0)))
        .max_width((viewport.width() - 32.0).max(160.0))
        .max_height((viewport.height() - 60.0).max(100.0))
        .open(&mut open)
        .resizable(false)
        .show(ctx, |ui| {
            let action_height = ui.spacing().interact_size.y + ui.spacing().item_spacing.y;
            egui::ScrollArea::vertical()
                .auto_shrink([false, true])
                .max_height((ui.available_height() - action_height).max(20.0))
                .show(ui, |ui| {
                    ui.horizontal_wrapped(|ui| {
                        ui.spinner();
                        ui.label("検索結果を準備しています…");
                    });
                    ui.label("検索・対応形式の確認・サムネイルの準備情報を取得中です。");
                });
            cancel = ui.button("取消").clicked();
        });
    !open || cancel
}

#[doc(hidden)]
pub fn draw_everything_search_snapshot_fixture(ui: &mut egui::Ui) {
    let mut query = String::from("旅行 日本語 ext:jpg;png;pdf");
    let mut focus_request = false;
    draw_everything_controls(ui, &mut query, &mut focus_request);
    draw_everything_status(
        ui,
        EverythingStatus {
            total: 32_145,
            raw_returned: 10_000,
            displayed: 6_789,
            unreadable: 12,
            limit_reached: true,
            query_changed: true,
            error: Some(crate::everything_ipc::EverythingError::Busy.user_message()),
        },
    );
}

#[doc(hidden)]
pub fn draw_everything_progress_snapshot_fixture(ctx: &egui::Context) {
    draw_everything_progress(ctx);
}

#[cfg(test)]
mod tests {
    use egui_kittest::kittest::Queryable;

    #[test]
    fn everything_search_bar_not_responding_narrow_dark() {
        let mut fonts_ready = false;
        let size = egui::vec2(320.0, 400.0);
        let mut harness = crate::test_ui::builder().with_size(size).build(move |ctx| {
            crate::os_theme::apply_resolved(ctx, crate::os_theme::ResolvedTheme::Dark);
            if !fonts_ready {
                crate::ui_fonts::configure_fonts(ctx);
                fonts_ready = true;
                ctx.request_repaint();
                return;
            }
            egui::CentralPanel::default().show(ctx, |ui| {
                let mut query = String::from("旅行 日本語 ext:jpg;png;pdf");
                let mut focus = false;
                super::draw_everything_controls(ui, &mut query, &mut focus);
                // Production renders a failed search via the same error label.
                ui.colored_label(
                    ui.visuals().error_fg_color,
                    crate::everything_ipc::EverythingError::NotResponding.user_message(),
                );
            });
        });
        harness.run();
        let viewport = egui::Rect::from_min_size(egui::Pos2::ZERO, size);
        for label in [
            "検索",
            "×",
            crate::everything_ipc::EverythingError::NotResponding.user_message(),
        ] {
            assert!(viewport.contains_rect(harness.get_by_label(label).rect()));
        }
        harness.snapshot("everything_search_bar_not_responding_narrow_dark");
    }
}
