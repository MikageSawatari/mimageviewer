//! Automatic observations and refresh results stay with the existing append/scan owners.
use super::*;
use crate::book_append_undo::{AppendOperation, BookAppendUndoReceipt};

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct BookAppendRefresh {
    pub(crate) operation: AppendOperation,
    pub(crate) adoption_owner: ViewerContextId,
    pub(crate) folder: PathBuf,
    pub(crate) items_generation: u64,
    pub(crate) scan_options: ExternalRescanOptions,
    pub(crate) observed_folder_mtime: Option<std::time::SystemTime>,
}

#[derive(Clone, Debug, Default)]
pub(crate) enum FolderRefreshCause {
    #[default]
    External,
    /// General refresh returned by Append; no self-operation exception.
    /// It retains the existing deferred delivery across general Delete.
    BookAppendExternal,
    BookAppend(BookAppendUndoReceipt),
}

impl FolderRefreshCause {
    pub(super) fn operation(&self) -> Option<AppendOperation> {
        match self {
            Self::External | Self::BookAppendExternal => None,
            Self::BookAppend(receipt) => Some(receipt.operation()),
        }
    }
    // All filesystem inspection stays on the scan worker. A changed composition
    // is an ordinary external refresh, never an exception to receipt invalidation.
    pub(super) fn verified_for_folder(self, folder: &Path) -> Self {
        match self {
            Self::BookAppend(receipt)
                if crate::folder_tree::path_eq(folder, &receipt.files.folder)
                    && receipt.files.verify_current().is_ok() =>
            {
                Self::BookAppend(receipt)
            }
            Self::BookAppendExternal => Self::BookAppendExternal,
            Self::BookAppend(_) => Self::BookAppendExternal,
            Self::External => Self::External,
        }
    }
    pub(super) fn is_append_delivery(&self) -> bool {
        !matches!(self, Self::External)
    }
}

#[derive(Clone, Debug)]
pub(crate) struct FolderRefreshRequest {
    pub(crate) folder: PathBuf,
    pub(crate) cause: FolderRefreshCause,
}
impl From<PathBuf> for FolderRefreshRequest {
    fn from(folder: PathBuf) -> Self {
        Self {
            folder,
            cause: FolderRefreshCause::External,
        }
    }
}

impl App {
    pub(super) fn book_append_refresh_request(
        &self,
        operation: AppendOperation,
        folder: PathBuf,
    ) -> BookAppendRefresh {
        BookAppendRefresh {
            operation,
            folder,
            adoption_owner: self.projected_viewer_context_id(),
            items_generation: self.items_generation,
            scan_options: self.current_external_scan_options(),
            observed_folder_mtime: None,
        }
    }
    pub(super) fn current_external_scan_options(&self) -> ExternalRescanOptions {
        ExternalRescanOptions {
            file_type_visibility: self.settings.file_type_visibility.clone(),
            include_convertible_archives: !self
                .settings
                .archive_file_handling_ignores_convertible(),
            include_epub: !self.settings.epub_file_handling_ignores_epub(),
            show_hidden_files: self.settings.show_hidden_files,
        }
    }
    pub(super) fn book_append_refresh_is_current(&self, refresh: &BookAppendRefresh) -> bool {
        if refresh.adoption_owner != self.projected_viewer_context_id()
            || refresh.items_generation != self.items_generation
            || refresh.scan_options != self.current_external_scan_options()
            || !self.book_append_refresh_folder_is_current(&refresh.folder)
            || !self.book_append_operation_is_live(refresh.operation)
        {
            return false;
        }
        let expected = match self.book_op_pending.as_ref().map(|pending| &pending.intent) {
            Some(crate::books::BookOpIntent::Append { target_folder, .. }) => Some(target_folder),
            _ => self
                .book_append_undo_receipt
                .as_ref()
                .map(|receipt| &receipt.files.folder),
        };
        expected.is_some_and(|folder| crate::folder_tree::path_eq(folder, &refresh.folder))
    }
    fn book_append_refresh_folder_is_current(&self, folder: &Path) -> bool {
        self.current_folder
            .as_ref()
            .is_some_and(|current| crate::folder_tree::path_eq(current, folder))
            && !self.items_are_global_search_view
            && !self.global_search.active
            && !self.items_are_tag_view
            && !self.tag_view.on_results_grid()
            && !self.favsearch.on_results_grid()
    }
    pub(super) fn defer_book_append_observation(
        &mut self,
        folder: &Path,
        context: ViewerContextId,
    ) -> bool {
        let Some(pending) = self.book_op_pending.as_mut() else {
            return false;
        };
        let crate::books::BookOpIntent::Append {
            target_folder,
            observations,
            ..
        } = &mut pending.intent
        else {
            return false;
        };
        if !crate::folder_tree::path_eq(folder, target_folder) {
            return false;
        }
        observations.record(context);
        true
    }

    pub(super) fn cancel_book_append_readonly_refresh(&mut self) {
        let Some(receipt) = self.book_append_undo_receipt.as_ref() else {
            return;
        };
        if self
            .external_rescan_pending
            .as_ref()
            .is_some_and(|scan| crate::folder_tree::path_eq(&scan.folder, &receipt.files.folder))
        {
            let scan = self.external_rescan_pending.take().unwrap();
            scan.cancel.store(true, Ordering::Relaxed);
        }
        if self
            .folder_refresh_pending
            .as_ref()
            .is_some_and(|request| request.cause.operation() == Some(receipt.operation()))
        {
            self.folder_refresh_pending = None;
        }
        // Existing generation/cancel owners reject readonly results after Undo.
        self.cancel_pending_folder_nav();
        if self
            .top_level_grid_view
            .open_path_classification()
            .is_some_and(|request| request.context == self.projected_viewer_context_id())
        {
            self.top_level_grid_view.set_open_path_classification(None);
        }
    }

    pub(super) fn finish_book_append_terminal(
        &mut self,
        ctx: &egui::Context,
        mut summary: crate::books::BookAppendSummary,
        scan: Option<(
            ScannedDir,
            crate::book_append_undo::BookCompositionStamp,
            BookAppendRefresh,
        )>,
    ) -> bool {
        let Some(crate::books::BookOpIntent::Append {
            operation_id,
            source_context,
            accepted_revision,
            target_folder,
            ..
        }) = self.book_op_pending.as_ref().map(|pending| &pending.intent)
        else {
            return false;
        };
        let operation = AppendOperation {
            operation_id: *operation_id,
            source_context: *source_context,
            accepted_revision: *accepted_revision,
        };
        if !crate::folder_tree::path_eq(target_folder, &summary.folder) {
            summary.undo_files = None;
        }
        let same_context = operation.source_context == self.projected_viewer_context_id();
        let current_target =
            same_context && self.book_append_refresh_folder_is_current(&summary.folder);
        let valid = operation.accepted_revision == self.book_append_undo_revision && same_context;
        if !valid {
            summary.undo_files = None;
        }

        if current_target
            && self.fullscreen_idx.is_none()
            && !self.viewer_session_blocks_main_window()
            && scan.is_none()
        {
            self.start_book_append_refresh_terminal(ctx, summary);
            return true; // receiver changed, same Append intent remains owned
        }

        self.finish_book_append_metadata(&summary);
        let receipt = summary
            .undo_files
            .clone()
            .map(|files| BookAppendUndoReceipt {
                files,
                operation_id: operation.operation_id,
                source_context: operation.source_context,
                accepted_revision: operation.accepted_revision,
            });
        if let Some((scan, composition, request)) = scan {
            if !self.book_append_refresh_is_current(&request) {
                summary.undo_files = None;
                if let Some(crate::books::BookOpIntent::Append { observations, .. }) = self
                    .book_op_pending
                    .as_mut()
                    .map(|pending| &mut pending.intent)
                {
                    observations.record(request.adoption_owner);
                }
            } else {
                let explained = receipt
                    .as_ref()
                    .is_some_and(|receipt| receipt.files.verify_composition(&composition).is_ok());
                let observed_folder_mtime = request.observed_folder_mtime;
                let mut refresh = self.current_list_refresh_intent();
                if explained
                    && self.book_append_operation_is_current(operation)
                    && let StartupListIntent::RefreshList {
                        book_append_refresh,
                        ..
                    } = &mut refresh
                {
                    *book_append_refresh = Some(request);
                }
                let owner = self.current_folder_reload_owner(&summary.folder);
                if current_target {
                    let signature = signature_from_scan(&scan);
                    let accepted = if self
                        .current_folder_signature
                        .is_some_and(|old| old.same_listing(signature))
                    {
                        self.current_folder_signature = Some(signature);
                        self.current_folder_last_mtime =
                            observed_folder_mtime.or(self.current_folder_last_mtime);
                        true
                    } else {
                        self.load_folder_with_scan_owned(
                            summary.folder.clone(),
                            Some(scan),
                            owner,
                            refresh,
                        )
                    };
                    if !explained || !accepted {
                        summary.undo_files = None;
                    }
                } else {
                    summary.undo_files = None;
                }
            }
        } else if current_target {
            self.folder_refresh_pending = Some(FolderRefreshRequest {
                folder: summary.folder.clone(),
                cause: receipt
                    .clone()
                    .map(FolderRefreshCause::BookAppend)
                    .unwrap_or_default(),
            });
        }
        // Publish only after metadata and the immediate visible adoption terminal.
        if operation.accepted_revision == self.book_append_undo_revision && same_context {
            self.book_append_undo_receipt =
                summary
                    .undo_files
                    .clone()
                    .map(|files| BookAppendUndoReceipt {
                        files,
                        operation_id: operation.operation_id,
                        source_context: operation.source_context,
                        accepted_revision: operation.accepted_revision,
                    });
        }
        if let Some(rows) = self.book_list_cache.as_mut() {
            crate::books::apply_append_to_cached_list(
                rows,
                &summary.book_name,
                &summary.folder,
                summary.added,
            );
        }
        crate::logger::log(format!(
            "book append: {} added={} folder={}",
            summary.book_name,
            summary.added,
            summary.folder.display()
        ));
        let mut message = format!(
            "「{}」に {} ページ追加しました",
            summary.book_name, summary.added
        );
        if summary.erase_fallback_pages > 0 {
            message.push_str(&format!(
                "\n{} ページの消しゴム補完は簡易処理で再作成しました",
                summary.erase_fallback_pages
            ));
        }
        if summary.undo_files.is_none() {
            message.push_str("\nこの追加は取り消せません");
        }
        self.show_feedback_toast_with_duration(message, 4.5);
        false
    }

    fn start_book_append_refresh_terminal(
        &mut self,
        ctx: &egui::Context,
        summary: crate::books::BookAppendSummary,
    ) {
        let visibility = self.settings.file_type_visibility.clone();
        let archives = !self.settings.archive_file_handling_ignores_convertible();
        let epub = !self.settings.epub_file_handling_ignores_epub();
        let hidden = self.settings.show_hidden_files;
        let operation = match &self.book_op_pending.as_ref().expect("Append owner").intent {
            crate::books::BookOpIntent::Append {
                operation_id,
                source_context,
                accepted_revision,
                ..
            } => AppendOperation {
                operation_id: *operation_id,
                source_context: *source_context,
                accepted_revision: *accepted_revision,
            },
            _ => unreachable!("Append refresh"),
        };
        let mut request = self.book_append_refresh_request(operation, summary.folder.clone());
        let payload = Arc::new(std::sync::Mutex::new(Some(summary)));
        let worker_payload = Arc::clone(&payload);
        let repaint = ctx.clone();
        let (tx, rx) = mpsc::channel();
        match std::thread::Builder::new()
            .name("book-append-refresh".into())
            .spawn(move || {
                let mut summary = worker_payload
                    .lock()
                    .unwrap()
                    .take()
                    .expect("one refresh payload");
                let result = (|| {
                    let before =
                        crate::book_append_undo::capture_book_composition(&summary.folder)?;
                    let scan = folder_scan::scan_directory_with_visibility_cancel(
                        &summary.folder,
                        archives,
                        epub,
                        hidden,
                        &visibility,
                        None,
                    )
                    .map_err(|error| error.to_string())?;
                    let composition =
                        crate::book_append_undo::capture_book_composition(&summary.folder)?;
                    if before != composition {
                        summary.undo_files = None;
                    }
                    request.observed_folder_mtime = summary
                        .folder
                        .metadata()
                        .and_then(|meta| meta.modified())
                        .ok();
                    Ok(crate::books::BookOpResult::AppendRefreshPrepared {
                        summary,
                        scan,
                        composition,
                        request,
                    })
                })();
                let _ = tx.send(result);
                repaint.request_repaint();
            }) {
            Ok(_) => {
                self.book_op_pending.as_mut().expect("Append owner").rx = rx;
            }
            Err(error) => {
                crate::logger::log(format!("book append refresh worker: {error}"));
                let mut fallback = payload
                    .lock()
                    .unwrap()
                    .take()
                    .expect("failed spawn retains payload");
                fallback.undo_files = None;
                self.finish_book_append_metadata(&fallback);
                let intent = self
                    .book_op_pending
                    .as_ref()
                    .expect("Append owner")
                    .intent
                    .clone();
                self.book_op_pending = None;
                self.folder_refresh_pending = Some(FolderRefreshRequest {
                    folder: fallback.folder,
                    cause: FolderRefreshCause::BookAppendExternal,
                });
                self.release_book_append_observations(ctx, &intent);
                self.show_feedback_toast(
                    "追加後の一覧を更新できませんでした。追加の取り消しは利用できません".into(),
                );
            }
        }
    }

    pub(super) fn release_book_append_observations(
        &mut self,
        ctx: &egui::Context,
        intent: &crate::books::BookOpIntent,
    ) {
        let crate::books::BookOpIntent::Append {
            target_folder,
            source_context,
            observations,
            ..
        } = intent
        else {
            return;
        };
        // The current watcher/resume/scan producer is Main's projected owner;
        // external_rescan_pending is App-global. Never mount sibling contexts
        // or cancel their work to turn this terminal into a new fanout producer.
        if (observations
            .contexts()
            .contains(&self.edit_request_owner_context())
            || *source_context != self.projected_viewer_context_id())
            && self.book_append_refresh_folder_is_current(target_folder)
        {
            let cause = self
                .book_append_undo_receipt
                .as_ref()
                .filter(|receipt| {
                    receipt.accepted_revision == self.book_append_undo_revision
                        && crate::folder_tree::path_eq(&receipt.files.folder, target_folder)
                })
                .cloned()
                .map(FolderRefreshCause::BookAppend)
                .unwrap_or(FolderRefreshCause::BookAppendExternal);
            if self.delete_pending.is_some() {
                self.folder_refresh_pending = Some(FolderRefreshRequest {
                    folder: target_folder.clone(),
                    cause,
                });
                return;
            }
            self.start_external_rescan_with_cause(
                ctx,
                target_folder.clone(),
                std::time::SystemTime::UNIX_EPOCH,
                cause,
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::book_append_undo::{AppendObservations, AppendReceiptBuilder};
    use crate::books::{BookAppendSummary, BookOpIntent, BookOpPending, BookOpResult};

    fn fixture() -> (AppTestEnvForTest, BookAppendUndoReceipt, BookAppendSummary) {
        let mut app = setup_app_for_test();
        let root = app.tmp.path().join("refresh-books");
        let folder = root.join("book");
        std::fs::create_dir_all(&folder).unwrap();
        let mut builder = AppendReceiptBuilder::begin(&root, &folder, "book".into()).unwrap();
        let page = folder.join("0001_page.jpg");
        std::fs::write(&page, b"generated").unwrap();
        builder.record(&page).unwrap();
        let receipt = BookAppendUndoReceipt {
            files: builder.finish().unwrap(),
            operation_id: 42,
            source_context: app.projected_viewer_context_id(),
            accepted_revision: app.book_append_undo_revision,
        };
        app.current_folder = Some(folder.clone());
        let summary = BookAppendSummary {
            book_name: "book".into(),
            folder,
            added: 1,
            first_path: Some(page),
            erase_fallback_pages: 0,
            edit_copies: vec![],
            semantic_copies: vec![],
            undo_files: Some(receipt.files.clone()),
            metadata_copy: Default::default(),
        };
        (app, receipt, summary)
    }

    fn hold(
        app: &mut App,
        receipt: &BookAppendUndoReceipt,
    ) -> mpsc::Sender<Result<BookOpResult, String>> {
        let (tx, rx) = mpsc::channel();
        app.book_op_pending = Some(BookOpPending {
            rx,
            intent: BookOpIntent::Append {
                operation_id: receipt.operation_id,
                source_context: receipt.source_context,
                accepted_revision: receipt.accepted_revision,
                target_folder: receipt.files.folder.clone(),
                observations: AppendObservations::default(),
            },
        });
        tx
    }

    fn scan(folder: &Path) -> ScannedDir {
        folder_scan::scan_directory_with_visibility_cancel(
            folder,
            true,
            true,
            true,
            &Default::default(),
            None,
        )
        .unwrap()
    }

    #[test]
    fn section1373_automatic_request_exact_folder_stays_with_append_before_io() {
        let (mut app, receipt, _) = fixture();
        let _tx = hold(&mut app, &receipt);
        app.check_external_folder_changes(&egui::Context::default(), ExternalChangeCheck::Notified);
        assert!(app.external_rescan_pending.is_none());
        assert!(!app.defer_book_append_observation(
            &receipt.files.folder.join("child"),
            receipt.source_context
        ));
        let BookOpIntent::Append { observations, .. } =
            &app.book_op_pending.as_ref().unwrap().intent
        else {
            panic!()
        };
        assert_eq!(observations.contexts(), &[receipt.source_context]);
    }

    #[test]
    fn section1373_preappend_scan_completion_is_deferred_without_operation_identity() {
        let (mut app, receipt, _) = fixture();
        let scan = scan(&receipt.files.folder);
        let signature = signature_from_scan(&scan);
        let (tx, rx) = mpsc::channel();
        tx.send(Some((scan, signature, FolderRefreshCause::External)))
            .unwrap();
        app.external_rescan_pending = Some(ExternalRescanPending {
            owner_context_id: app.edit_request_owner_context(),
            folder: receipt.files.folder.clone(),
            mtime: std::time::SystemTime::UNIX_EPOCH,
            items_generation: app.items_generation,
            scan_options: ExternalRescanOptions {
                file_type_visibility: app.settings.file_type_visibility.clone(),
                include_convertible_archives: true,
                include_epub: true,
                show_hidden_files: true,
            },
            cancel: Arc::new(AtomicBool::new(false)),
            rx,
            restart_requested: false,
            cause: FolderRefreshCause::External,
        });
        let _append = hold(&mut app, &receipt);
        let generation = app.items_generation;
        app.poll_external_rescan(&egui::Context::default());
        assert_eq!(app.items_generation, generation);
        assert!(app.current_folder_signature.is_none());
        let BookOpIntent::Append { observations, .. } =
            &app.book_op_pending.as_ref().unwrap().intent
        else {
            panic!()
        };
        assert_eq!(observations.contexts(), &[receipt.source_context]);
    }

    #[test]
    fn section1373_immediate_refresh_publishes_receipt_after_adoption() {
        let (mut app, receipt, summary) = fixture();
        let tx = hold(&mut app, &receipt);
        let scan = scan(&receipt.files.folder);
        tx.send(Ok(BookOpResult::AppendRefreshPrepared {
            summary,
            scan,
            composition: receipt.files.composition.clone(),
            request: app
                .book_append_refresh_request(receipt.operation(), receipt.files.folder.clone()),
        }))
        .unwrap();
        app.poll_book_op_pending(&egui::Context::default());
        assert!(app.book_op_pending.is_none());
        assert!(app.book_append_operation_matches_receipt(receipt.operation()));
        assert_eq!(app.items.len(), 1);
        assert!(app.current_folder_signature.is_some());
    }

    #[test]
    fn section1373_external_composition_change_never_publishes_self_receipt() {
        let (mut app, receipt, summary) = fixture();
        let tx = hold(&mut app, &receipt);
        std::fs::write(receipt.files.folder.join("outside.jpg"), b"external").unwrap();
        let scan = scan(&receipt.files.folder);
        let composition =
            crate::book_append_undo::capture_book_composition(&receipt.files.folder).unwrap();
        tx.send(Ok(BookOpResult::AppendRefreshPrepared {
            summary,
            scan,
            composition,
            request: app
                .book_append_refresh_request(receipt.operation(), receipt.files.folder.clone()),
        }))
        .unwrap();
        app.poll_book_op_pending(&egui::Context::default());
        assert!(app.book_append_undo_receipt.is_none());
        assert_eq!(app.items.len(), 2);
    }

    #[test]
    fn section1373_fullscreen_append_retires_into_typed_deferred_refresh() {
        let (mut app, receipt, summary) = fixture();
        let tx = hold(&mut app, &receipt);
        app.fullscreen_idx = Some(0);
        app.viewer_presentation = ViewerPresentation::Fullscreen;
        tx.send(Ok(BookOpResult::Append(summary))).unwrap();
        app.poll_book_op_pending(&egui::Context::default());
        assert!(app.book_op_pending.is_none());
        assert!(app.book_append_operation_matches_receipt(receipt.operation()));
        let request = app.folder_refresh_pending.as_ref().unwrap();
        assert_eq!(request.cause.operation(), Some(receipt.operation()));
        assert_eq!(app.fullscreen_idx, Some(0));
        app.cancel_book_append_readonly_refresh();
        assert!(app.folder_refresh_pending.is_none());
    }

    #[test]
    fn section1373_changed_hidden_composition_downgrades_deferred_cause() {
        let (_app, receipt, _) = fixture();
        assert!(matches!(
            FolderRefreshCause::BookAppend(receipt.clone())
                .verified_for_folder(&receipt.files.folder),
            FolderRefreshCause::BookAppend(_)
        ));
        std::fs::write(receipt.files.folder.join("sidecar.txt"), b"external").unwrap();
        assert!(matches!(
            FolderRefreshCause::BookAppend(receipt.clone())
                .verified_for_folder(&receipt.files.folder),
            FolderRefreshCause::BookAppendExternal
        ));
    }

    #[test]
    fn section1373_invalidated_fullscreen_receipt_still_dispatches_general_refresh() {
        let (mut app, receipt, _) = fixture();
        app.book_append_undo_receipt = Some(receipt.clone());
        app.folder_refresh_pending = Some(FolderRefreshRequest {
            folder: receipt.files.folder.clone(),
            cause: FolderRefreshCause::BookAppend(receipt),
        });
        app.clear_meta_undo(); // ordinary fullscreen close uses this boundary
        app.consume_folder_refresh_pending(&egui::Context::default());
        assert!(app.folder_refresh_pending.is_none());
        let pending = app.external_rescan_pending.as_ref().unwrap();
        assert!(matches!(
            pending.cause,
            FolderRefreshCause::BookAppendExternal
        ));
    }

    #[test]
    fn section1373_newer_mutation_prevents_late_append_receipt() {
        let (mut app, receipt, summary) = fixture();
        let tx = hold(&mut app, &receipt);
        app.invalidate_book_append_undo();
        let scan = scan(&receipt.files.folder);
        let request =
            app.book_append_refresh_request(receipt.operation(), receipt.files.folder.clone());
        tx.send(Ok(BookOpResult::AppendRefreshPrepared {
            summary,
            scan,
            composition: receipt.files.composition.clone(),
            request: app
                .book_append_refresh_request(receipt.operation(), receipt.files.folder.clone()),
        }))
        .unwrap();
        app.poll_book_op_pending(&egui::Context::default());
        assert!(app.book_append_undo_receipt.is_none());
    }

    #[test]
    fn section1373_unchanged_visible_projection_keeps_generation_and_receipt() {
        let (mut app, receipt, summary) = fixture();
        app.settings
            .file_type_visibility
            .excluded_categories
            .insert(crate::file_type_visibility::FileTypeCategory::Image);
        let scan = folder_scan::scan_directory_with_visibility_cancel(
            &receipt.files.folder,
            true,
            true,
            false,
            &app.settings.file_type_visibility,
            None,
        )
        .unwrap();
        app.current_folder_signature = Some(signature_from_scan(&scan));
        let generation = app.items_generation;
        let request =
            app.book_append_refresh_request(receipt.operation(), receipt.files.folder.clone());
        let tx = hold(&mut app, &receipt);
        tx.send(Ok(BookOpResult::AppendRefreshPrepared {
            summary,
            scan,
            composition: receipt.files.composition.clone(),
            request,
        }))
        .unwrap();
        app.poll_book_op_pending(&egui::Context::default());
        assert_eq!(app.items_generation, generation);
        assert!(app.items.is_empty());
        assert!(app.book_append_operation_matches_receipt(receipt.operation()));
    }

    #[test]
    fn section1373_old_scan_cannot_restore_deleted_rows_or_old_visibility() {
        for cause in ["delete", "visibility"] {
            let (mut app, receipt, summary) = fixture();
            let request =
                app.book_append_refresh_request(receipt.operation(), receipt.files.folder.clone());
            let old_scan = scan(&receipt.files.folder);
            let tx = hold(&mut app, &receipt);
            app.items = vec![GridItem::Image(receipt.files.folder.join("newer.jpg"))];
            if cause == "delete" {
                app.items_generation = app.items_generation.wrapping_add(1);
            } else {
                app.settings
                    .file_type_visibility
                    .excluded_categories
                    .insert(crate::file_type_visibility::FileTypeCategory::Image);
            }
            let generation = app.items_generation;
            tx.send(Ok(BookOpResult::AppendRefreshPrepared {
                summary,
                scan: old_scan,
                composition: receipt.files.composition.clone(),
                request,
            }))
            .unwrap();
            app.poll_book_op_pending(&egui::Context::default());
            assert_eq!(app.items_generation, generation, "{cause}");
            assert_eq!(
                app.items[0].drag_source_path(),
                Some(receipt.files.folder.join("newer.jpg").as_path()),
                "{cause}"
            );
            assert!(app.book_append_undo_receipt.is_none());
            assert!(
                app.external_rescan_pending.is_some(),
                "{cause}: stale result returns to current prepare owner"
            );
        }
    }

    #[cfg(windows)]
    #[test]
    fn section1373_detached_source_terminal_and_main_refresh_keep_separate_owners() {
        let (mut app, mut receipt, summary) = fixture();
        let main = app.projected_viewer_context_id();
        let source_folder = app.tmp.path().join("source");
        std::fs::create_dir_all(&source_folder).unwrap();
        let other = app
            .build_viewer_context("book_append_other", |owner, _| {
                owner.current_folder = Some(source_folder.clone());
                owner.navigation_scope = ViewerNavigationScope::DetachedPhysical;
                owner.fullscreen_idx = Some(0);
                owner.viewer_presentation = ViewerPresentation::Fullscreen;
                BuildOutcome::Commit
            })
            .unwrap();
        receipt.source_context = other;
        receipt.accepted_revision = app.book_append_undo_revision;
        let tx = hold(&mut app, &receipt);
        let generation = app.items_generation;
        tx.send(Ok(BookOpResult::Append(summary))).unwrap();
        app.poll_book_op_pending(&egui::Context::default());
        assert_eq!(app.projected_viewer_context_id(), main);
        assert_eq!(app.items_generation, generation);
        assert!(app.book_append_operation_matches_receipt(receipt.operation()));
        let pending = app.external_rescan_pending.as_ref().unwrap();
        assert_eq!(pending.owner_context_id, main);
        assert_eq!(pending.cause.operation(), Some(receipt.operation()));
        let qualification =
            app.book_append_refresh_request(receipt.operation(), receipt.files.folder.clone());
        assert!(app.book_append_refresh_is_current(&qualification));
        assert!(!app.book_append_refresh_is_current(&BookAppendRefresh {
            adoption_owner: other,
            ..qualification
        }));
        // Complete the actual Main scan consumer, rather than only checking its
        // request header. Source A stays parked while B adopts the fresh rows.
        let mut pending = app.external_rescan_pending.take().unwrap();
        pending.cancel.store(true, Ordering::Relaxed);
        let fresh = scan(&receipt.files.folder);
        let signature = signature_from_scan(&fresh);
        let (scan_tx, scan_rx) = mpsc::channel();
        scan_tx
            .send(Some((
                fresh,
                signature,
                FolderRefreshCause::BookAppend(receipt.clone()),
            )))
            .unwrap();
        pending.rx = scan_rx;
        pending.cancel = Arc::new(AtomicBool::new(false));
        app.external_rescan_pending = Some(pending);
        app.poll_external_rescan(&egui::Context::default());
        assert!(app.external_rescan_pending.is_none());
        assert_eq!(app.projected_viewer_context_id(), main);
        assert!(app.items_generation > generation);
        assert!(
            app.items.iter().any(
                |item| item.drag_source_path() == Some(receipt.files.created[0].path.as_path())
            )
        );
        assert!(app.book_append_operation_matches_receipt(receipt.operation()));
        assert!(
            app.with_viewer_context_ref(other, |source| source
                .current_folder()
                .as_ref()
                .is_some_and(|folder| crate::folder_tree::path_eq(folder, &source_folder)))
                .unwrap()
        );
    }

    #[test]
    fn section1373_closed_source_preserves_success_and_newer_sibling_draft() {
        let (mut app, mut receipt, mut summary) = fixture();
        receipt.source_context = ViewerContextId::for_test(u64::MAX);
        summary.edit_copies.push(crate::books::BookPathMapping {
            from: PathBuf::from("source.jpg"),
            to: receipt.files.created[0].path.clone(),
        });
        app.erase_mask = Some(vec![true, false]);
        let tx = hold(&mut app, &receipt);
        let generation = app.items_generation;
        tx.send(Ok(BookOpResult::Append(summary))).unwrap();
        app.poll_book_op_pending(&egui::Context::default());
        assert!(app.book_append_undo_receipt.is_none());
        assert!(app.book_op_pending.is_none());
        assert_eq!(app.erase_mask, Some(vec![true, false]));
        assert_eq!(app.items_generation, generation);
        assert!(app.external_rescan_pending.is_some()); // Main's target gets ordinary latest refresh
    }

    #[test]
    fn section1373_append_return_refresh_survives_both_general_delete_orders() {
        for case in ["in_flight", "delete_done"] {
            let (mut app, receipt, _) = fixture();
            let scan = scan(&receipt.files.folder);
            let signature = signature_from_scan(&scan);
            let (scan_tx, scan_rx) = mpsc::channel();
            scan_tx
                .send(Some((
                    scan,
                    signature,
                    FolderRefreshCause::BookAppendExternal,
                )))
                .unwrap();
            app.external_rescan_pending = Some(ExternalRescanPending {
                owner_context_id: app.projected_viewer_context_id(),
                folder: receipt.files.folder.clone(),
                mtime: std::time::SystemTime::UNIX_EPOCH,
                items_generation: app.items_generation,
                scan_options: app.current_external_scan_options(),
                cancel: Arc::new(AtomicBool::new(false)),
                rx: scan_rx,
                restart_requested: false,
                cause: FolderRefreshCause::BookAppendExternal,
            });
            let (_delete_tx, delete_rx) = mpsc::channel();
            app.delete_pending = Some(crate::delete_worker::DeletePending {
                cancel: Arc::new(AtomicBool::new(false)),
                rx: delete_rx,
                total: 1,
                succeeded: vec![],
                failed: vec![],
                purged_pdf_password_paths: vec![],
                purge_deferred: false,
                source_scopes: vec![],
            });
            if case == "delete_done" {
                app.delete_pending = None;
                app.items_generation = app.items_generation.wrapping_add(1);
            }
            app.poll_external_rescan(&egui::Context::default());
            assert!(app.external_rescan_pending.is_none(), "{case}");
            assert!(
                app.folder_refresh_pending
                    .as_ref()
                    .unwrap()
                    .cause
                    .is_append_delivery(),
                "{case}"
            );
            app.consume_folder_refresh_pending(&egui::Context::default());
            if case == "in_flight" {
                assert!(app.folder_refresh_pending.is_some());
                assert!(app.external_rescan_pending.is_none());
                app.delete_pending = None;
                app.consume_folder_refresh_pending(&egui::Context::default());
            }
            assert!(app.folder_refresh_pending.is_none());
            assert!(matches!(
                app.external_rescan_pending.as_ref().unwrap().cause,
                FolderRefreshCause::BookAppendExternal
            ));
        }
    }

    #[test]
    fn section1373_append_delivery_replaces_preappend_scan_instead_of_borrowing_its_result() {
        let (mut app, receipt, _) = fixture();
        let (_tx, rx) = mpsc::channel();
        let cancel = Arc::new(AtomicBool::new(false));
        app.external_rescan_pending = Some(ExternalRescanPending {
            owner_context_id: app.projected_viewer_context_id(),
            folder: receipt.files.folder.clone(),
            mtime: std::time::SystemTime::UNIX_EPOCH,
            items_generation: app.items_generation,
            scan_options: app.current_external_scan_options(),
            cancel: Arc::clone(&cancel),
            rx,
            restart_requested: false,
            cause: FolderRefreshCause::External,
        });
        app.book_append_undo_receipt = Some(receipt.clone());
        app.start_external_rescan_with_cause(
            &egui::Context::default(),
            receipt.files.folder.clone(),
            std::time::SystemTime::UNIX_EPOCH,
            FolderRefreshCause::BookAppend(receipt.clone()),
        );
        assert!(cancel.load(Ordering::Relaxed));
        assert_eq!(
            app.external_rescan_pending
                .as_ref()
                .unwrap()
                .cause
                .operation(),
            Some(receipt.operation())
        );
    }
}
