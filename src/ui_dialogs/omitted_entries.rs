use crate::app::folder_scan::{OmittedFolderEntries, OmittedReason};
use crate::ui_dialogs::preferences::PreferencesOpenRequest;

impl crate::app::App {
    pub(crate) fn show_omitted_entries_dialog(&mut self, ctx: &egui::Context) {
        if !self.omitted_entries_dialog_visible() {
            self.omitted_entries_dialog_folder = None;
            return;
        }
        let escape = self.dialog_escape_pressed(ctx);
        // Borrow the live result on every frame; never clone or cache its rows in the dialog.
        let owner = self.current_normal_folder_omitted_entries().unwrap();
        let mut open = true;
        let (request, _) = draw_window(ctx, &owner.folder, &owner.omitted, &mut open);
        if !open || escape || request.is_some() {
            self.omitted_entries_dialog_folder = None;
        }
        if let Some(request) = request {
            self.open_preferences_request(request);
        }
    }
}

/// Shared by the product and headless fixtures. The row count is observable for
/// timing-free virtualization tests, without inspecting or copying the dataset.
fn draw_window(
    ctx: &egui::Context,
    folder: &std::path::Path,
    omitted: &OmittedFolderEntries,
    open: &mut bool,
) -> (Option<PreferencesOpenRequest>, usize) {
    let name = folder.file_name().unwrap_or_default().to_string_lossy();
    let mut request = None;
    let mut rows_drawn = 0;
    egui::Window::new(format!("非表示の項目 — {name}"))
        .id(egui::Id::unique("omitted_entries_dialog"))
        .open(open)
        .default_pos(ctx.content_rect().min + egui::vec2(30.0, 25.0))
        .default_size(egui::vec2(760.0, 480.0))
        .min_size(egui::vec2(320.0, 240.0))
        .resizable(true)
        .collapsible(false)
        .show(ctx, |ui| {
            request = crate::ui_main::draw_omitted_entries_summary(ui, omitted.counts);
            ui.separator();
            let width = ui.available_width();
            // Reserve the scrollbar's width for both header and body columns.
            let spacing = ui.spacing().item_spacing.x;
            let usable = (width - ui.spacing().scroll.bar_width - spacing).max(0.0);
            let name_width = usable * 0.55;
            let reason_width = usable - name_width;
            let row_height = ui.text_style_height(&egui::TextStyle::Body).max(22.0);
            ui.horizontal(|ui| {
                draw_cell(ui, "ファイル名", name_width, row_height);
                draw_cell(ui, "非表示理由", reason_width, row_height);
            });
            egui::ScrollArea::vertical()
                .id_salt(("omitted_entries_rows", folder))
                .auto_shrink([false, false])
                .show_rows(ui, row_height, omitted.entries.len(), |ui, rows| {
                    rows_drawn = rows.len();
                    for index in rows {
                        let entry = &omitted.entries[index];
                        ui.horizontal(|ui| {
                            draw_cell(ui, &entry.name, name_width, row_height)
                                .on_hover_text(&entry.name);
                            draw_cell(ui, entry.reason.label(), reason_width, row_height)
                                .on_hover_text(entry.reason.label());
                        });
                    }
                });
        });
    (request, rows_drawn)
}

fn draw_cell(ui: &mut egui::Ui, text: &str, width: f32, height: f32) -> egui::Response {
    // Reserve the entire column, but lay text out from its left edge. add_sized
    // uses a centered child layout even for a left-aligned single-line Label.
    let (rect, _) = ui.allocate_exact_size(egui::vec2(width, height), egui::Sense::hover());
    let mut cell = ui.new_child(
        egui::UiBuilder::new()
            .max_rect(rect)
            .layout(egui::Layout::left_to_right(egui::Align::Center)),
    );
    cell.set_clip_rect(ui.clip_rect().intersect(rect));
    cell.add(egui::Label::new(text).truncate())
}

fn fixture() -> OmittedFolderEntries {
    let mut omitted = OmittedFolderEntries::default();
    for (reason, name) in [
        (OmittedReason::ZipExists, "旅の記録.rar"),
        (OmittedReason::ImagePriority, "写真02.jpg"),
        (OmittedReason::Hidden, "秘密の写真.png"),
        (OmittedReason::IgnoredEpub, "読みかけ.epub"),
        (OmittedReason::FileType, "録画.mp4"),
        (OmittedReason::Unsupported, "メモ.txt"),
        (OmittedReason::System, "Thumbs.db"),
        (OmittedReason::System, "._写真01.jpg"),
    ] {
        omitted.record(reason, name.into());
    }
    omitted.sort_for_display();
    omitted
}

#[doc(hidden)]
pub fn draw_omitted_entries_snapshot_fixture(ui: &mut egui::Ui) {
    draw_window(
        ui.ctx(),
        std::path::Path::new("C:/写真"),
        &fixture(),
        &mut true,
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui_kittest::kittest::Queryable;

    #[test]
    fn omitted_dialog_virtualizes_twenty_thousand_rows_and_keeps_width_stable() {
        let mut omitted = OmittedFolderEntries::default();
        for index in 0..20_000 {
            omitted.record(
                OmittedReason::Unsupported,
                format!("{index:05}{}.txt", "長い名前".repeat(60)),
            );
        }
        let mut harness = crate::test_ui::builder()
            .with_context_setup(crate::ui_fonts::configure_fonts)
            .with_size(egui::vec2(900.0, 600.0))
            .build_ui_state(
                move |ui, rows_drawn| {
                    *rows_drawn = draw_window(
                        ui.ctx(),
                        std::path::Path::new("C:/写真"),
                        &omitted,
                        &mut true,
                    )
                    .1;
                },
                0usize,
            );
        harness.run();
        assert!(
            (1..30).contains(harness.state()),
            "only visible rows are laid out"
        );
        let before = harness.get_by_label("非表示の項目 — 写真").rect();
        for index in 0..30 {
            harness.event(egui::Event::PointerMoved(egui::pos2(
                80.0 + index as f32,
                300.0,
            )));
            harness.run_steps(1);
        }
        assert_eq!(before, harness.get_by_label("非表示の項目 — 写真").rect());
    }

    #[test]
    fn omitted_dialog_settings_links_yield_the_existing_request() {
        let omitted = fixture();
        let mut harness = crate::test_ui::builder()
            .with_context_setup(crate::ui_fonts::configure_fonts)
            .with_size(egui::vec2(900.0, 650.0))
            .build_ui_state(
                move |ui, request| {
                    if let Some(clicked) = draw_window(
                        ui.ctx(),
                        std::path::Path::new("C:/写真"),
                        &omitted,
                        &mut true,
                    )
                    .0
                    {
                        *request = Some(clicked);
                    }
                },
                None,
            );
        harness.run();
        harness.get_by_label("同名ファイル設定を開く").click();
        harness.run();
        assert_eq!(
            *harness.state(),
            Some(PreferencesOpenRequest::DUPLICATE_FILES)
        );
    }
}
