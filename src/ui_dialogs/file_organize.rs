//! Fixed destinations: one request owner, Shell execution, and external-change refresh only.
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::{atomic::Ordering, mpsc};

use crate::app::{App, ExternalChangeCheck};
use crate::grid_item::GridItem;
use crate::settings::FileOrganizeDestination;
use crate::shell_file_ops::{ShellTransferOperation, ShellTransferRequest, ShellTransferResult};
use eframe::egui;

#[derive(Default)]
pub(crate) enum FileOrganizeRequest {
    #[default]
    Hidden,
    Selecting(Box<FileOrganizeSelection>),
    Running(Box<FileOrganizeRunning>),
}

pub(crate) struct FileOrganizeSelection {
    pub(crate) sources: Vec<PathBuf>,
    pub(crate) destinations: Vec<FileOrganizeDestination>,
    pub(crate) focus: Option<(usize, Option<ShellTransferOperation>)>,
}

pub(crate) struct FileOrganizeRunning {
    pub(crate) request: ShellTransferRequest,
    pub(crate) rx: mpsc::Receiver<ShellTransferResult>,
}

impl FileOrganizeRequest {
    // Consume the selected request once. In particular, a duplicate cannot retire Running.
    fn take_for_submission(
        &mut self,
        row: usize,
        operation: ShellTransferOperation,
        remote_owned: bool,
        closing: bool,
        owner: Option<isize>,
    ) -> Result<Option<ShellTransferRequest>, &'static str> {
        if !matches!(self, Self::Selecting(_)) {
            return Ok(None);
        }
        let Self::Selecting(selection) = std::mem::take(self) else {
            unreachable!()
        };
        let FileOrganizeSelection {
            sources,
            destinations,
            ..
        } = *selection;
        if remote_owned {
            return Err("リモート接続中のため整理操作を実行しませんでした");
        }
        if closing {
            return Err("終了が要求されたため整理操作を実行しませんでした");
        }
        if owner.is_none_or(|hwnd| hwnd == 0) {
            return Err("整理操作を開始できませんでした");
        }
        let Some(destination) = destinations.get(row) else {
            return Err("整理先を選び直してください");
        };
        Ok(Some(ShellTransferRequest {
            sources,
            destination: destination.path.clone(),
            operation,
        }))
    }
}

fn resolve_sources(
    items: &[GridItem],
    checked: &HashSet<usize>,
    cursor: Option<usize>,
    order: &[usize],
) -> Result<Vec<PathBuf>, String> {
    let indices = if checked.is_empty() {
        cursor.into_iter().collect::<Vec<_>>()
    } else {
        // Grid order is the visible order; refuse stale/missing indices rather than omit targets.
        let mut indices: Vec<_> = order
            .iter()
            .copied()
            .filter(|idx| checked.contains(idx))
            .collect();
        let ordered: HashSet<_> = indices.iter().copied().collect();
        let mut remaining: Vec<_> = checked
            .iter()
            .copied()
            .filter(|idx| !ordered.contains(idx))
            .collect();
        remaining.sort_unstable();
        indices.extend(remaining);
        indices
    };
    if indices.is_empty() {
        return Err("整理する項目を選択してください".to_owned());
    }
    let mut sources = Vec::with_capacity(indices.len());
    for index in indices {
        let Some(item) = items.get(index) else {
            return Err("選択項目が変更されました。選び直してください".to_owned());
        };
        let Some(path) = item.drag_source_path() else {
            let reason = item
                .file_operation_refusal()
                .map(|reason| reason.message("移動 / コピー"))
                .unwrap_or_else(|| "この項目は移動 / コピーできません".to_owned());
            return Err(format!(
                "{}: {reason}\n対象外の項目があるため実行しません。該当項目のチェックを外してください",
                item.name()
            ));
        };
        sources.push(path.to_owned());
    }
    Ok(sources)
}

fn refresh_matches(current: &Path, request: &ShellTransferRequest) -> bool {
    crate::folder_tree::path_eq(current, &request.destination)
        || request.sources.iter().any(|source| {
            source
                .parent()
                .is_some_and(|parent| crate::folder_tree::path_eq(current, parent))
        })
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FileOrganizeUiAction {
    Submit(usize, ShellTransferOperation),
    Preferences,
    Close,
}

/// Same contents used by the application and headless UI snapshots.
pub fn render_file_organize_contents(
    ui: &mut egui::Ui,
    sources: &[PathBuf],
    destinations: &[FileOrganizeDestination],
    focus: &mut Option<(usize, Option<ShellTransferOperation>)>,
) -> Option<FileOrganizeUiAction> {
    ui.label(format!("対象: {} 件", sources.len()));
    ui.collapsing("対象の一覧", |ui| {
        egui::ScrollArea::vertical().max_height(100.0).show_rows(
            ui,
            ui.text_style_height(&egui::TextStyle::Body),
            sources.len(),
            |ui, rows| {
                for row in rows {
                    let path = sources[row].display().to_string();
                    ui.add(egui::Label::new(&path).truncate())
                        .on_hover_text(path);
                }
            },
        );
    });
    ui.separator();
    let mut action = None;
    if destinations.is_empty() {
        ui.label("ファイル整理先はまだ登録されていません。環境設定で登録してください。");
        if ui.button("環境設定で登録…").clicked() {
            action = Some(FileOrganizeUiAction::Preferences);
        }
    } else {
        ui.spacing_mut().scroll = egui::style::ScrollStyle::solid();
        egui::ScrollArea::vertical()
            .max_height(ui.available_height().min(360.0).max(100.0) - 40.0)
            .auto_shrink([false, false])
            .show(ui, |ui| {
                for (row, destination) in destinations.iter().enumerate() {
                    let group = ui.group(|ui| {
                        ui.label(egui::RichText::new(&destination.name).strong());
                        ui.add(egui::Label::new(destination.path.display().to_string()).wrap())
                            .on_hover_text(destination.path.display().to_string());
                        ui.horizontal(|ui| {
                            for (operation, label) in [
                                (ShellTransferOperation::Move, "移動"),
                                (ShellTransferOperation::Copy, "コピー"),
                            ] {
                                let response = ui.add(
                                    egui::Button::new(label)
                                        .selected(*focus == Some((row, Some(operation))))
                                        .min_size(egui::vec2(72.0, 28.0)),
                                );
                                if response.clicked() {
                                    *focus = Some((row, Some(operation)));
                                    action = Some(FileOrganizeUiAction::Submit(row, operation));
                                }
                                ui.add_space(24.0);
                            }
                        });
                    });
                    if focus.is_some_and(|(selected, _)| selected == row) {
                        ui.painter().rect_stroke(
                            group.response.rect,
                            6.0,
                            ui.visuals().selection.stroke,
                            egui::StrokeKind::Inside,
                        );
                    }
                }
            });
    }
    ui.separator();
    if ui.button("閉じる").clicked() {
        action = Some(FileOrganizeUiAction::Close);
    }
    action
}

impl App {
    pub(crate) fn file_organize_dialog_visible(&self) -> bool {
        matches!(
            self.file_organize_request,
            FileOrganizeRequest::Selecting(_)
        )
    }

    pub(crate) fn request_file_organize_dialog(&mut self, clicked: Option<usize>) {
        if !matches!(self.file_organize_request, FileOrganizeRequest::Hidden) {
            self.show_feedback_toast("整理操作が完了するまでお待ちください".to_owned());
            return;
        }
        if self.remote_session_blocks_local_control() {
            self.show_feedback_toast("リモート接続中は整理操作を開始できません".to_owned());
            return;
        }
        match resolve_sources(
            &self.items,
            &self.checked,
            clicked.or(self.selected),
            self.current_grid_order(),
        ) {
            Ok(sources) => {
                self.file_organize_request =
                    FileOrganizeRequest::Selecting(Box::new(FileOrganizeSelection {
                        sources,
                        destinations: self.settings.file_organize_destinations.clone(),
                        focus: None,
                    }))
            }
            Err(message) => self.show_feedback_toast(message),
        }
    }

    pub(crate) fn submit_file_organize(
        &mut self,
        ctx: &egui::Context,
        row: usize,
        operation: ShellTransferOperation,
    ) {
        // The sole admission boundary shared by pointer and Enter. No worker is spawned earlier.
        let closing = self.shutdown_requested.load(Ordering::SeqCst)
            || self
                .tray_controller
                .as_ref()
                .is_some_and(|tray| tray.is_quit_requested())
            || ctx.input(|input| {
                input
                    .raw
                    .viewports
                    .get(&egui::ViewportId::ROOT)
                    .is_some_and(|viewport| viewport.close_requested())
            });
        match self.file_organize_request.take_for_submission(
            row,
            operation,
            self.remote_session_blocks_local_control(),
            closing,
            self.main_hwnd,
        ) {
            Ok(Some(request)) => {
                let rx = crate::shell_file_ops::transfer_items_async(
                    self.main_hwnd,
                    request.clone(),
                    ctx.clone(),
                );
                self.file_organize_request =
                    FileOrganizeRequest::Running(Box::new(FileOrganizeRunning { request, rx }));
            }
            Ok(None) => {}
            Err(message) => self.show_feedback_toast(message.to_owned()),
        }
    }

    pub(crate) fn poll_file_organize(&mut self, ctx: &egui::Context) {
        if self.file_organize_dialog_visible() && self.remote_session_blocks_local_control() {
            self.file_organize_request = FileOrganizeRequest::Hidden;
            self.show_feedback_toast(
                "リモート接続されたため整理先の選択画面を閉じました".to_owned(),
            );
            return;
        }
        let FileOrganizeRequest::Running(running) = &self.file_organize_request else {
            return;
        };
        let result = match running.rx.try_recv() {
            Ok(result) => result,
            Err(mpsc::TryRecvError::Empty) => return,
            Err(mpsc::TryRecvError::Disconnected) => {
                Err("整理操作の結果を受信できませんでした".to_owned())
            }
        };
        let FileOrganizeRequest::Running(running) = std::mem::take(&mut self.file_organize_request)
        else {
            unreachable!()
        };
        let request = running.request;
        match result {
            Ok(outcome) if outcome.aborted => self.show_feedback_toast(
                "操作が中断されました。処理済みの項目は元に戻りません".to_owned(),
            ),
            Ok(_) => {}
            Err(message) => {
                crate::logger::log(format!("file_organize: {message}"));
                self.show_feedback_toast(format!(
                    "操作を完了できませんでした。フォルダの内容を確認してください\n{message}"
                ));
            }
        }
        // No removal, metadata migration, retained-list cleanup or viewer invalidation here.
        if self.is_physical_folder_listing()
            && self
                .current_folder
                .as_deref()
                .is_some_and(|current| refresh_matches(current, &request))
        {
            self.check_external_folder_changes(ctx, ExternalChangeCheck::Notified);
        }
    }

    pub(crate) fn show_file_organize_dialog(&mut self, ctx: &egui::Context) {
        if !self.file_organize_dialog_visible() {
            return;
        }
        let escape = self.dialog_escape_pressed(ctx);
        let enter = self.dialog_enter_pressed(ctx);
        let allow_arrows = !self.ime_input_active(ctx);
        let FileOrganizeRequest::Selecting(selection) = &mut self.file_organize_request else {
            return;
        };
        let FileOrganizeSelection {
            sources,
            destinations,
            focus,
        } = selection.as_mut();
        if allow_arrows && !destinations.is_empty() {
            ctx.input_mut(|input| {
                let modifiers = egui::Modifiers::NONE;
                let down = input.consume_key(modifiers, egui::Key::ArrowDown);
                let up = input.consume_key(modifiers, egui::Key::ArrowUp);
                let left = input.consume_key(modifiers, egui::Key::ArrowLeft);
                let right = input.consume_key(modifiers, egui::Key::ArrowRight);
                if down || up || left || right {
                    let (row, operation) = focus.unwrap_or((0, None));
                    let row = if down {
                        (row + usize::from(focus.is_some())).min(destinations.len() - 1)
                    } else if up {
                        row.saturating_sub(1)
                    } else {
                        row
                    };
                    let operation = if left {
                        Some(ShellTransferOperation::Move)
                    } else if right {
                        Some(ShellTransferOperation::Copy)
                    } else {
                        operation
                    };
                    *focus = Some((row, operation));
                }
            });
        }
        let action = egui::Modal::new(egui::Id::new("file_organize_modal"))
            .show(ctx, |ui| {
                ui.set_width((ctx.content_rect().width() - 48.0).clamp(240.0, 580.0));
                ui.heading("ファイル整理先");
                render_file_organize_contents(ui, sources, destinations, focus)
            })
            .inner;
        let action = if escape {
            Some(FileOrganizeUiAction::Close)
        } else {
            action.or_else(|| {
                if enter {
                    focus.and_then(|(row, operation)| {
                        operation.map(|operation| FileOrganizeUiAction::Submit(row, operation))
                    })
                } else {
                    None
                }
            })
        };
        match action {
            Some(FileOrganizeUiAction::Submit(row, operation)) => {
                self.submit_file_organize(ctx, row, operation)
            }
            Some(FileOrganizeUiAction::Close) => {
                self.file_organize_request = FileOrganizeRequest::Hidden
            }
            Some(FileOrganizeUiAction::Preferences) => {
                self.file_organize_request = FileOrganizeRequest::Hidden;
                self.open_preferences_page(crate::ui_dialogs::preferences::PreferencesPage::Folder);
            }
            None => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn selecting() -> FileOrganizeRequest {
        FileOrganizeRequest::Selecting(Box::new(FileOrganizeSelection {
            sources: vec![PathBuf::from(r"C:\source\a.jpg")],
            destinations: vec![FileOrganizeDestination {
                name: "archive".into(),
                path: PathBuf::from(r"C:\destination"),
            }],
            focus: None,
        }))
    }

    #[test]
    fn file_organize_selection_checks_every_target_and_keeps_grid_order() {
        let items = vec![
            GridItem::Image("a.jpg".into()),
            GridItem::Folder("folder".into()),
            GridItem::ZipImage {
                zip_path: "a.zip".into(),
                entry_name: "page.jpg".into(),
            },
        ];
        assert_eq!(
            resolve_sources(&items, &HashSet::new(), Some(1), &[0, 1, 2]).unwrap(),
            vec![PathBuf::from("folder")]
        );
        assert_eq!(
            resolve_sources(&items, &HashSet::from([0, 1]), Some(2), &[1, 0, 2]).unwrap(),
            vec![PathBuf::from("folder"), PathBuf::from("a.jpg")]
        );
        assert!(
            resolve_sources(&items, &HashSet::from([0, 2]), Some(1), &[0, 1, 2])
                .unwrap_err()
                .contains("実行しません")
        );
        assert!(resolve_sources(&items, &HashSet::from([8]), Some(1), &[0, 1, 2]).is_err());
        assert!(resolve_sources(&items, &HashSet::new(), Some(2), &[0, 1, 2]).is_err());
    }

    #[test]
    fn file_organize_admission_rejects_remote_exit_and_duplicate() {
        for (remote, closing, owner) in [
            (true, false, Some(1)),
            (false, true, Some(1)),
            (false, false, None),
        ] {
            let mut state = selecting();
            assert!(
                state
                    .take_for_submission(0, ShellTransferOperation::Move, remote, closing, owner)
                    .is_err()
            );
            assert!(matches!(state, FileOrganizeRequest::Hidden));
        }
        let mut state = selecting();
        let request = state
            .take_for_submission(0, ShellTransferOperation::Copy, false, false, Some(1))
            .unwrap()
            .unwrap();
        let (_tx, rx) = mpsc::channel();
        state = FileOrganizeRequest::Running(Box::new(FileOrganizeRunning { request, rx }));
        assert!(
            state
                .take_for_submission(0, ShellTransferOperation::Move, true, true, Some(1))
                .unwrap()
                .is_none()
        );
        assert!(matches!(state, FileOrganizeRequest::Running(_)));
    }

    #[test]
    fn file_organize_refresh_uses_current_source_or_destination_only() {
        let request = ShellTransferRequest {
            sources: vec![PathBuf::from(r"C:\source\a.jpg")],
            destination: PathBuf::from(r"D:\destination"),
            operation: ShellTransferOperation::Move,
        };
        assert!(refresh_matches(Path::new(r"C:\source"), &request));
        assert!(refresh_matches(Path::new(r"D:\destination"), &request));
        assert!(!refresh_matches(Path::new(r"C:\elsewhere"), &request));
    }
}
