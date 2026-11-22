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
    Done(PathBuf, Result<epub_convert::SavedPdf, EpubConvertError>),
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
    pub(crate) fn completed_for_test(
        source: PathBuf,
        result: Result<epub_convert::SavedPdf, EpubConvertError>,
    ) -> Self {
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
        let result = save(&source);
        if let Err(error) = tx.send(EpubBatchMsg::Done(source, result))
            && let EpubBatchMsg::Done(_, Ok(saved)) = error.0
        {
            epub_convert::log_saved_pdf_completion_not_received(&saved.path);
        }
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
        let Some(mut pending) = self.epub_batch_convert.take() else {
            return;
        };
        loop {
            match pending.rx.try_recv() {
                Ok(EpubBatchMsg::Start(name)) => pending.current_name = name,
                Ok(EpubBatchMsg::Done(path, result)) => {
                    let result = match result {
                        Ok(saved) => {
                            self.apply_epub_saved_metadata(saved.metadata);
                            EpubBatchResult::Saved(saved.path, saved.user_data_errors)
                        }
                        Err(EpubConvertError::ExistingPdf) => EpubBatchResult::Skipped,
                        Err(error) => {
                            EpubBatchResult::Failed(super::epub_convert::error_message(&error))
                        }
                    };
                    pending.results.push((path, result));
                }
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
        let refresh = pending.finished && !pending.refreshed;
        let pdf_available = pending.results.iter().any(|(_, result)| {
            matches!(
                result,
                EpubBatchResult::Saved(..) | EpubBatchResult::Skipped
            )
        });
        if refresh {
            pending.refreshed = true;
        }
        self.epub_batch_convert = Some(pending);
        if refresh {
            if pdf_available {
                self.checked.clear();
                if self.items_are_reading_history_view {
                    self.enter_reading_history();
                } else {
                    self.apply_sort_change_reload();
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
        egui::Modal::new(egui::Id::unique("epub_batch_convert")).show(ctx, |ui| {
            ui.set_min_width(360.0);
            ui.heading("PDF ファイルに変換");
            ui.label(format!("{} / {}", pending.results.len(), pending.total));
            if !pending.finished && !pending.current_name.is_empty() {
                ui.label(&pending.current_name);
            }
            egui::ScrollArea::vertical()
                .scroll_source(egui::scroll_area::ScrollSource {
                    drag: egui::scroll_area::DragScroll::Always,
                    ..Default::default()
                })
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
    fn section1761_batch_completion_adopts_committed_rotation() {
        let mut app = crate::app::setup_app_for_test();
        let source = app.tmp.path().join("batch.epub");
        let destination = source.with_extension("pdf");
        let source_key = format!("{}::page_1", crate::adjustment_db::normalize_path(&source));
        let destination_key = format!(
            "{}::page_1",
            crate::adjustment_db::normalize_path(&destination)
        );
        app.rotation_db
            .as_ref()
            .unwrap()
            .set_key(&source_key, crate::rotation_db::Rotation::Cw90)
            .unwrap();
        app.update_rotation_page_owner([(source_key, crate::rotation_db::Rotation::Cw90)]);
        app.items = vec![GridItem::PdfFile(source.clone())];
        let data_dir = app.tmp.path().to_path_buf();
        let cancel = Arc::new(AtomicBool::new(false));
        let worker_cancel = Arc::clone(&cancel);
        let (tx, rx) = mpsc::channel();
        // Exercise run_batch's real Done/Finished producer. PDF rendering and
        // publication are omitted; its save callback runs the actual DB-copy tail.
        let epoch = crate::page_edit_write_epoch::TestEpochScope::capture().unwrap();
        std::thread::spawn(move || {
            let _epoch = crate::page_edit_write_epoch::TestEpochScope::enter(epoch);
            run_batch(vec![source], worker_cancel, tx, |source| {
                let saved = epub_convert::copy_saved_pdf_metadata(
                    &data_dir,
                    source,
                    source.with_extension("pdf"),
                    false,
                );
                assert!(
                    saved.user_data_errors.is_empty(),
                    "{:?}",
                    saved.user_data_errors
                );
                Ok(saved)
            });
        })
        .join()
        .unwrap();
        app.epub_batch_convert = Some(EpubBatchPending {
            rx,
            cancel_after_current: cancel,
            total: 1,
            current_name: String::new(),
            results: Vec::new(),
            finished: false,
            refreshed: false,
        });
        app.poll_epub_batch_convert();
        let pending = app.epub_batch_convert.as_ref().unwrap();
        assert!(pending.finished && pending.refreshed);
        assert!(matches!(
            &pending.results[0].1,
            EpubBatchResult::Saved(path, warnings) if path == &destination && warnings.is_empty()
        ));
        assert_eq!(
            app.rotation_db.as_ref().unwrap().get_key(&destination_key),
            Some(crate::rotation_db::Rotation::Cw90),
            "the batch worker committed the copied destination rotation"
        );
        app.items = vec![GridItem::PdfFile(destination)];
        assert_eq!(
            (
                app.rotation_for_page_key(&destination_key),
                app.grid_edit_badges(0).rotation,
            ),
            (crate::rotation_db::Rotation::Cw90, true),
            "batch refresh must adopt the copied rotation into the memory-only cover and badge owner"
        );
    }

    #[test]
    fn section1761_batch_moves_report_and_retains_only_display_result() {
        let mut app = crate::app::setup_app_for_test();
        let source = app.tmp.path().join("display-only.epub");
        let destination = source.with_extension("pdf");
        let source_key = crate::edit_source::page_key_for_pdf(&source, 0);
        let destination_key = crate::edit_source::page_key_for_pdf(&destination, 0);
        app.rotation_db
            .as_ref()
            .unwrap()
            .set_key(&source_key, crate::rotation_db::Rotation::Cw90)
            .unwrap();
        let mut saved = epub_convert::copy_saved_pdf_metadata(
            app.tmp.path(),
            &source,
            destination.clone(),
            false,
        );
        assert!(saved.user_data_errors.is_empty());
        assert_eq!(
            saved
                .metadata
                .presence
                .as_ref()
                .unwrap()
                .rotations
                .get(&destination_key),
            Some(&crate::rotation_db::Rotation::Cw90)
        );
        saved.user_data_errors.push("display warning".into());
        let (tx, rx) = mpsc::channel();
        tx.send(EpubBatchMsg::Done(source.clone(), Ok(saved)))
            .unwrap();
        app.epub_batch_convert = Some(EpubBatchPending {
            rx,
            cancel_after_current: Arc::new(AtomicBool::new(false)),
            total: 1,
            current_name: String::new(),
            results: Vec::new(),
            finished: false,
            refreshed: false,
        });
        app.poll_epub_batch_convert();
        assert_eq!(
            app.rotation_for_page_key(&destination_key),
            crate::rotation_db::Rotation::Cw90
        );
        let pending = app.epub_batch_convert.as_ref().unwrap();
        assert!(!pending.finished && !pending.refreshed);
        assert_eq!(pending.results.len(), 1);
        assert_eq!(pending.results[0].0, source);
        // The completed result has exactly these display fields; it cannot retain
        // the worker's page-key report after its ownership moved into App.
        let EpubBatchResult::Saved(path, warnings) = &pending.results[0].1 else {
            panic!("expected the saved display result");
        };
        assert_eq!(path, &destination);
        assert_eq!(warnings, &["display warning".to_string()]);
    }

    #[test]
    fn section1761_batch_each_done_adopts_before_finished_reload() {
        let mut app = crate::app::setup_app_for_test();
        let data = app.tmp.path().to_path_buf();
        let sources = [data.join("first.epub"), data.join("second.epub")];
        for source in &sources {
            let key = crate::edit_source::page_key_for_pdf(source, 0);
            app.rotation_db
                .as_ref()
                .unwrap()
                .set_key(&key, crate::rotation_db::Rotation::Cw90)
                .unwrap();
        }
        let (worker_tx, worker_rx) = mpsc::channel();
        let worker_sources = sources.clone();
        let epoch = crate::page_edit_write_epoch::TestEpochScope::capture().unwrap();
        std::thread::spawn(move || {
            let _epoch = crate::page_edit_write_epoch::TestEpochScope::enter(epoch);
            run_batch(
                worker_sources.to_vec(),
                Arc::new(AtomicBool::new(false)),
                worker_tx,
                |source| {
                    Ok(epub_convert::copy_saved_pdf_metadata(
                        &data,
                        source,
                        source.with_extension("pdf"),
                        false,
                    ))
                },
            );
        })
        .join()
        .unwrap();
        // Deliver only one Done at a time, keeping Finished queued outside the UI owner.
        let (tx, rx) = mpsc::channel();
        app.epub_batch_convert = Some(EpubBatchPending {
            rx,
            cancel_after_current: Arc::new(AtomicBool::new(false)),
            total: 2,
            current_name: String::new(),
            results: Vec::new(),
            finished: false,
            refreshed: false,
        });
        let mut done = 0;
        for message in worker_rx.try_iter() {
            if matches!(&message, EpubBatchMsg::Done(..)) {
                done += 1;
                tx.send(message).unwrap();
                app.poll_epub_batch_convert();
                let pending = app.epub_batch_convert.as_ref().unwrap();
                assert!(!pending.finished && !pending.refreshed);
                assert_eq!(pending.results.len(), done);
                for (index, source) in sources.iter().enumerate() {
                    let key =
                        crate::edit_source::page_key_for_pdf(&source.with_extension("pdf"), 0);
                    assert_eq!(
                        app.rotation_for_page_key(&key),
                        if index < done {
                            crate::rotation_db::Rotation::Cw90
                        } else {
                            crate::rotation_db::Rotation::None
                        }
                    );
                }
            }
        }
        assert_eq!(done, 2);
        tx.send(EpubBatchMsg::Finished).unwrap();
        app.poll_epub_batch_convert();
        assert!(app.epub_batch_convert.as_ref().unwrap().refreshed);
    }

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
                    metadata: Default::default(),
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
        assert!(matches!(&done[0].1, Ok(saved) if saved.path == PathBuf::from("new.pdf")));
        assert!(matches!(&done[1].1, Err(EpubConvertError::ExistingPdf)));
        assert!(matches!(&done[2].1, Err(EpubConvertError::Invalid)));
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
                    metadata: Default::default(),
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
