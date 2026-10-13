use super::*;

/// Fresh exact-key presence for the existing Append receiver. This runs only on
/// its worker and reads current durable state, never the original copy values.
pub(super) fn load_book_append_edit_presence(
    summary: &crate::books::BookAppendSummary,
    mut available: page_edit_snapshot::PageEditAvailability,
) -> Result<Option<page_edit_snapshot::StablePageEditProjection>, String> {
    let report = &summary.metadata_copy;
    let mut keys = [
        &report.adjusted,
        &report.local_adjusted,
        &report.cropped,
        &report.erased,
        &report.concealed,
        &report.annotated,
    ]
    .into_iter()
    .flatten()
    .cloned()
    .collect::<Vec<_>>();
    keys.sort_unstable();
    keys.dedup();
    let items = keys
        .into_iter()
        .map(|key| GridItem::Image(PathBuf::from(key)))
        .collect::<Vec<_>>();
    // Append does not transfer view-trim state.
    available.view_trim = false;
    page_edit_snapshot::PageEditSnapshot::load_and_project_stable(
        &items,
        available,
        &AtomicBool::new(false),
    )
}

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

    /// Patch only the App-global rollup sets after a fresh DB read. Visible and
    /// parked contexts already loaded initialized metadata after file creation;
    /// their caches and edit drafts must retain any later accepted changes.
    pub(super) fn apply_book_append_edit_presence(
        &mut self,
        summary: &crate::books::BookAppendSummary,
        prepared: &page_edit_snapshot::StablePageEditProjection,
    ) -> bool {
        if !crate::page_edit_write_epoch::PAGE_EDIT_WRITES.accepts(prepared.stamp) {
            return false;
        }
        let report = &summary.metadata_copy;
        let read = &prepared.snapshot;
        for key in &report.adjusted {
            Self::set_page_key_presence(
                &mut self.adjusted_page_keys,
                key,
                read.adjustment.contains_key(key),
            );
        }
        for key in &report.local_adjusted {
            // A queued local-adjust write can precede its DB write epoch. Its
            // accepted UI presence remains authoritative until completion.
            if !self.local_adjust_write_pending.contains_key(key) {
                Self::set_page_key_presence(
                    &mut self.local_adjust_page_keys,
                    key,
                    read.local_adjust.contains(key),
                );
            }
        }
        for key in &report.cropped {
            Self::set_page_key_presence(
                &mut self.export_crop_page_keys,
                key,
                read.export_crop.contains_key(key),
            );
        }
        for key in &report.erased {
            Self::set_page_key_presence(&mut self.mask_page_keys, key, read.mask.contains(key));
        }
        for key in &report.concealed {
            Self::set_page_key_presence(
                &mut self.conceal_page_keys,
                key,
                read.conceal.contains(key),
            );
        }
        for key in &report.annotated {
            Self::set_page_key_presence(&mut self.comic_page_keys, key, read.comic.contains(key));
        }
        true
    }

    pub(super) fn finish_book_append_metadata(
        &mut self,
        summary: &crate::books::BookAppendSummary,
    ) {
        let report = &summary.metadata_copy;
        // Metadata is initialized on the append worker before the file is visible.
        // This terminal carries changed keys only: a user may already have edited
        // or cleared the destination. Fresh DB reads own cache/presence hydration.
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

    fn presence_summary(app: &AppTestEnvForTest) -> crate::books::BookAppendSummary {
        let receipt = receipt(app);
        let destination = receipt.files.created[0].path.clone();
        let key = crate::adjustment_db::normalize_path(&destination);
        crate::books::BookAppendSummary {
            book_name: receipt.files.book_name.clone(),
            folder: receipt.files.folder.clone(),
            added: 1,
            first_path: Some(destination),
            edit_copies: Vec::new(),
            semantic_copies: Vec::new(),
            erase_fallback_pages: 0,
            undo_files: Some(receipt.files),
            metadata_copy: crate::book_append_undo::BookAppendMetadataCopySummary {
                adjusted: vec![key.clone()],
                local_adjusted: vec![key.clone()],
                cropped: vec![key.clone()],
                erased: vec![key.clone()],
                concealed: vec![key.clone()],
                annotated: vec![key],
                ..Default::default()
            },
        }
    }

    #[test]
    fn section1373_fresh_presence_read_reflects_later_add_and_clear_without_unrelated_loss() {
        let mut app = setup_app_for_test();
        let summary = presence_summary(&app);
        let key = summary.metadata_copy.adjusted[0].clone();
        let other = "unrelated-edit.jpg".to_owned();
        let owner: &mut App = &mut app;
        for keys in [
            &mut owner.adjusted_page_keys,
            &mut owner.local_adjust_page_keys,
            &mut owner.export_crop_page_keys,
            &mut owner.mask_page_keys,
            &mut owner.conceal_page_keys,
            &mut owner.comic_page_keys,
        ] {
            keys.insert(other.clone());
        }
        let available = page_edit_snapshot::PageEditAvailability {
            adjustment: true,
            ..Default::default()
        };
        // File is already visible; later writes precede the existing Append
        // receiver's fresh read. Original copy report remains unchanged.
        for present in [true, false] {
            if present {
                app.adjustment_db
                    .as_ref()
                    .unwrap()
                    .set_page_params(&key, &crate::adjustment::AdjustParams::default())
                    .unwrap();
            } else {
                app.adjustment_db
                    .as_ref()
                    .unwrap()
                    .remove_page_params(&key)
                    .unwrap();
            }
            let prepared = load_book_append_edit_presence(&summary, available)
                .unwrap()
                .unwrap();
            assert!(app.apply_book_append_edit_presence(&summary, &prepared));
            assert_eq!(app.adjusted_page_keys.contains(&key), present);
            for keys in [
                &app.adjusted_page_keys,
                &app.local_adjust_page_keys,
                &app.export_crop_page_keys,
                &app.mask_page_keys,
                &app.conceal_page_keys,
                &app.comic_page_keys,
            ] {
                assert!(keys.contains(&other));
            }
        }
    }

    #[test]
    fn section1373_presence_read_rejects_later_edit_epoch_without_patching_globals() {
        let mut app = setup_app_for_test();
        let summary = presence_summary(&app);
        let key = summary.metadata_copy.adjusted[0].clone();
        let prepared = load_book_append_edit_presence(&summary, Default::default())
            .unwrap()
            .unwrap();
        app.adjusted_page_keys.insert(key.clone());
        let write = crate::page_edit_write_epoch::PAGE_EDIT_WRITES.begin_for_key(&key);
        assert!(!app.apply_book_append_edit_presence(&summary, &prepared));
        assert!(app.adjusted_page_keys.contains(&key));
        drop(write);
        assert!(!app.apply_book_append_edit_presence(&summary, &prepared));
        assert!(app.adjusted_page_keys.contains(&key));
    }

    #[test]
    fn section1373_presence_read_preserves_queued_local_adjust_add_and_clear() {
        let mut app = setup_app_for_test();
        let summary = presence_summary(&app);
        let key = summary.metadata_copy.local_adjusted[0].clone();
        let mut prepared = load_book_append_edit_presence(&summary, Default::default())
            .unwrap()
            .unwrap();
        app.local_adjust_write_pending.insert(
            key.clone(),
            LocalAdjustWritePending {
                generation: 1,
                idx: 0,
                sidecar: None,
            },
        );
        for present in [true, false] {
            App::set_page_key_presence(&mut app.local_adjust_page_keys, &key, present);
            if present {
                prepared.snapshot.local_adjust.remove(&key);
            } else {
                prepared.snapshot.local_adjust.insert(key.clone());
            }
            assert!(app.apply_book_append_edit_presence(&summary, &prepared));
            assert_eq!(app.local_adjust_page_keys.contains(&key), present);
        }
        app.local_adjust_write_pending.remove(&key);
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
}
