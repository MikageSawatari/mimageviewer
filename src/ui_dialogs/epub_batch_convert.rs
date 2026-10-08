//! Explicit EPUB to sibling-PDF batch conversion. All filesystem and database work stays in the worker.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, mpsc};

use crate::app::App;
use crate::epub_convert::{self, CancelToken, EpubConvertError, PdfiumConvertedPdfVerifier};
use crate::grid_item::GridItem;

#[derive(Debug)]
pub(crate) enum EpubBatchResult {
    Saved(PathBuf, Vec<String>),
    Skipped,
    Failed(String),
}

pub(crate) enum EpubBatchMsg {
    Start(String),
    Done(PathBuf, EpubBatchResult),
    Finished,
}

pub(crate) struct EpubBatchPending {
    rx: mpsc::Receiver<EpubBatchMsg>,
    cancel_after_current: Arc<AtomicBool>,
    total: usize,
    current_name: String,
    results: Vec<(PathBuf, EpubBatchResult)>,
    finished: bool,
    refreshed: bool,
}

#[cfg(test)]
impl EpubBatchPending {
    pub(crate) fn completed_for_test(source: PathBuf, result: EpubBatchResult) -> Self {
        let (tx, rx) = mpsc::channel();
        tx.send(EpubBatchMsg::Done(source, result)).unwrap();
        tx.send(EpubBatchMsg::Finished).unwrap();
        Self {
            rx,
            cancel_after_current: Arc::new(AtomicBool::new(false)),
            total: 1,
            current_name: String::new(),
            results: Vec::new(),
            finished: false,
            refreshed: false,
        }
    }
}

fn run_batch(
    targets: Vec<PathBuf>,
    cancel_after_current: Arc<AtomicBool>,
    tx: mpsc::Sender<EpubBatchMsg>,
    save: impl Fn(&std::path::Path) -> Result<epub_convert::SavedPdf, EpubConvertError>,
) {
    for source in targets {
        if cancel_after_current.load(Ordering::Acquire) {
            break;
        }
        let name = source
            .file_name()
            .unwrap_or(source.as_os_str())
            .to_string_lossy()
            .into_owned();
        let _ = tx.send(EpubBatchMsg::Start(name));
        let result = match save(&source) {
            Ok(saved) => EpubBatchResult::Saved(saved.path, saved.user_data_errors),
            Err(EpubConvertError::ExistingPdf) => EpubBatchResult::Skipped,
            Err(error) => EpubBatchResult::Failed(super::epub_convert::error_message(&error)),
        };
        let _ = tx.send(EpubBatchMsg::Done(source, result));
    }
    let _ = tx.send(EpubBatchMsg::Finished);
}

fn collect_epub_targets(
    items: &[GridItem],
    indices: impl IntoIterator<Item = usize>,
) -> Vec<PathBuf> {
    let mut seen = std::collections::HashSet::new();
    indices
        .into_iter()
        .filter_map(|index| match items.get(index) {
            Some(GridItem::PdfFile(path))
                if path
                    .extension()
                    .is_some_and(|ext| ext.eq_ignore_ascii_case("epub"))
                    && seen.insert(path.clone()) =>
            {
                Some(path.clone())
            }
            _ => None,
        })
        .collect()
}

impl App {
    fn selected_epub_batch_paths(&self, clicked_index: Option<usize>) -> Vec<PathBuf> {
        let indices = if self.checked.is_empty() {
            clicked_index.map(|index| vec![index]).unwrap_or_else(|| {
                self.selection_target_indices(crate::app::ActionSurface::MainWindow)
            })
        } else {
            self.selection_target_indices(crate::app::ActionSurface::MainWindow)
        };
        collect_epub_targets(&self.items, indices)
    }

    pub(crate) fn start_batch_convert_to_pdf(&mut self) {
        self.start_batch_convert_to_pdf_at(None);
    }

    pub(crate) fn start_batch_convert_to_pdf_at(&mut self, clicked_index: Option<usize>) {
        if self.document_open_modal_admission_blocked()
            || self.batch_convert.is_some()
            || self.archive_convert.is_some()
            || self.pdf_enumerate_pending.as_ref().is_some_and(|pending| {
                matches!(pending.5, crate::app::PdfOpenPhase::ColdCandidate { .. })
            })
            || self
                .top_level_grid_view
                .open_path_classification()
                .is_some()
            || self.folder_open_preparation_pending()
            || self.startup_open_path_resolve_pending.is_some()
            || self.bookmark_open_pending.is_some()
            || self
                .top_level_grid_view
                .history_navigation_transition()
                .is_some()
        {
            return;
        }
        let targets = self.selected_epub_batch_paths(clicked_index);
        if targets.is_empty() {
            return;
        }
        let total = targets.len();
        let cancel_after_current = Arc::new(AtomicBool::new(false));
        let cancel = Arc::clone(&cancel_after_current);
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            run_batch(targets, cancel, tx, |source| {
                let gate = crate::pdf_loader::epub_conversion_guard()
                    .map_err(|error| EpubConvertError::Unavailable(error.to_string()))?;
                let (progress, _rx) = mpsc::channel();
                let cancel = CancelToken::new()?;
                epub_convert::save_sibling(
                    gate,
                    source,
                    &cancel,
                    &progress,
                    super::epub_convert::EPUB_WORKER_TIMEOUT_SECS,
                    &PdfiumConvertedPdfVerifier,
                )
            });
        });
        self.epub_batch_convert = Some(EpubBatchPending {
            rx,
            cancel_after_current,
            total,
            current_name: String::new(),
            results: Vec::new(),
            finished: false,
            refreshed: false,
        });
    }

    pub(crate) fn poll_epub_batch_convert(&mut self) {
        let Some(pending) = self.epub_batch_convert.as_mut() else {
            return;
        };
        loop {
            match pending.rx.try_recv() {
                Ok(EpubBatchMsg::Start(name)) => pending.current_name = name,
                Ok(EpubBatchMsg::Done(path, result)) => pending.results.push((path, result)),
                Ok(EpubBatchMsg::Finished) => {
                    pending.finished = true;
                    break;
                }
                Err(mpsc::TryRecvError::Empty) => break,
                Err(mpsc::TryRecvError::Disconnected) => {
                    pending.finished = true;
                    if pending.results.len() < pending.total
                        && !pending.cancel_after_current.load(Ordering::Acquire)
                    {
                        pending.results.push((
                            PathBuf::new(),
                            EpubBatchResult::Failed("処理が中断されました".into()),
                        ));
                    }
                    break;
                }
            }
        }
        if pending.finished && !pending.refreshed {
            pending.refreshed = true;
            let pdf_available = pending.results.iter().any(|(_, result)| {
                matches!(
                    result,
                    EpubBatchResult::Saved(..) | EpubBatchResult::Skipped
                )
            });
            if pdf_available {
                self.checked.clear();
                if self.items_are_reading_history_view {
                    self.enter_reading_history();
                } else {
                    self.apply_sort_change_reload_without_ui_io();
                }
            }
        }
    }

    pub(crate) fn show_epub_batch_convert_dialog(&mut self, ctx: &egui::Context) {
        let Some(pending) = self.epub_batch_convert.as_ref() else {
            return;
        };
        if !pending.finished {
            ctx.request_repaint();
        }
        let mut cancel = false;
        let mut close = false;
        egui::Modal::new(egui::Id::new("epub_batch_convert")).show(ctx, |ui| {
            ui.set_min_width(360.0);
            ui.heading("PDF ファイルに変換");
            ui.label(format!("{} / {}", pending.results.len(), pending.total));
            if !pending.finished && !pending.current_name.is_empty() {
                ui.label(&pending.current_name);
            }
            egui::ScrollArea::vertical()
                .max_height(240.0)
                .show(ui, |ui| {
                    for (source, result) in &pending.results {
                        let name = source
                            .file_name()
                            .unwrap_or(source.as_os_str())
                            .to_string_lossy();
                        let result = match result {
                            EpubBatchResult::Saved(destination, warnings)
                                if warnings.is_empty() =>
                            {
                                format!(
                                    "{} を保存しました",
                                    destination
                                        .file_name()
                                        .unwrap_or(destination.as_os_str())
                                        .to_string_lossy()
                                )
                            }
                            EpubBatchResult::Saved(destination, warnings) => format!(
                                "{} を保存しました（設定の引き継ぎに失敗: {}）",
                                destination
                                    .file_name()
                                    .unwrap_or(destination.as_os_str())
                                    .to_string_lossy(),
                                warnings.join("; ")
                            ),
                            EpubBatchResult::Skipped => {
                                "同名の PDF があるためスキップしました".to_owned()
                            }
                            EpubBatchResult::Failed(reason) => format!("失敗: {reason}"),
                        };
                        ui.label(format!("{name}: {result}"));
                    }
                });
            if pending.finished {
                close = ui.button("閉じる").clicked();
            } else if pending.cancel_after_current.load(Ordering::Acquire) {
                ui.label("現在のファイルを処理してから停止します");
            } else {
                cancel = ui.button("キャンセル").clicked();
            }
        });
        if cancel && let Some(pending) = &self.epub_batch_convert {
            pending.cancel_after_current.store(true, Ordering::Release);
        }
        if close {
            self.epub_batch_convert = None;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn epub_batch_uses_clicked_book_when_none_checked_and_filters_mixed_selection() {
        let items = vec![
            GridItem::PdfFile(PathBuf::from("first.pdf")),
            GridItem::PdfFile(PathBuf::from("book.epub")),
            GridItem::ZipFile(PathBuf::from("archive.zip")),
        ];
        assert_eq!(
            collect_epub_targets(&items, [1]),
            vec![PathBuf::from("book.epub")]
        );
        assert_eq!(
            collect_epub_targets(&items, [0, 1, 2, 1]),
            vec![PathBuf::from("book.epub")]
        );

        let mut app = crate::app::setup_app_for_test();
        app.items = items;
        app.selected = Some(1);
        assert_eq!(
            app.selected_epub_batch_paths(None),
            vec![PathBuf::from("book.epub")]
        );
        app.selected = Some(0);
        assert_eq!(
            app.selected_epub_batch_paths(Some(1)),
            vec![PathBuf::from("book.epub")]
        );
        app.checked = [0, 1, 2].into_iter().collect();
        app.selected = Some(1);
        assert_eq!(
            app.selected_epub_batch_paths(None),
            vec![PathBuf::from("book.epub")]
        );
    }

    #[test]
    fn epub_batch_reports_saved_existing_and_failed_in_selection_order() {
        let paths = ["new.epub", "exists.epub", "bad.epub"].map(PathBuf::from);
        let (tx, rx) = mpsc::channel();
        run_batch(
            paths.to_vec(),
            Arc::new(AtomicBool::new(false)),
            tx,
            |source| match source.file_stem().unwrap().to_string_lossy().as_ref() {
                "new" => Ok(epub_convert::SavedPdf {
                    path: source.with_extension("pdf"),
                    reused_cache: false,
                    user_data_errors: vec![],
                }),
                "exists" => Err(EpubConvertError::ExistingPdf),
                _ => Err(EpubConvertError::Invalid),
            },
        );
        let done: Vec<_> = rx
            .try_iter()
            .filter_map(|message| match message {
                EpubBatchMsg::Done(path, result) => Some((path, result)),
                _ => None,
            })
            .collect();
        assert_eq!(done.len(), 3);
        assert!(
            matches!(&done[0].1, EpubBatchResult::Saved(path, _) if path == &PathBuf::from("new.pdf"))
        );
        assert!(matches!(&done[1].1, EpubBatchResult::Skipped));
        assert!(
            matches!(&done[2].1, EpubBatchResult::Failed(message) if message.contains("不正な EPUB"))
        );
    }

    #[test]
    fn epub_batch_cancel_stops_after_current_item() {
        let cancel = Arc::new(AtomicBool::new(false));
        let during = Arc::clone(&cancel);
        let (tx, rx) = mpsc::channel();
        run_batch(
            vec![PathBuf::from("one.epub"), PathBuf::from("two.epub")],
            cancel,
            tx,
            move |source| {
                during.store(true, Ordering::Release);
                Ok(epub_convert::SavedPdf {
                    path: source.with_extension("pdf"),
                    reused_cache: false,
                    user_data_errors: vec![],
                })
            },
        );
        assert_eq!(
            rx.try_iter()
                .filter(|message| matches!(message, EpubBatchMsg::Done(..)))
                .count(),
            1
        );
    }
}
