use super::*;

impl App {
    pub(crate) fn book_append_undo_in_progress(&self) -> bool {
        self.book_op_pending.as_ref().is_some_and(|pending| {
            matches!(
                pending.intent,
                crate::books::BookOpIntent::UndoAppend { .. }
            )
        })
    }

    pub(crate) fn invalidate_book_append_undo(&mut self) {
        self.book_append_undo_receipt = None;
        self.book_append_undo_revision = self.book_append_undo_revision.wrapping_add(1);
    }

    pub(crate) fn admit_book_append_followup_mutation(&mut self) -> bool {
        if self.book_append_undo_in_progress() {
            self.show_feedback_toast("本への追加を取り消しています".into());
            return false;
        }
        self.invalidate_book_append_undo();
        true
    }

    pub(crate) fn book_append_operation_matches_receipt(
        &self,
        operation: crate::book_append_undo::AppendOperation,
    ) -> bool {
        self.book_append_undo_receipt
            .as_ref()
            .is_some_and(|receipt| {
                receipt.operation() == operation
                    && receipt.accepted_revision == self.book_append_undo_revision
            })
    }

    pub(crate) fn book_append_operation_is_current(
        &self,
        operation: crate::book_append_undo::AppendOperation,
    ) -> bool {
        self.book_append_operation_is_live(operation)
            && operation.source_context == self.projected_viewer_context_id()
    }

    pub(crate) fn book_append_operation_is_live(
        &self,
        operation: crate::book_append_undo::AppendOperation,
    ) -> bool {
        let same_pending = self.book_op_pending.as_ref().is_some_and(|pending| matches!(
            &pending.intent,
            crate::books::BookOpIntent::Append { operation_id, source_context, accepted_revision, .. }
                if *operation_id == operation.operation_id
                    && *source_context == operation.source_context
                    && *accepted_revision == operation.accepted_revision
        ));
        (same_pending || self.book_append_operation_matches_receipt(operation))
            && operation.accepted_revision == self.book_append_undo_revision
            && self
                .with_viewer_context_ref(operation.source_context, |_| ())
                .is_some()
    }

    fn book_append_undo_writers_busy(&self) -> bool {
        self.book_op_pending.is_some()
            || self.delete_pending.is_some()
            || self.metadata_cleanup_pending.is_some()
            || self.delete_purge_retry_pending.is_some()
            || self.rename_migration_in_flight.is_some()
            || !self.rename_migration_queue.is_empty()
            || !self.rename_migration_boot_retry.is_empty()
            || self.rename_migration_writers_busy()
            || self.local_adjust_lut_pending.is_some()
            || self.local_adjust_segmentation_pending.is_some()
            || self.stamp_embed_pending.is_some()
            || self.view_trim_save_pending
            || !self.view_trim_dirty_page_overrides.is_empty()
            || self.tag_maintenance_rx.is_some()
            || self.new_folder_pending.is_some()
            || self.metadata_transfer_book_append_writers_busy()
            || self.book_reorder.as_ref().is_some_and(|state| {
                state.flush_pending.is_some() || state.transfer_pending.is_some()
            })
            || self.remote_session_blocks_local_control()
            || self.remote_bookmark_writes_pending()
    }

    /// True consumes this input: busy, invalid, or accepted append Undo never
    /// falls through into a second, older metadata operation.
    pub(crate) fn try_undo_book_append(&mut self, ctx: &egui::Context) -> bool {
        if self.book_append_undo_in_progress() {
            self.show_feedback_toast("本への追加を取り消しています".into());
            return true;
        }
        let Some(receipt) = self.book_append_undo_receipt.as_ref().cloned() else {
            return false;
        };
        if receipt.accepted_revision != self.book_append_undo_revision
            || receipt.source_context != self.projected_viewer_context_id()
        {
            self.invalidate_book_append_undo();
            self.show_feedback_toast("本への追加の取り消しは終了しました".into());
            return true;
        }
        if self.book_append_undo_writers_busy() {
            self.show_feedback_toast("処理中のため本への追加を取り消せません".into());
            return true;
        }
        if !self.admit_rename_migration_source_change(ctx) {
            return true;
        }
        let paths = receipt
            .files
            .created
            .iter()
            .map(|stamp| stamp.path.clone())
            .collect::<Vec<_>>();
        self.release_viewer_surfaces_for_removed_paths_with_metadata_boundary(
            ctx,
            &paths,
            "book_append_undo",
            FsMetadataBoundary::BookAppend(receipt.operation()),
        );
        // Teardown may have issued a final writer. A maintenance lease must not
        // become a substitute for consuming those existing writer completions.
        if self.book_append_undo_writers_busy()
            || receipt.accepted_revision != self.book_append_undo_revision
            || receipt.source_context != self.projected_viewer_context_id()
            || !self.book_append_operation_matches_receipt(receipt.operation())
        {
            self.show_feedback_toast("処理中のため本への追加を取り消せません".into());
            return true;
        }
        self.cancel_book_append_readonly_refresh();
        self.book_append_undo_receipt = None;
        let intent = crate::books::BookOpIntent::UndoAppend {
            receipt: receipt.clone(),
        };
        let data_dir = crate::data_dir::get();
        let hwnd = self.main_hwnd_for_book_append_undo();
        self.start_book_op(ctx, "book-append-undo", intent, move || {
            crate::book_append_undo::undo_append_at(data_dir, receipt, hwnd)
        });
        true
    }

    #[cfg(windows)]
    fn main_hwnd_for_book_append_undo(&self) -> Option<isize> {
        self.main_hwnd
    }
    #[cfg(not(windows))]
    fn main_hwnd_for_book_append_undo(&self) -> Option<isize> {
        None
    }

    fn remote_bookmark_writes_pending(&self) -> bool {
        self.remote_session_ui.has_pending_bookmark_writes()
    }

    pub(super) fn start_book_append_metadata_terminal(
        &mut self,
        ctx: &egui::Context,
        summary: crate::books::BookAppendSummary,
    ) {
        let fence = (!summary.edit_copies.is_empty())
            .then(|| {
                self.local_adjust_write_handle
                    .as_ref()
                    .map(|handle| handle.enqueue_fence())
            })
            .flatten();
        let mut stores = crate::book_append_undo::AppendMetadataStores {
            adjustment: self.adjustment_db.is_some(),
            local_adjust: self.local_adjust_db.is_some(),
            crop: self.export_crop_db.is_some(),
            mask: self.mask_db.is_some(),
            conceal: self.conceal_db.is_some(),
            comic: self.comic_db.is_some(),
            rating: self.rating_db.is_some(),
            tags: self.tags_db.is_some(),
        };
        let data_dir = crate::data_dir::get();
        let (tx, rx) = std::sync::mpsc::channel();
        let tx_worker = tx.clone();
        // Move the payload once, without cloning thousands of page paths on UI.
        // On spawn failure no worker exists, so the error branch can take it
        // without waiting for a running worker or introducing another pending owner.
        let payload = Arc::new(std::sync::Mutex::new(Some(summary)));
        let worker_payload = Arc::clone(&payload);
        let result = std::thread::Builder::new()
            .name("book-append-metadata".into())
            .spawn(move || {
                let mut summary = worker_payload
                    .lock()
                    .unwrap()
                    .take()
                    .expect("one metadata payload");
                let fence_error = match fence {
                    None => None,
                    Some(Err(error)) => Some(error),
                    Some(Ok(fence)) => fence.recv().err().map(|error| error.to_string()),
                };
                if let Some(error) = fence_error {
                    summary.undo_files = None;
                    summary
                        .metadata_copy
                        .errors
                        .push(format!("local_adjust fence: {error}"));
                    stores.local_adjust = false;
                }
                crate::book_append_undo::copy_append_metadata_at(&data_dir, &mut summary, stores);
                let _ = tx_worker.send(Ok(crate::books::BookOpResult::Append(summary)));
            });
        // Receiver handoff never retires the operation or its deferred observations.
        self.book_op_pending
            .as_mut()
            .expect("append owns existing pending")
            .rx = rx;
        match result {
            Ok(_) => {
                ctx.request_repaint_after(std::time::Duration::from_millis(100));
            }
            Err(error) => {
                crate::logger::log(format!("[book-append-undo] metadata worker spawn: {error}"));
                let mut failed_summary = payload
                    .lock()
                    .unwrap()
                    .take()
                    .expect("failed spawn retains metadata payload");
                failed_summary.undo_files = None;
                failed_summary
                    .metadata_copy
                    .errors
                    .push(format!("metadata worker: {error}"));
                let _ = tx.send(Ok(crate::books::BookOpResult::Append(failed_summary)));
                ctx.request_repaint();
            }
        }
    }

    pub(super) fn finish_book_append_metadata(
        &mut self,
        summary: &crate::books::BookAppendSummary,
    ) {
        let report = &summary.metadata_copy;
        self.adjusted_page_keys
            .extend(report.adjusted.iter().cloned());
        self.local_adjust_page_keys
            .extend(report.local_adjusted.iter().cloned());
        self.export_crop_page_keys
            .extend(report.cropped.iter().cloned());
        self.mask_page_keys.extend(report.erased.iter().cloned());
        self.conceal_page_keys
            .extend(report.concealed.iter().cloned());
        self.comic_page_keys
            .extend(report.annotated.iter().cloned());
        // A target may already have loaded these new paths while the metadata
        // worker ran. Publish exact copied values without resetting its draft.
        for (key, stars) in &report.ratings {
            self.record_rating_session_write(key.clone(), *stars, false);
        }
        self.publish_book_append_destination_metadata(&report.ratings, &report.tags);
        // Destinations are create_new outputs. Copy completion cannot erase
        // a later draft in the source or an unchanged sibling's page caches;
        // visible destination hydration has its own existing install boundary.
        if !summary.edit_copies.is_empty() || !summary.semantic_copies.is_empty() {
            self.invalidate_rating_counts_cache();
        }
        if !report.errors.is_empty() {
            self.show_feedback_toast(format!(
                "本ページの編集情報の引き継ぎに一部失敗しました ({} 件)",
                report.errors.len()
            ));
        }
    }

    pub(super) fn finish_book_append_undo(
        &mut self,
        result: crate::book_append_undo::BookAppendUndoResult,
    ) {
        self.apply_content_identity_store_mutations(result.store_mutations);
        if !result.succeeded.is_empty() {
            self.purge_in_memory_metadata_for_removed_paths(&result.succeeded, &[]);
            self.invalidate_rename_migrations_for_removed_paths(&result.succeeded);
            let matcher = removed_path_key_matcher(&result.succeeded);
            let mut idxs = self
                .items
                .iter()
                .enumerate()
                .filter_map(|(idx, item)| {
                    item.drag_source_path()
                        .filter(|path| matcher(&crate::adjustment_db::normalize_path(path)))
                        .map(|_| idx)
                })
                .collect::<Vec<_>>();
            idxs.sort_unstable_by(|a, b| b.cmp(a));
            if !idxs.is_empty() {
                self.remove_items_batch(&idxs);
            }
            if let Some(rows) = self.book_list_cache.as_mut()
                && let Some(row) = rows
                    .iter_mut()
                    .find(|row| crate::folder_tree::path_eq(&row.path, &result.folder))
            {
                row.page_count = row.page_count.saturating_sub(result.succeeded.len());
            }
        }
        if result.failed.is_empty() && result.purge_errors.is_empty() {
            self.show_feedback_toast(format!(
                "「{}」への {} ページの追加を取り消しました",
                result.book_name,
                result.succeeded.len()
            ));
        } else {
            for (path, error) in &result.failed {
                crate::logger::log(format!("[book-append-undo] {}: {error}", path.display()));
            }
            for error in &result.purge_errors {
                crate::logger::log(format!("[book-append-undo] metadata purge: {error}"));
            }
            self.show_feedback_toast(format!(
                "{} ページを取り消しました。{} 件を取り消せませんでした",
                result.succeeded.len(),
                result.failed.len() + result.purge_errors.len()
            ));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::book_append_undo::{
        AppendObservations, AppendOperation, AppendReceiptBuilder, BookAppendUndoReceipt,
    };
    use crate::books::{BookOpIntent, BookOpPending, BookOpResult};

    #[test]
    fn section1373_manual_target_reload_before_metadata_updates_only_copied_cache_keys() {
        let mut app = setup_app_for_test();
        let receipt = receipt(&app);
        let destination = receipt.files.created[0].path.clone();
        let source = app.tmp.path().join("unchanged-source.jpg");
        let destination_key = crate::adjustment_db::normalize_path(&destination);
        let source_key = crate::adjustment_db::normalize_path(&source);
        // The explicit target reload has already cached the newly created rows
        // before the copy worker's SQLite terminal, so same-list refresh cannot
        // repair these values through an items install.
        app.items = vec![
            crate::grid_item::GridItem::Image(destination.clone()),
            crate::grid_item::GridItem::Image(source),
        ];
        app.rating_cache.insert(0, 0);
        app.rating_cache.insert(1, 3);
        app.tags_cache.insert(destination_key.clone(), Vec::new());
        app.tags_cache
            .insert(source_key.clone(), vec!["#source".into()]);
        app.erase_mask = Some(vec![true, false]);
        let before_generation = app.rating_session_write_generation;
        let before_tags_revision = app.details_cell_content_revisions.tags;
        let summary = crate::books::BookAppendSummary {
            book_name: receipt.files.book_name.clone(),
            folder: receipt.files.folder.clone(),
            added: 1,
            first_path: Some(destination.clone()),
            edit_copies: Vec::new(),
            semantic_copies: Vec::new(),
            erase_fallback_pages: 0,
            undo_files: Some(receipt.files),
            metadata_copy: crate::book_append_undo::BookAppendMetadataCopySummary {
                ratings: [(destination_key.clone(), 4)].into(),
                tags: [(destination_key.clone(), vec!["#copied".into()])].into(),
                ..Default::default()
            },
        };
        app.finish_book_append_metadata(&summary);
        assert_eq!(app.rating_cache.get(&0), Some(&4));
        assert_eq!(
            app.tags_cache.get(&destination_key),
            Some(&vec!["#copied".into()])
        );
        assert_eq!(app.rating_cache.get(&1), Some(&3));
        assert_eq!(
            app.tags_cache.get(&source_key),
            Some(&vec!["#source".into()])
        );
        assert_eq!(app.erase_mask.as_deref(), Some([true, false].as_slice()));
        assert_eq!(
            app.details_cell_content_revisions.tags,
            before_tags_revision + 1
        );
        // Existing per-path publication also overlays a late prepared rating
        // read and rejects a pre-copy XMP hydration result.
        app.rating_cache.insert(0, 0);
        app.overlay_rating_session_writes_since(before_generation);
        assert_eq!(app.get_rating(0), 4);
        app.hydrate_ratings_from_xmp(vec![(destination, 1, 0)]);
        assert_eq!(app.get_rating(0), 4);
    }

    fn receipt(app: &AppTestEnvForTest) -> BookAppendUndoReceipt {
        let root = app.tmp.path().join("undo-helper-books");
        let folder = root.join("book");
        std::fs::create_dir_all(&folder).unwrap();
        let mut builder = AppendReceiptBuilder::begin(&root, &folder, "book".into()).unwrap();
        let output = folder.join("0001_copy.jpg");
        std::fs::write(&output, b"created page").unwrap();
        builder.record(&output).unwrap();
        BookAppendUndoReceipt {
            files: builder.finish().unwrap(),
            operation_id: 17,
            source_context: app.projected_viewer_context_id(),
            accepted_revision: app.book_append_undo_revision,
        }
    }

    fn hold_append(
        app: &mut App,
        receipt: &BookAppendUndoReceipt,
    ) -> std::sync::mpsc::Sender<Result<BookOpResult, String>> {
        let (tx, rx) = std::sync::mpsc::channel();
        let mut observations = AppendObservations::default();
        observations.record(receipt.source_context);
        app.book_op_pending = Some(BookOpPending {
            rx,
            intent: BookOpIntent::Append {
                operation_id: receipt.operation_id,
                source_context: receipt.source_context,
                accepted_revision: receipt.accepted_revision,
                target_folder: receipt.files.folder.clone(),
                observations,
            },
        });
        tx
    }

    #[test]
    fn section1373_own_refresh_matches_pending_before_receipt_and_receipt_after_retirement() {
        let mut app = setup_app_for_test();
        let receipt = receipt(&app);
        let operation = receipt.operation();
        let _tx = hold_append(&mut app, &receipt);
        assert!(app.book_append_operation_is_current(operation));
        assert!(!app.book_append_operation_matches_receipt(operation));
        assert!(!app.book_append_operation_is_current(AppendOperation {
            operation_id: operation.operation_id + 1,
            ..operation
        }));
        app.book_op_pending = None;
        assert!(!app.book_append_operation_is_current(operation));
        app.book_append_undo_receipt = Some(receipt);
        assert!(app.book_append_operation_is_current(operation));
        assert!(app.book_append_operation_matches_receipt(operation));
        app.invalidate_book_append_undo();
        assert!(!app.book_append_operation_is_current(operation));
    }

    #[test]
    fn section1373_late_pending_completion_cannot_match_after_accepted_mutation() {
        let mut app = setup_app_for_test();
        let receipt = receipt(&app);
        let operation = receipt.operation();
        let _tx = hold_append(&mut app, &receipt);
        assert!(app.admit_book_append_followup_mutation());
        assert!(app.book_op_pending.is_some());
        assert!(!app.book_append_operation_is_current(operation));
        assert!(!app.book_append_undo_in_progress());
    }

    #[test]
    fn section1373_metadata_receiver_handoff_keeps_append_identity_and_observations() {
        let mut app = setup_app_for_test();
        let receipt = receipt(&app);
        let _tx = hold_append(&mut app, &receipt);
        let summary = crate::books::BookAppendSummary {
            book_name: receipt.files.book_name.clone(),
            folder: receipt.files.folder.clone(),
            added: 1,
            first_path: Some(receipt.files.created[0].path.clone()),
            edit_copies: Vec::new(),
            semantic_copies: Vec::new(),
            erase_fallback_pages: 0,
            undo_files: Some(receipt.files.clone()),
            metadata_copy: Default::default(),
        };
        app.start_book_append_metadata_terminal(&egui::Context::default(), summary);
        let pending = app.book_op_pending.as_ref().unwrap();
        let BookOpIntent::Append {
            operation_id,
            source_context,
            accepted_revision,
            target_folder,
            observations,
        } = &pending.intent
        else {
            panic!("same append owner required");
        };
        assert_eq!(*operation_id, receipt.operation_id);
        assert_eq!(*source_context, receipt.source_context);
        assert_eq!(*accepted_revision, receipt.accepted_revision);
        assert_eq!(target_folder, &receipt.files.folder);
        assert_eq!(observations.contexts(), &[receipt.source_context]);
        let result = pending
            .rx
            .recv_timeout(std::time::Duration::from_secs(10))
            .unwrap()
            .unwrap();
        let BookOpResult::Append(summary) = result else {
            panic!("metadata terminal required");
        };
        assert!(summary.undo_files.is_some());
        assert!(app.book_append_undo_receipt.is_none());
        assert!(app.book_op_pending.is_some());
    }
}
