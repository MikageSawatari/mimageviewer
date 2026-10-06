//! Main-window modal owner for a single clipboard HTML snapshot.
use crate::app::{ActionSurface, App};
use crate::clipboard_capture::{
    CaptureIntent, SelectionSnapshot,
    fetch::{CaptureDestination, CaptureFetchEvent, CaptureFetchSession, FetchedImage},
};
use egui::{self, TextureHandle};
use std::collections::HashSet;
use std::sync::Arc;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SelectionPhase {
    Fetching,
    Saving { completed: usize, total: usize },
}

impl SelectionPhase {
    fn can_close(self) -> bool {
        matches!(self, Self::Fetching)
    }
}

enum CaptureCell {
    Pending,
    Ready {
        image: FetchedImage,
        texture: TextureHandle,
        selected: bool,
    },
    Failed(String),
    Duplicate,
}

pub(crate) struct CaptureSelection {
    snapshot: Arc<SelectionSnapshot>,
    session: CaptureFetchSession,
    phase: SelectionPhase,
    cells: Vec<CaptureCell>,
    hashes: HashSet<[u8; 32]>,
    minimum: u32,
    fetched: usize,
    fetch_complete: bool,
}

impl CaptureSelection {
    fn selected_images(&self) -> Vec<FetchedImage> {
        self.cells
            .iter()
            .filter_map(|cell| match cell {
                CaptureCell::Ready {
                    image,
                    selected: true,
                    ..
                } if image.width.min(image.height) >= self.minimum => Some(image.clone()),
                _ => None,
            })
            .collect()
    }
}

impl App {
    pub(crate) fn capture_selection_open(&self) -> bool {
        self.capture_selection.is_some()
    }

    pub(crate) fn handle_capture_selection(
        &mut self,
        snapshot: Arc<SelectionSnapshot>,
        ctx: &egui::Context,
        activate: impl FnOnce(&mut Self, &egui::Context),
    ) {
        let automatic = matches!(&snapshot.intent, CaptureIntent::Automatic { .. });
        if automatic && self.capture_selection_open() {
            return;
        }
        if !self.clipboard_capture_admission_allowed() {
            if automatic {
                self.clipboard_capture_service
                    .resolve_selection(snapshot.token, false);
            } else {
                self.show_feedback_toast_on(
                    "一覧画面で Ctrl+V を押すと開けます".into(),
                    ActionSurface::MainWindow,
                );
            }
            return;
        }
        if snapshot.html.candidates.is_empty() {
            self.show_feedback_toast_on(
                "取り込める画像がありません".into(),
                ActionSurface::MainWindow,
            );
            return;
        }
        let destination = match &snapshot.intent {
            CaptureIntent::Manual { destination } => {
                CaptureDestination::Direct(destination.clone())
            }
            CaptureIntent::Automatic { snapshot } => {
                // The resolver is already started by S1. A pending default is
                // resolved on the fetch worker, never by a shell call in UI.
                CaptureDestination::Monthly(snapshot.config.destination.clone())
            }
        };
        if self.capture_fetch_service.is_none() {
            self.capture_fetch_service =
                crate::clipboard_capture::fetch::CaptureFetchService::new(crate::data_dir::get())
                    .map_err(|error| crate::logger::log(format!("clipboard_capture: {error}")))
                    .ok();
        }
        let Some(service) = &self.capture_fetch_service else {
            self.show_feedback_toast_on(
                "画像の取得を開始できませんでした".into(),
                ActionSurface::MainWindow,
            );
            return;
        };
        let session = service.start(
            snapshot.html.page_url.clone(),
            snapshot.html.candidates.clone(),
            destination,
            snapshot.timestamp.clone(),
            ctx.clone(),
        );
        if automatic {
            self.clipboard_capture_service
                .resolve_selection(snapshot.token, true);
        }
        self.clipboard_capture_service.set_selection_open(true);
        self.capture_selection = Some(CaptureSelection {
            cells: (0..snapshot.html.candidates.len())
                .map(|_| CaptureCell::Pending)
                .collect(),
            snapshot,
            session,
            phase: SelectionPhase::Fetching,
            hashes: HashSet::new(),
            minimum: 100,
            fetched: 0,
            fetch_complete: false,
        });
        if automatic {
            activate(self, ctx);
        }
        ctx.request_repaint();
    }

    pub(crate) fn poll_capture_selection(&mut self, ctx: &egui::Context) {
        let Some(selection) = self.capture_selection.as_mut() else {
            return;
        };
        let mut uploads = 0;
        let mut finished = None;
        while uploads < 2 {
            let Ok(event) = selection.session.results.try_recv() else {
                break;
            };
            match event {
                CaptureFetchEvent::Fetched { index, result } => {
                    selection.fetched += 1;
                    let cell = match result {
                        Ok(mut image) => {
                            if !selection.hashes.insert(image.content_hash) {
                                CaptureCell::Duplicate
                            } else {
                                let thumbnail = std::mem::replace(
                                    &mut image.thumbnail,
                                    egui::ColorImage::new([0, 0], Vec::new()),
                                );
                                let texture = ctx.load_texture(
                                    format!(
                                        "clipboard-capture-{}-{index}",
                                        selection.snapshot.token
                                    ),
                                    thumbnail,
                                    egui::TextureOptions::LINEAR,
                                );
                                uploads += 1;
                                CaptureCell::Ready {
                                    image,
                                    texture,
                                    selected: true,
                                }
                            }
                        }
                        Err(error) => CaptureCell::Failed(error),
                    };
                    if let Some(slot) = selection.cells.get_mut(index) {
                        *slot = cell;
                    }
                }
                CaptureFetchEvent::FetchComplete => selection.fetch_complete = true,
                CaptureFetchEvent::SaveProgress { completed, total } => {
                    selection.phase = SelectionPhase::Saving { completed, total };
                }
                CaptureFetchEvent::SaveComplete(summary) => {
                    finished = Some(summary);
                    break;
                }
            }
        }
        if !selection.session.results.is_empty() {
            ctx.request_repaint();
        }
        if let Some(summary) = finished {
            let mut text = format!("{} 枚保存しました", summary.saved);
            if summary.failed > 0 {
                text.push_str(&format!(" (保存失敗 {} 枚)", summary.failed));
            }
            if summary.metadata_failed > 0 {
                text.push_str(&format!(
                    " (出どころの記録失敗 {} 件)",
                    summary.metadata_failed
                ));
            }
            if let Some(error) = summary.errors.first() {
                text.push_str(&format!(": {error}"));
            }
            self.capture_selection = None;
            self.clipboard_capture_service.set_selection_open(false);
            self.show_feedback_toast_on(text, ActionSurface::MainWindow);
            ctx.request_repaint();
        }
    }

    pub(crate) fn show_capture_selection_dialog(&mut self, ctx: &egui::Context) {
        let escape = self.dialog_escape_pressed(ctx);
        let Some(selection) = self.capture_selection.as_mut() else {
            return;
        };
        let can_close = selection.phase.can_close();
        let mut open = true;
        let mut close = can_close && escape;
        let mut save = false;
        let mut window = egui::Window::new("クリップボードの画像を選んで保存")
            .id(egui::Id::new("clipboard-capture-selection"))
            .default_pos(ctx.content_rect().min + egui::vec2(60.0, 40.0))
            .default_width(720.0)
            .min_width(450.0)
            .resizable(true)
            .collapsible(false);
        if can_close {
            window = window.open(&mut open);
        }
        let domain = url::Url::parse(&selection.snapshot.html.page_url)
            .ok()
            .and_then(|url| url.host_str().map(str::to_owned))
            .unwrap_or_default();
        window.show(ctx, |ui| {
            let actions = draw_selection_contents(
                ui,
                ctx,
                &domain,
                selection.snapshot.html.omitted,
                &mut selection.cells,
                &mut selection.minimum,
                selection.phase,
                selection.fetched,
                selection.fetch_complete,
            );
            save = actions.save;
            close |= actions.close;
        });
        if save {
            let images = selection.selected_images();
            let total = images.len();
            match selection.session.save(images) {
                Ok(()) => {
                    selection.phase = SelectionPhase::Saving {
                        completed: 0,
                        total,
                    }
                }
                Err(error) => {
                    self.show_feedback_toast_on(error, ActionSurface::MainWindow);
                    return;
                }
            }
        }
        // Save can enter Saving in this same frame as Esc or a close click.
        // Permission belongs to the phase at the final transition boundary.
        if selection.phase.can_close() && (close || !open) {
            self.capture_selection = None;
            self.clipboard_capture_service.set_selection_open(false);
            ctx.request_repaint();
        }
    }
}

#[derive(Default)]
struct SelectionActions {
    save: bool,
    close: bool,
}

fn selected_count(cells: &[CaptureCell], minimum: u32) -> usize {
    cells.iter().filter(|cell| matches!(cell,
        CaptureCell::Ready { image, selected: true, .. } if image.width.min(image.height) >= minimum
    )).count()
}

#[allow(clippy::too_many_arguments)]
fn draw_selection_contents(
    ui: &mut egui::Ui,
    ctx: &egui::Context,
    domain: &str,
    omitted: usize,
    cells: &mut [CaptureCell],
    minimum: &mut u32,
    phase: SelectionPhase,
    fetched: usize,
    fetch_complete: bool,
) -> SelectionActions {
    let can_close = phase.can_close();
    let mut actions = SelectionActions::default();
    ui.label(format!("出どころ: {domain}"));
    ui.label(format!("取得: {} / {} 件", fetched, cells.len()));
    if omitted > 0 {
        ui.label(format!("上限のため省略: {} 件", omitted));
    }
    ui.add_enabled_ui(can_close, |ui| {
        ui.horizontal_wrapped(|ui| {
            ui.label("最小サイズ (短辺 px)");
            ui.add(egui::DragValue::new(minimum).range(0..=32768));
            for (label, selected) in [("全選択", true), ("全解除", false)] {
                if ui.button(label).clicked() {
                    for cell in cells.iter_mut() {
                        if let CaptureCell::Ready {
                            selected: checked, ..
                        } = cell
                        {
                            *checked = selected;
                        }
                    }
                }
            }
        });
    });
    ui.separator();
    let visible: Vec<usize> = cells
        .iter()
        .enumerate()
        .filter_map(|(index, cell)| match cell {
            CaptureCell::Duplicate => None,
            CaptureCell::Ready { image, .. } if image.width.min(image.height) < *minimum => None,
            _ => Some(index),
        })
        .collect();
    let columns = ((ui.available_width() / 150.0) as usize).max(1);
    let rows = visible.len().div_ceil(columns);
    let height = (ctx.content_rect().height() - 260.0).clamp(160.0, 460.0);
    ui.style_mut().spacing.scroll =
        super::non_overlapping_dialog_scroll_style(ui.style().spacing.scroll);
    egui::ScrollArea::vertical()
        .id_salt("clipboard-capture-cells")
        .auto_shrink([false, false])
        .max_height(height)
        .show_rows(ui, 166.0, rows, |ui, range| {
            for row in range {
                ui.horizontal(|ui| {
                    for &index in visible.iter().skip(row * columns).take(columns) {
                        ui.allocate_ui_with_layout(
                            egui::vec2(142.0, 160.0),
                            egui::Layout::top_down(egui::Align::Center),
                            |ui| {
                                draw_capture_cell(ui, &mut cells[index], can_close);
                            },
                        );
                    }
                });
            }
        });
    ui.separator();
    match phase {
        SelectionPhase::Fetching => {
            let selected = selected_count(cells, *minimum);
            ui.horizontal(|ui| {
                actions.save = ui
                    .add_enabled(
                        fetch_complete && selected > 0,
                        egui::Button::new(format!("選んだ {selected} 枚を保存")),
                    )
                    .clicked();
                actions.close |= ui.button("閉じる").clicked();
            });
        }
        SelectionPhase::Saving { completed, total } => {
            ui.label(format!("保存中: {completed} / {total} 枚"));
            ui.add_enabled(false, egui::Button::new("閉じる"));
        }
    }

    actions
}

#[doc(hidden)]
pub fn draw_capture_selection_snapshot_fixture(ui: &mut egui::Ui, saving: bool) {
    let ctx = ui.ctx().clone();
    let mut preview = FetchedImage::snapshot_preview();
    let texture = ctx.load_texture(
        "clipboard-capture-snapshot-preview",
        std::mem::replace(
            &mut preview.thumbnail,
            egui::ColorImage::new([0, 0], Vec::new()),
        ),
        egui::TextureOptions::LINEAR,
    );
    let second = if saving {
        let mut image = FetchedImage::snapshot_preview();
        image.width = 640;
        image.height = 480;
        image.content_hash = [1; 32];
        let texture = ctx.load_texture(
            "clipboard-capture-snapshot-second",
            std::mem::replace(
                &mut image.thumbnail,
                egui::ColorImage::new([0, 0], Vec::new()),
            ),
            egui::TextureOptions::LINEAR,
        );
        CaptureCell::Ready {
            image,
            texture,
            selected: true,
        }
    } else {
        CaptureCell::Pending
    };
    let mut cells = vec![
        CaptureCell::Ready {
            image: preview,
            texture,
            selected: true,
        },
        second,
        CaptureCell::Failed("取得できませんでした (403)".into()),
        CaptureCell::Failed("画像が大きすぎます".into()),
    ];
    let phase = if saving {
        SelectionPhase::Saving {
            completed: 1,
            total: 2,
        }
    } else {
        SelectionPhase::Fetching
    };
    ui.heading("クリップボードの画像を選んで保存");
    draw_selection_contents(
        ui,
        &ctx,
        "www.example.com",
        3,
        &mut cells,
        &mut 100,
        phase,
        if saving { 4 } else { 3 },
        saving,
    );
}

fn draw_capture_cell(ui: &mut egui::Ui, cell: &mut CaptureCell, enabled: bool) {
    match cell {
        CaptureCell::Pending => {
            egui::Frame::group(ui.style()).show(ui, |ui| {
                ui.set_min_size(egui::vec2(128.0, 120.0));
                ui.label("取得中");
            });
        }
        CaptureCell::Failed(error) => {
            egui::Frame::group(ui.style()).show(ui, |ui| {
                ui.set_min_size(egui::vec2(128.0, 120.0));
                ui.colored_label(ui.visuals().error_fg_color, error.as_str());
            });
            ui.add_enabled(false, egui::Checkbox::new(&mut false, "選択不可"));
        }
        CaptureCell::Ready {
            image,
            texture,
            selected,
        } => {
            ui.add(
                egui::Image::new(&*texture)
                    .fit_to_exact_size(egui::vec2(128.0, 120.0))
                    .maintain_aspect_ratio(true),
            );
            ui.add_enabled(
                enabled,
                egui::Checkbox::new(selected, format!("{} × {}", image.width, image.height)),
            );
        }
        CaptureCell::Duplicate => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use base64::Engine;
    use egui_kittest::{Harness, kittest::Queryable};

    fn ready_dialog() -> crate::app::AppTestEnvForTest {
        let mut app = crate::app::setup_app_for_test();
        let ctx = egui::Context::default();
        let mut bytes = Vec::new();
        {
            let mut encoder = png::Encoder::new(&mut bytes, 120, 100);
            encoder.set_color(png::ColorType::Rgba);
            encoder.set_depth(png::BitDepth::Eight);
            encoder
                .write_header()
                .unwrap()
                .write_image_data(&vec![255; 120 * 100 * 4])
                .unwrap();
        }
        let url = format!(
            "data:image/png;base64,{}",
            base64::engine::general_purpose::STANDARD.encode(bytes)
        );
        let snapshot = Arc::new(SelectionSnapshot {
            token: 77,
            html_epoch: 0,
            html: crate::clipboard_capture::html::HtmlCapture {
                page_url: "https://example.com/page".into(),
                candidates: vec![url],
                omitted: 0,
            },
            intent: CaptureIntent::Manual {
                destination: app.tmp.path().join("paste"),
            },
            timestamp: crate::clipboard_capture::data::CaptureTimestamp {
                stamp: "20261006-120000-000".into(),
                month: "2026-10".into(),
            },
        });
        app.handle_capture_selection(snapshot, &ctx, |_, _| panic!("manual activation"));
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while !app.capture_selection.as_ref().unwrap().fetch_complete {
            app.poll_capture_selection(&ctx);
            assert!(std::time::Instant::now() < deadline);
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
        assert_eq!(
            app.capture_selection
                .as_ref()
                .unwrap()
                .selected_images()
                .len(),
            1
        );
        app
    }

    #[test]
    fn clipboard_fetching_all_three_close_routes_cancel_modal() {
        for route in ["button", "escape", "title"] {
            let mut harness = Harness::builder()
                .with_size(egui::vec2(900.0, 700.0))
                .build_state(
                    |ctx, app: &mut crate::app::AppTestEnvForTest| {
                        app.show_capture_selection_dialog(ctx)
                    },
                    ready_dialog(),
                );
            harness.run();
            match route {
                "button" => harness.get_by_label("閉じる").click(),
                "escape" => harness.key_press(egui::Key::Escape),
                "title" => harness.get_by_label("Close window").click(),
                _ => unreachable!(),
            }
            harness.run();
            assert!(!harness.state().capture_selection_open(), "{route}");
        }
    }

    #[test]
    fn clipboard_save_and_escape_in_same_frame_keeps_saving_modal() {
        let mut harness = Harness::builder()
            .with_size(egui::vec2(900.0, 700.0))
            .build_state(
                |ctx, app: &mut crate::app::AppTestEnvForTest| {
                    app.show_capture_selection_dialog(ctx)
                },
                ready_dialog(),
            );
        harness.run();
        // Follow the Harness click sequence: establish the hovered window and
        // widget in a separate frame before pressing. Only release + Esc are
        // combined; merging the initial hover with press can miss the hit.
        harness.get_by_label("選んだ 1 枚を保存").hover();
        harness.step();
        let pos = harness.get_by_label("選んだ 1 枚を保存").rect().center();
        harness
            .input_mut()
            .events
            .extend([egui::Event::PointerButton {
                pos,
                button: egui::PointerButton::Primary,
                pressed: true,
                modifiers: egui::Modifiers::NONE,
            }]);
        harness.step();
        harness.input_mut().events.extend([
            egui::Event::PointerButton {
                pos,
                button: egui::PointerButton::Primary,
                pressed: false,
                modifiers: egui::Modifiers::NONE,
            },
            egui::Event::Key {
                key: egui::Key::Escape,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers: egui::Modifiers::NONE,
            },
        ]);
        harness.step();
        assert!(harness.state().capture_selection_open());
        assert!(
            !harness
                .state()
                .capture_selection
                .as_ref()
                .unwrap()
                .phase
                .can_close()
        );
        assert!(harness.state().common_modal_dialog_open());
        assert!(harness.state().document_open_modal_admission_blocked());
        // Completion is deliberately not polled by this harness: all close
        // routes must preserve the modal while it is awaiting SaveComplete.
        harness.key_press(egui::Key::Escape);
        harness.get_by_label("閉じる").click();
        harness.run();
        assert!(harness.state().capture_selection_open());
        assert!(harness.query_by_label("Close window").is_none());
        let ctx = harness.ctx.clone();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while harness.state().capture_selection_open() {
            harness.state_mut().poll_capture_selection(&ctx);
            assert!(std::time::Instant::now() < deadline);
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
        assert_eq!(
            harness.state().fs_feedback_toast.as_ref().unwrap().0,
            "1 枚保存しました"
        );
    }
}
