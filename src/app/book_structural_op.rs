//! Process-wide admission through terminal adoption for compiled-book mutations.
use super::*;
use std::collections::BTreeSet;

type Work = Box<dyn FnOnce() -> Result<crate::books::BookOpResult, String> + Send>;
type UnsavedTrims = Arc<
    std::sync::Mutex<
        Vec<(
            ViewerContextId,
            crate::ui_view_trim::PendingViewTrimTransfer,
        )>,
    >,
>;

#[derive(Clone, Copy)]
pub(crate) enum Completion {
    Manager,
    ReorderFlush,
    ReorderTransfer,
}

enum Phase {
    Requested {
        name: &'static str,
        work: Work,
    },
    Running {
        rx: mpsc::Receiver<Result<crate::books::BookOpResult, String>>,
        trims: UnsavedTrims,
    },
}

pub(crate) struct BookStructuralOp {
    phase: Phase,
    intent: crate::books::BookOpIntent,
    completion: Completion,
    _lease: Option<crate::remote_ipc::session::LocalModalLease>,
}

#[cfg(test)]
impl BookStructuralOp {
    pub(super) fn running_for_test(
        rx: mpsc::Receiver<Result<crate::books::BookOpResult, String>>,
        intent: crate::books::BookOpIntent,
        completion: Completion,
    ) -> Self {
        Self {
            phase: Phase::Running {
                rx,
                trims: Default::default(),
            },
            intent,
            completion,
            _lease: None,
        }
    }
}

impl App {
    /// D2c: finish every accepted open before admitting a structural modal. The
    /// source path is deliberately irrelevant. Retained-session readoption and
    /// passive display/decoder reads are not new opens.
    fn book_structural_open_pending(&mut self) -> bool {
        if self.sidecar_restore.is_some()
            || self.saved_group_open.is_some()
            || self.startup_open_path_resolve_pending.is_some()
            || self.media_navigation_pending.is_some()
            || self.smart_pdf_password_dialog_path().is_some()
            || self.archive_convert.as_ref().is_some_and(|conversion| {
                !matches!(
                    conversion.completion,
                    crate::ui_dialogs::archive_convert::ArchiveConvertCompletionPolicy::SiblingZip
                ) && !matches!(
                    conversion.phase,
                    crate::ui_dialogs::archive_convert::ArchiveConvertPhase::Error { .. }
                )
            })
        {
            return true;
        }
        self.viewer_context_ids().into_iter().any(|id| {
            self.with_owner_viewer_context(id, |app| {
                app.folder_nav_pending.is_some()
                    || app
                        .folder_pane_open_pending
                        .as_ref()
                        .is_some_and(|pending| {
                            !matches!(
                                pending.purpose,
                                FolderOpenScanPurpose::CurrentViewOrderRefresh { .. }
                            )
                        })
                    || app.top_level_grid_view.open_path_classification().is_some()
                    || app.location_navigation_pending()
                    || app.top_level_grid_view.collection_navigation_pending()
                    || app
                        .top_level_grid_view
                        .collection_session()
                        .is_some_and(|session| {
                            matches!(
                                &session.load,
                                top_level_grid_view::CollectionGridLoadState::RequestNeeded {
                                    installed: None,
                                    ..
                                } | top_level_grid_view::CollectionGridLoadState::Snapshot {
                                    installed: None,
                                    ..
                                } | top_level_grid_view::CollectionGridLoadState::Preparing {
                                    installed: None,
                                    ..
                                }
                            )
                        })
                    || app.smart_folder_opening_pending()
                    || app.staged_smart_child_preflight_from_visible_scope()
                    || app.bookmark_open_pending.is_some()
                    || app.pdf_enumerate_pending.is_some()
                    || app.zip_enumerate_pending.is_some()
                    || app.epub_convert.is_some()
                    || app.pdf_password_request.is_some()
                    || app.pending_rating_view_zipdir_open.is_some()
                    || app.pending_auto_fs_open
                    || app.fs_nav_after_pdf_enumerate.is_some()
                    || app.book_composition_permission.open_pending()
                    || app
                        .stack_script_pending
                        .as_ref()
                        .is_some_and(|pending| pending.open_pending())
            })
            .unwrap_or(true)
        })
    }

    /// The book operation protects every producer, including accepted results and
    /// their downstream records. Generic rename and sidecar restore retain their
    /// narrower admission contracts; neither polls completions from a predicate.
    pub(super) fn book_structural_writers_busy(&self) -> bool {
        self.rename_migration_writers_busy()
            || self.has_tag_prewarm_work()
            || self.has_page_edit_producers()
            || self.content_identity_detection_pending.is_some()
            || self.content_identity_backfill_pending.is_some()
            || self
                .reading_history_writer
                .as_ref()
                .is_some_and(|w| w.is_busy())
            || self.composition_editor_writes_pending()
            || self.remote_bookmark_writes_pending()
            || self
                .book_composition_service
                .as_ref()
                .is_some_and(|s| s.is_busy())
            || self.stamp_embed_pending.is_some()
            || self.tag_maintenance_rx.is_some()
            || self.edit_bundle_apply_pending.is_some()
            || self.edit_bundle_paste_pending.is_some()
            || self.edit_bundle_bulk_pending.is_some()
            || crate::page_edit_write_epoch::PAGE_EDIT_WRITES
                .sample()
                .active_writers
                != 0
            || self
                .content_identity_recorder
                .as_ref()
                .is_some_and(|w| w.is_busy())
    }

    fn book_structural_context_input_pending(&mut self) -> bool {
        self.viewer_context_ids().into_iter().any(|id| {
            self.with_owner_viewer_context(id, |app| {
                app.adjustment_drag_session.is_some()
                    || app.adjustment_standard_drag_session.is_some()
                    || app.local_adjust_shape_key_edit.is_some()
                    || app.local_adjust_shape_drag.is_some()
                    || app.local_adjust_canvas_drag.is_some()
                    || app.local_adjust_mask_brush_stroke.is_some()
                    || app.local_adjust_mask_shape_drag_start.is_some()
                    || !app.local_adjust_mask_lasso_points.is_empty()
                    || app.erase_last_paint_pos.is_some()
                    || app.erase_drag.is_some()
                    || app.erase_line_start.is_some()
                    || app.erase_shape_drag_start.is_some()
                    || !app.erase_lasso_points.is_empty()
                    || app.conceal_last_paint_pos.is_some()
                    || app.conceal_drag.is_some()
                    || app.conceal_line_start.is_some()
                    || app.conceal_shape_drag_start.is_some()
                    || !app.conceal_lasso_points.is_empty()
                    || app.text_drag.is_some()
                    || app.text_marquee.is_some()
                    || app.text_dirty_at.is_some()
                    || app.export_crop_drag.is_some()
                    || app.export_crop_create_drag.is_some()
                    || app.text_subdialog_open()
                    // This is an existing displayed-list edit, not an open.
                    // Retain its previous admission block separately from D2c.
                    || app.folder_pane_open_pending.as_ref().is_some_and(|pending| {
                        matches!(pending.purpose, FolderOpenScanPurpose::CurrentViewOrderRefresh { .. })
                    })
                    || app
                        .fs_pending
                        .iter()
                        .any(|(_, p)| p.purpose.original_entry().is_some())
                    || app
                        .fs_upload_backlog
                        .iter()
                        .any(|p| p.purpose.original_entry().is_some())
                    || app
                        .erase_inpaint_pending
                        .values()
                        .any(|p| p.purpose.entry().is_some())
                    || app
                        .local_adjust_pending
                        .values()
                        .any(|p| match p.request.kind {
                            EditMaterializeKind::Local { continuation, .. } => {
                                continuation != LocalMaterializeContinuation::Display
                            }
                            EditMaterializeKind::Conceal { purpose, .. } => {
                                purpose != ConcealMaterializePurpose::Display
                            }
                        })
            })
            .unwrap_or(true)
        })
    }

    fn book_structural_admission_block_reason(&self) -> Option<&'static str> {
        if self.book_structural_op.is_some() || self.book_op_pending.is_some() {
            return Some("製本処理中です");
        }
        if matches!(
            self.rename_migration_in_flight,
            Some(RenameMigrationInFlight::Generic { .. })
        ) || self
            .rename_migration_queue
            .iter()
            .chain(&self.rename_migration_boot_retry)
            .any(|job| job.generic_pending)
            || self.rename_pending.is_some()
        {
            return Some("名前変更の反映中です。完了後にやり直してください");
        }
        if self.delete_pending.is_some()
            || self.delete_purge_retry_pending.is_some()
            || self.metadata_cleanup_pending.is_some()
            || self.metadata_transfer.is_some()
            || self.image_folder_zip_operation.is_some()
            || self.content_identity_restore_pending.is_some()
            || !self.drop_copy_pending.is_empty()
            || self.capture_pending.is_some()
            || self.batch_convert.is_some()
            || self.epub_batch_convert.is_some()
            || self.document_open_modal_admission_blocked()
        {
            return Some("ファイル・編集情報の変更中です。完了後にやり直してください");
        }
        None
    }

    pub(crate) fn request_book_structural_op(
        &mut self,
        ctx: &egui::Context,
        name: &'static str,
        intent: crate::books::BookOpIntent,
        completion: Completion,
        work: impl FnOnce() -> Result<crate::books::BookOpResult, String> + Send + 'static,
    ) -> bool {
        if self.book_structural_open_pending() {
            self.show_feedback_warning(
                "フォルダや本を開いています。開き終わってから本の変更をやり直してください".into(),
            );
            return false;
        }
        if let Some(reason) = self.book_structural_admission_block_reason() {
            self.show_feedback_warning(reason.into());
            return false;
        }
        if self.book_structural_context_input_pending() {
            self.show_feedback_warning(
                "編集中の操作が完了してから、本の変更をやり直してください".into(),
            );
            return false;
        }
        if !self.admit_rename_migration_source_change(ctx) {
            return false;
        }
        let lease = if let Some(handle) = self.remote_session_handle() {
            let Some(lease) = handle.try_local_modal_lease() else {
                self.show_feedback_warning("mIV Remote の使用中は本を変更できません".into());
                return false;
            };
            Some(lease)
        } else {
            None
        };
        if !self.admit_book_append_followup_mutation() {
            return false;
        }
        self.book_structural_op = Some(BookStructuralOp {
            phase: Phase::Requested {
                name,
                work: Box::new(work),
            },
            intent,
            completion,
            _lease: lease,
        });
        ctx.request_repaint();
        true
    }

    /// Polling only: previously accepted completions retain their normal save paths.
    pub(crate) fn poll_book_structural_op(&mut self, ctx: &egui::Context) {
        if self.book_structural_op.is_none() {
            return;
        }
        self.poll_all_page_edit_producers(ctx);
        // Accepted XMP reads remain owned even by parked viewers. Draining is an
        // explicit frame transition; DB hydration itself runs on its worker.
        for owner in self.viewer_context_ids() {
            self.with_owner_viewer_context(owner, |app| app.poll_tag_prewarm_results());
        }
        let requested = matches!(
            self.book_structural_op.as_ref().map(|op| &op.phase),
            Some(Phase::Requested { .. })
        );
        if requested && self.book_structural_writers_busy() {
            ctx.request_repaint_after(std::time::Duration::from_millis(50));
            return;
        }
        let mut op = self.book_structural_op.take().expect("owner checked");
        let result = match op.phase {
            Phase::Requested { name, work } => {
                let mut trims = Vec::new();
                for id in self.viewer_context_ids() {
                    if let Some(Some(pending)) = self
                        .with_owner_viewer_context(id, |app| app.take_pending_view_trim_transfer())
                    {
                        trims.push((id, pending));
                    }
                }
                let data_dir = self.rename_migration_data_dir();
                let trims: UnsavedTrims = Arc::new(std::sync::Mutex::new(trims));
                let worker_trims = Arc::clone(&trims);
                let (tx, rx) = mpsc::channel();
                match std::thread::Builder::new()
                    .name(name.into())
                    .spawn(move || {
                        let result = (|| {
                            {
                                // Until this pre-FS save succeeds, the original dirty payload
                                // remains owned, including a thread-spawn/preparation failure.
                                let mut unsaved =
                                    worker_trims.lock().unwrap_or_else(|e| e.into_inner());
                                for (_, pending) in unsaved.iter() {
                                    crate::view_trim_db::ViewTrimDb::open_at(
                                        &data_dir.join("view_trim.db"),
                                    )
                                    .and_then(|mut db| db.apply_write_batch(&pending.batch))
                                    .map_err(|e| e.to_string())?;
                                }
                                unsaved.clear();
                            }
                            work()
                        })();
                        let _ = tx.send(result);
                    }) {
                    Ok(_) => {
                        op.phase = Phase::Running { rx, trims };
                        self.book_structural_op = Some(op);
                        ctx.request_repaint_after(std::time::Duration::from_millis(50));
                        return;
                    }
                    Err(error) => {
                        self.restore_book_structural_trims(trims);
                        Err(format!("製本 worker を開始できません: {error}"))
                    }
                }
            }
            Phase::Running { rx, trims } => match rx.try_recv() {
                Ok(result) => {
                    self.restore_book_structural_trims(trims);
                    result
                }
                Err(mpsc::TryRecvError::Empty) => {
                    op.phase = Phase::Running { rx, trims };
                    self.book_structural_op = Some(op);
                    ctx.request_repaint_after(std::time::Duration::from_millis(50));
                    return;
                }
                Err(mpsc::TryRecvError::Disconnected) => {
                    self.restore_book_structural_trims(trims);
                    Err("製本処理が中断されました".into())
                }
            },
        };
        // The lease remains in op until every result consumer has adopted the mutation.
        if let Completion::ReorderTransfer = op.completion
            && let Ok(crate::books::BookOpResult::Transfer(summary)) = &result
            && let Some(state) = self.book_reorder.as_mut()
        {
            state.entries = summary.source_entries.clone();
            state.dirty = false;
            state.dragging = None;
            state.drag_auto_scroll_enabled = false;
            state.drag_insert_index = None;
            state.thumb_textures.clear();
            state.thumb_failed.clear();
            state.thumb_pending_keys.clear();
            state.thumb_upload_backlog.clear();
            state.thumb_tx = None;
            state.thumb_rx = None;
            state.selected_keys.clear();
            if !state.entries.is_empty() {
                let index = state.selected.unwrap_or(0).min(state.entries.len() - 1);
                state.selected = Some(index);
                state.selection_anchor = Some(index);
                state
                    .selected_keys
                    .insert(crate::adjustment_db::normalize_path(
                        &state.entries[index].path,
                    ));
            }
        }
        let success = result.is_ok();
        if let Err(error) = &result
            && let Some(state) = self.book_reorder.as_mut()
        {
            state.error = Some(error.clone());
        }
        self.complete_book_op_result(ctx, result, &op.intent);
        if (matches!(op.completion, Completion::ReorderFlush) && success)
            || (matches!(op.completion, Completion::ReorderTransfer)
                && (!success
                    || self
                        .book_reorder
                        .as_ref()
                        .is_some_and(|s| s.entries.is_empty())))
        {
            self.book_reorder = None;
        }
        ctx.request_repaint();
    }

    fn restore_book_structural_trims(&mut self, trims: UnsavedTrims) {
        let unsaved = std::mem::take(&mut *trims.lock().unwrap_or_else(|e| e.into_inner()));
        for (id, pending) in unsaved {
            self.with_owner_viewer_context(id, |app| {
                app.restore_pending_view_trim_transfer(pending)
            });
        }
    }
}

impl App {
    /// Presence is derived. Rewrite it from a snapshot so A/B swaps and empty sources
    /// have the same replacement semantics as the worker, without opening a DB.
    pub(super) fn adopt_book_path_metadata(
        &mut self,
        moves: &[crate::books::BookPathMapping],
        copies: &[crate::books::BookPathMapping],
        root_rename: Option<(&Path, &Path)>,
    ) {
        let original_moves = moves;
        let moves = Self::normalized_book_page_edit_mappings(moves);
        let copies = Self::normalized_book_page_edit_mappings(copies);
        for set in [
            &mut self.adjusted_page_keys,
            &mut self.local_adjust_page_keys,
            &mut self.export_crop_page_keys,
            &mut self.mask_page_keys,
            &mut self.conceal_page_keys,
            &mut self.comic_page_keys,
        ] {
            replace_presence(set, &moves, false);
            replace_presence(set, &copies, true);
        }
        replace_presence(&mut self.rotation_page_keys, &moves, false);
        for (_, target) in &copies {
            for key in scoped_keys(&self.rotation_page_keys, target) {
                self.rotation_page_keys.remove(&key);
            }
        }
        replace_key_map(&mut self.rating_session_writes, &moves, false);
        replace_key_map(&mut self.rating_session_writes, &copies, true);
        self.invalidate_rating_counts_cache();
        let paths = moves
            .iter()
            .chain(&copies)
            .flat_map(|(a, b)| [PathBuf::from(a), PathBuf::from(b)])
            .collect::<Vec<_>>();
        self.book_resume_meters.remove_scopes(&paths);
        self.reload_book_resume_meters();
        let scopes = moves
            .iter()
            .chain(&copies)
            .flat_map(|(a, b)| [a.clone(), b.clone()])
            .collect();
        // Page changes also affect a book's representative thumbnail in its parent
        // shelf. Root renames affect the roots themselves, never their whole shelf.
        let pin_folders = moves
            .iter()
            .chain(&copies)
            .flat_map(|(a, b)| [a, b])
            .filter_map(|key| {
                let path = Path::new(key);
                if root_rename.is_some() {
                    Some(path.to_path_buf())
                } else {
                    path.parent().map(Path::to_path_buf)
                }
            })
            .collect::<std::collections::HashSet<_>>();
        self.with_owner_viewer_context(self.viewer_context_main(), |app| {
            // Snapshot and independent readers retain the pixels and state
            // accepted when opened. A cache-only DB refresh would mix identities.
            if app.is_snapshot_active() {
                return;
            }
            let matches = |key: &str| key_matches_scope(key, &scopes);
            // Match the existing Renamed / Reordered / Transfer terminal's
            // pending_reload conditions; an extracted list with no reload
            // must keep its page caches too.
            let rebuilds_pages = app.current_folder.as_ref().is_some_and(|folder| {
                root_rename.is_some()
                    || pin_folders
                        .iter()
                        .any(|parent| crate::folder_tree::path_eq(folder, parent))
            });
            if rebuilds_pages
                && (app
                    .items
                    .iter()
                    .filter_map(crate::edit_source::page_key_for_grid_item)
                    .any(|key| matches(&key))
                    || app
                        .current_folder
                        .as_deref()
                        .is_some_and(|p| matches(&crate::adjustment_db::normalize_path(p))))
            {
                if root_rename.is_none()
                    && spread_shift_live_mappings::migrate(&mut app.spread_shift, original_moves)
                {
                    app.invalidate_fullscreen_display_unit_owners();
                }
                app.rating_cache.clear();
                app.current_folder_rating_cache = None;
                app.clear_tags_cache();
                app.clear_page_edit_state();
                app.clear_rotation_cache_for_reload();
                app.prepare_page_edits_for_current_virtual_items();
                let affected = app
                    .thumb_edit_preview_keys
                    .iter()
                    .filter_map(|(&idx, key)| matches(key).then_some(idx))
                    .collect::<Vec<_>>();
                for idx in affected {
                    app.evict_thumbnail_for_reload(idx);
                }
                app.refresh_current_view_pin_thumbnails(CurrentViewRefresh::Full);
            } else if app.items.iter().any(|item| {
                app.folder_pin_lookup_target_for_item(item)
                    .is_some_and(|target| {
                        pin_folders.iter().any(|folder| {
                            crate::books::path_is_under_or_equal(folder, &target.path)
                        })
                    })
            }) {
                app.refresh_current_view_pin_thumbnails(CurrentViewRefresh::Pins {
                    folders: pin_folders.clone(),
                    videos: Default::default(),
                });
            }
        });
    }

    pub(super) fn adopt_book_store_summary(&mut self, metadata: crate::books::BookStoreSummary) {
        self.apply_content_identity_store_mutations(metadata.store_mutations);
        self.report_book_page_edit_mapping_errors("structural", metadata.errors);
    }
}

fn key_matches_scope(key: &str, scopes: &std::collections::HashSet<String>) -> bool {
    scopes.contains(key)
        || key.char_indices().rev().any(|(index, ch)| {
            (ch == '/' || key[index..].starts_with("::")) && scopes.contains(&key[..index])
        })
}

fn replace_presence(set: &mut BTreeSet<String>, mappings: &[(String, String)], copy: bool) {
    // Global presence can contain millions of unrelated keys. Only snapshot the
    // affected sorted ranges; never scan every key once for every book page.
    let source = affected_keys(set, mappings);
    for (from, to) in mappings {
        for key in scoped_keys(&source, to).into_iter().chain(
            (!copy)
                .then(|| scoped_keys(&source, from))
                .into_iter()
                .flatten(),
        ) {
            set.remove(&key);
        }
    }
    for (from, to) in mappings {
        for key in scoped_keys(&source, from) {
            set.insert(format!("{to}{}", &key[from.len()..]));
        }
    }
}

fn scoped_keys(keys: &BTreeSet<String>, root: &str) -> Vec<String> {
    let mut result = Vec::new();
    if keys.contains(root) {
        result.push(root.to_owned());
    }
    for separator in ["/", "::"] {
        let prefix = format!("{root}{separator}");
        result.extend(
            keys.range(prefix.clone()..)
                .take_while(|key| key.starts_with(&prefix))
                .cloned(),
        );
    }
    result
}

fn affected_keys(keys: &BTreeSet<String>, mappings: &[(String, String)]) -> BTreeSet<String> {
    mappings
        .iter()
        .flat_map(|(from, to)| {
            scoped_keys(keys, from)
                .into_iter()
                .chain(scoped_keys(keys, to))
        })
        .collect()
}

fn replace_key_map<V: Clone>(
    map: &mut HashMap<String, V>,
    mappings: &[(String, String)],
    copy: bool,
) {
    if mappings.is_empty() {
        return;
    }
    let keys = map.keys().cloned().collect();
    let keys = affected_keys(&keys, mappings);
    let source = keys
        .iter()
        .filter_map(|key| map.get(key).cloned().map(|v| (key.clone(), v)))
        .collect::<HashMap<_, _>>();
    for (from, to) in mappings {
        for key in scoped_keys(&keys, to).into_iter().chain(
            (!copy)
                .then(|| scoped_keys(&keys, from))
                .into_iter()
                .flatten(),
        ) {
            map.remove(&key);
        }
    }
    for (from, to) in mappings {
        for key in scoped_keys(&keys, from) {
            map.insert(format!("{to}{}", &key[from.len()..]), source[&key].clone());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request(
        app: &mut App,
        work: impl FnOnce() -> Result<crate::books::BookOpResult, String> + Send + 'static,
    ) -> bool {
        app.request_book_structural_op(
            &egui::Context::default(),
            "book-structural-test",
            crate::books::BookOpIntent::SourceMutation,
            Completion::Manager,
            work,
        )
    }

    #[cfg(windows)]
    #[test]
    fn book_structure_rejects_mounted_and_parked_path_classification_without_consuming_open() {
        for parked in [false, true] {
            let mut app = setup_app_for_test();
            app.rename_migration_journal_admission = RenameMigrationJournalAdmission::Durable;
            let source = app.tmp.path().join("candidate.epub");
            std::fs::create_dir(&source).unwrap();
            let begin = |owner: &mut App| {
                owner.settings.sidecar_backup_enabled = false;
                assert_eq!(
                    owner.load_folder_or_convert_archive_with_auto_fullscreen_owned(
                        source.clone(),
                        false,
                        OpenRequestOwner::Navigation,
                        StartupListIntent::PreservePresentation,
                    ),
                    FolderOpenOutcome::Classifying
                );
                owner
                    .top_level_grid_view
                    .open_path_classification()
                    .unwrap()
                    .request_id
            };
            let (id, request_id) = if parked {
                let id = app.build_window_context_for_test(1272301, |owner| {
                    begin(owner);
                });
                let request_id = app
                    .with_owner_viewer_context(id, |owner| {
                        owner
                            .top_level_grid_view
                            .open_path_classification()
                            .unwrap()
                            .request_id
                    })
                    .unwrap();
                (id, request_id)
            } else {
                let request_id = begin(&mut app);
                (app.projected_viewer_context_id(), request_id)
            };
            assert!(
                app.sidecar_restore.is_none(),
                "classification precedes hydration"
            );
            assert!(!request(&mut app, || unreachable!(
                "rejected worker started"
            )));
            assert!(app.book_structural_op.is_none());
            app.with_owner_viewer_context(id, |owner| {
                assert_eq!(
                    owner
                        .top_level_grid_view
                        .open_path_classification()
                        .unwrap()
                        .request_id,
                    request_id
                );
                owner.settle_open_path_classification_for_test();
                owner.finish_page_identity_preparation_for_test();
                assert_eq!(owner.current_folder.as_deref(), Some(source.as_path()));
            })
            .unwrap();
            assert!(request(&mut app, || Err("test terminal".into())));
        }
    }

    #[test]
    fn book_structure_requested_waits_then_running_retains_admission_until_adoption() {
        let mut app = setup_app_for_test();
        app.rename_migration_journal_admission = RenameMigrationJournalAdmission::Durable;
        let handle = crate::remote_ipc::session::SessionHandle::new();
        app.set_remote_session_handle(handle.clone());
        app.book_bookmark_pending_requests.insert(42);
        let (started, entered) = mpsc::channel();
        let (release, finish) = mpsc::channel();
        assert!(request(&mut app, move || {
            started.send(()).unwrap();
            finish.recv().unwrap();
            Err("test terminal".into())
        }));
        let ctx = egui::Context::default();
        app.poll_book_structural_op(&ctx);
        assert!(entered.try_recv().is_err());
        assert!(matches!(
            app.book_structural_op.as_ref().unwrap().phase,
            Phase::Requested { .. }
        ));
        assert_eq!(app.modal_dialog_block_reason(), Some("book_structural_op"));
        assert!(!app.grid_item_input_allowed());
        assert!(handle.try_local_modal_lease().is_none());
        app.book_bookmark_pending_requests.clear();
        app.poll_book_structural_op(&ctx);
        entered
            .recv_timeout(std::time::Duration::from_secs(3))
            .unwrap();
        assert!(matches!(
            app.book_structural_op.as_ref().unwrap().phase,
            Phase::Running { .. }
        ));
        assert!(!request(&mut app, || unreachable!()));
        release.send(()).unwrap();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
        while app.book_structural_op.is_some() && std::time::Instant::now() < deadline {
            app.poll_book_structural_op(&ctx);
            std::thread::yield_now();
        }
        assert!(app.book_structural_op.is_none());
        assert!(handle.try_local_modal_lease().is_some());
    }

    #[test]
    fn book_structure_rejects_generic_queue_source_delete_and_remote_lease_conflicts() {
        let mut app = setup_app_for_test();
        app.rename_migration_journal_admission = RenameMigrationJournalAdmission::Durable;
        app.rename_migration_queue.push_back(
            crate::rename_key_migration::PathMigrationJob::shell_rename(
                "A".into(),
                "B".into(),
                true,
            ),
        );
        assert!(!request(&mut app, || unreachable!()));
        app.rename_migration_queue.clear();
        for intent in [
            crate::books::BookOpIntent::SourceMutation,
            crate::books::BookOpIntent::Delete { path: "B".into() },
        ] {
            let (_tx, rx) = mpsc::channel();
            app.book_op_pending = Some(crate::books::BookOpPending { rx, intent });
            assert!(!request(&mut app, || unreachable!()));
            assert!(app.book_structural_op.is_none());
        }
        app.book_op_pending = None;
        let handle = crate::remote_ipc::session::SessionHandle::new();
        let held = handle.try_local_modal_lease().unwrap();
        app.set_remote_session_handle(handle.clone());
        assert!(!request(&mut app, || unreachable!()));
        assert!(app.book_structural_op.is_none());
        drop(held);
        assert!(request(&mut app, || Err("test terminal".into())));
    }

    #[test]
    fn book_structure_presence_swaps_and_clears_destinations_from_missing_sources() {
        let mut keys = ["a", "a/x", "a::page1", "ab/sibling", "b/y", "stale/z"]
            .into_iter()
            .map(str::to_owned)
            .collect();
        replace_presence(
            &mut keys,
            &[
                ("a".into(), "b".into()),
                ("b".into(), "a".into()),
                ("missing".into(), "stale".into()),
            ],
            false,
        );
        assert_eq!(
            keys,
            ["a/y", "b", "b/x", "b::page1", "ab/sibling"]
                .into_iter()
                .map(str::to_owned)
                .collect()
        );
        replace_presence(&mut keys, &[("b".into(), "a".into())], true);
        assert_eq!(
            keys,
            ["a", "a/x", "a::page1", "b", "b/x", "b::page1", "ab/sibling"]
                .into_iter()
                .map(str::to_owned)
                .collect()
        );
    }

    #[test]
    fn book_structure_refreshes_parent_shelf_pins_without_resetting_unrelated_page_state() {
        let mut app = setup_app_for_test();
        let shelf = app.tmp.path().join("shelf");
        let book = shelf.join("Book");
        let other = shelf.join("Other");
        app.current_folder = Some(shelf.clone());
        app.items = vec![GridItem::Folder(book.clone())];
        app.rating_cache.insert(0, 4);
        app.tags_cache
            .insert("shelf-key".into(), vec!["keep".into()]);
        let sibling = app.build_window_context_for_test(1272002, |owner| {
            owner.items = vec![GridItem::Folder(other.clone())];
            owner.current_folder = Some(shelf.clone());
            owner.rating_cache.insert(0, 3);
        });
        app.adopt_book_path_metadata(
            &[crate::books::BookPathMapping {
                from: book.join("0001.jpg"),
                to: book.join("0002.jpg"),
            }],
            &[],
            None,
        );
        let main = app.projected_viewer_context_id();
        assert!(matches!(
            &app.current_view_pin_refreshes[&main].refresh,
            CurrentViewRefresh::Pins { folders, videos }
                if folders.len() == 1
                    && folders.iter().any(|p| crate::path_key::eq_keep_drive(p, &book))
                    && videos.is_empty()
        ));
        assert!(!app.current_view_pin_refreshes.contains_key(&sibling));
        assert_eq!(app.rating_cache.get(&0), Some(&4));
        assert!(app.tags_cache.contains_key("shelf-key"));
        app.with_owner_viewer_context(sibling, |owner| {
            assert_eq!(owner.rating_cache.get(&0), Some(&3));
        })
        .unwrap();
    }

    #[test]
    fn book_structure_preserves_unsaved_trim_when_pre_fs_save_fails() {
        let mut app = setup_app_for_test();
        app.rename_migration_journal_admission = RenameMigrationJournalAdmission::Durable;
        let folder = app.tmp.path().join("Book");
        app.current_folder = Some(folder.clone());
        app.items = vec![GridItem::Image(folder.join("0001.jpg"))];
        app.view_trim_page_overrides.insert(0, Default::default());
        app.view_trim_dirty_page_overrides.insert(0);
        app.view_trim_save_pending = true;
        let blocked = app.tmp.path().join("not-a-directory");
        std::fs::write(&blocked, b"blocked").unwrap();
        app.rename_migration_data_dir_override = Some(blocked);
        let called = Arc::new(AtomicBool::new(false));
        let worker_called = Arc::clone(&called);
        assert!(request(&mut app, move || {
            worker_called.store(true, Ordering::Release);
            Err("must not begin filesystem".into())
        }));
        let ctx = egui::Context::default();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while app.book_structural_op.is_some() {
            assert!(std::time::Instant::now() < deadline);
            app.poll_book_structural_op(&ctx);
            std::thread::yield_now();
        }
        assert!(!called.load(Ordering::Acquire));
        assert!(app.view_trim_save_pending);
        assert!(app.view_trim_dirty_page_overrides.contains(&0));
    }

    #[test]
    fn book_structure_rejects_append_and_undo_owners() {
        for phase in ["append", "undo"] {
            let (mut app, _receipt, _sender) = book_append_producer_tests::followup_fixture(phase);
            assert!(!request(&mut app, || unreachable!()));
            assert!(app.book_structural_op.is_none());
        }
    }

    #[test]
    fn book_structure_waits_for_unconsumed_tag_result_and_its_identity_record() {
        let mut app = setup_app_for_test();
        app.rename_migration_journal_admission = RenameMigrationJournalAdmission::Durable;
        let page = app.tmp.path().join("tagged.jpg");
        std::fs::write(&page, b"tagged source").unwrap();
        app.items = vec![GridItem::Image(page.clone())];
        app.selected = Some(0);
        app.request_tag_add_for_selection("structure", ActionSurface::MainWindow);
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        while app.tag_write_handle.as_ref().unwrap().is_busy() {
            assert!(std::time::Instant::now() < deadline);
            std::thread::yield_now();
        }
        assert!(
            app.tag_write_handle
                .as_ref()
                .unwrap()
                .has_unconsumed_batch()
        );
        let (started, entered) = mpsc::channel();
        assert!(request(&mut app, move || {
            started.send(()).unwrap();
            Err("test terminal".into())
        }));
        let ctx = egui::Context::default();
        app.poll_book_structural_op(&ctx);
        assert!(entered.try_recv().is_err());
        app.poll_tag_write_results();
        while app.book_structural_writers_busy() {
            assert!(std::time::Instant::now() < deadline);
            std::thread::yield_now();
        }
        app.poll_book_structural_op(&ctx);
        entered
            .recv_timeout(std::time::Duration::from_secs(3))
            .unwrap();
    }

    #[test]
    fn book_structure_waits_for_delayed_history_and_content_identity_writers() {
        let mut app = setup_app_for_test();
        app.rename_migration_journal_admission = RenameMigrationJournalAdmission::Durable;
        let history_path = app.tmp.path().join("held-history.db");
        app.reading_history_writer =
            crate::reading_history_db::ReadingHistoryWriter::spawn_at_for_test(
                history_path.clone(),
            );
        app.reading_history_writer.as_ref().unwrap().prune(10);
        let ledger_path = app.tmp.path().join("held-identity.db");
        app.content_identity_recorder =
            crate::content_identity::ContentIdentityRecorder::spawn_at_for_test(
                ledger_path.clone(),
            );
        let page = app.tmp.path().join("source.jpg");
        std::fs::write(&page, b"source pixels").unwrap();
        let source = crate::content_identity::ContentIdentitySource::new(
            &page,
            crate::content_identity::ContentKind::Image,
        );
        app.content_identity_recorder.as_ref().unwrap().record(
            source.clone(),
            crate::content_identity::ContentIdentityTrigger::Edit,
        );
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        while app.reading_history_writer.as_ref().unwrap().is_busy()
            || app.content_identity_recorder.as_ref().unwrap().is_busy()
        {
            assert!(std::time::Instant::now() < deadline);
            std::thread::yield_now();
        }
        let history = rusqlite::Connection::open(history_path).unwrap();
        let ledger = rusqlite::Connection::open(ledger_path).unwrap();
        history.execute_batch("BEGIN IMMEDIATE").unwrap();
        ledger.execute_batch("BEGIN IMMEDIATE").unwrap();
        app.reading_history_writer.as_ref().unwrap().prune(10);
        app.content_identity_recorder.as_ref().unwrap().record(
            source,
            crate::content_identity::ContentIdentityTrigger::Edit,
        );
        let (started, entered) = mpsc::channel();
        assert!(request(&mut app, move || {
            started.send(()).unwrap();
            Err("test terminal".into())
        }));
        let ctx = egui::Context::default();
        app.poll_book_structural_op(&ctx);
        assert!(entered.try_recv().is_err());
        history.execute_batch("ROLLBACK").unwrap();
        while app.reading_history_writer.as_ref().unwrap().is_busy() {
            assert!(std::time::Instant::now() < deadline);
            std::thread::yield_now();
        }
        app.poll_book_structural_op(&ctx);
        assert!(entered.try_recv().is_err());
        ledger.execute_batch("ROLLBACK").unwrap();
        while app.content_identity_recorder.as_ref().unwrap().is_busy() {
            assert!(std::time::Instant::now() < deadline);
            std::thread::yield_now();
        }
        app.poll_book_structural_op(&ctx);
        entered
            .recv_timeout(std::time::Duration::from_secs(3))
            .unwrap();
    }

    #[cfg(windows)]
    #[test]
    fn book_structure_parked_xmp_result_runs_db_on_worker_and_waits_for_adoption() {
        let mut app = setup_app_for_test();
        app.rename_migration_journal_admission = RenameMigrationJournalAdmission::Durable;
        let page = app.tmp.path().join("parked-xmp.png");
        let key = crate::adjustment_db::normalize_path(&page);
        let db_path = app.tmp.path().join("parked-xmp-ratings.db");
        app.rating_db = Some(crate::rating_db::RatingDb::open_at(&db_path).unwrap());
        let parked = app.build_window_context_for_test(1272201, |owner| {
            owner.install_new_items(vec![GridItem::Image(page.clone())], vec![None]);
            owner.rebuild_visible_indices();
            owner.tag_prewarm_pending = Some(
                crate::tag_prewarm::TagPrewarmPending::from_results_for_test(vec![
                    crate::tag_prewarm::TagPrewarmResult {
                        path: page.clone(),
                        rating: Some(3),
                        rating_generation: 0,
                    },
                ]),
            );
        });
        let (entered_tx, entered_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        let (completed_tx, completed_rx) = mpsc::channel();
        app.rating_xmp_hydration
            .hold_next_import_for_test(crate::tag_prewarm::HydrationTestHook {
                entered: entered_tx,
                release: release_rx,
                completed: completed_tx,
            });
        let (started_tx, started_rx) = mpsc::channel();
        let ui_db_reads = crate::rating_db::read_operations_for_test();
        let ui_point_reads = crate::rating_db::point_read_operations_for_test();
        assert!(request(&mut app, move || {
            started_tx.send(()).unwrap();
            Err("test terminal".into())
        }));
        // The pure predicate must leave the buffered result in its parked owner.
        assert!(app.book_structural_writers_busy());
        assert!(app.book_structural_writers_busy());
        assert!(!app.rating_xmp_hydration.is_busy());
        assert!(entered_rx.try_recv().is_err());
        let ctx = egui::Context::default();
        app.poll_book_structural_op(&ctx);
        entered_rx
            .recv_timeout(std::time::Duration::from_secs(5))
            .unwrap();
        // The frame has returned while the DB worker is held before SQLite.
        assert!(app.rating_xmp_hydration.is_busy());
        assert!(started_rx.try_recv().is_err());
        assert_eq!(crate::rating_db::read_operations_for_test(), ui_db_reads);
        assert_eq!(
            crate::rating_db::point_read_operations_for_test(),
            ui_point_reads
        );
        assert!(
            app.rating_db
                .as_ref()
                .unwrap()
                .get_many(&[key.clone()])
                .is_empty()
        );
        release_tx.send(()).unwrap();
        completed_rx
            .recv_timeout(std::time::Duration::from_secs(5))
            .unwrap();
        assert!(
            app.book_structural_writers_busy(),
            "DB completion still requires UI adoption"
        );
        assert!(started_rx.try_recv().is_err());
        app.poll_book_structural_op(&ctx);
        started_rx
            .recv_timeout(std::time::Duration::from_secs(5))
            .unwrap();
        assert_eq!(
            app.rating_db
                .as_ref()
                .unwrap()
                .get_many(&[key.clone()])
                .get(&key),
            Some(&3)
        );
        assert_eq!(app.rating_session_value_for_key(&key), Some(3));
        app.with_owner_viewer_context(parked, |owner| {
            assert_eq!(owner.rating_cache.get(&0), Some(&3));
        })
        .unwrap();
    }

    #[test]
    fn book_structure_moves_rating_ledger_and_preserves_independent_reader_caches() {
        let mut app = setup_app_for_test();
        let a = app.tmp.path().join("0001_same.jpg");
        let b = app.tmp.path().join("0002_same.jpg");
        let ka = crate::adjustment_db::normalize_path(&a);
        let kb = crate::adjustment_db::normalize_path(&b);
        app.items = vec![GridItem::Image(a.clone()), GridItem::Image(b.clone())];
        app.current_folder = Some(app.tmp.path().into());
        app.rating_cache.insert(0, 5);
        app.rating_session_writes.insert(
            ka.clone(),
            RatingSessionWrite {
                generation: 1,
                stars: 5,
                explicit_user: true,
            },
        );
        app.rating_session_writes.insert(
            kb.clone(),
            RatingSessionWrite {
                generation: 2,
                stars: 1,
                explicit_user: true,
            },
        );
        let other = app.build_window_context_for_test(1272001, |owner| {
            owner.items = vec![GridItem::Image(a.clone())];
            owner.current_folder = a.parent().map(Path::to_path_buf);
            owner.tags_cache.insert(ka.clone(), vec!["old page".into()]);
            owner.rating_cache.insert(0, 5);
        });
        app.adopt_book_path_metadata(
            &[
                crate::books::BookPathMapping {
                    from: a.clone(),
                    to: b.clone(),
                },
                crate::books::BookPathMapping {
                    from: b.clone(),
                    to: a.clone(),
                },
            ],
            &[],
            None,
        );
        assert_eq!(app.rating_session_value_for_key(&ka), Some(1));
        assert_eq!(app.rating_session_value_for_key(&kb), Some(5));
        app.with_owner_viewer_context(other, |owner| {
            assert_eq!(owner.tags_cache.get(&ka), Some(&vec!["old page".into()]));
            assert_eq!(owner.rating_cache.get(&0), Some(&5));
            assert!(matches!(&owner.items[0], GridItem::Image(path) if path == &a));
        })
        .unwrap();
        assert!(app.rating_cache.is_empty());
        app.adopt_book_path_metadata(
            &[],
            &[crate::books::BookPathMapping {
                from: b.clone(),
                to: a.clone(),
            }],
            None,
        );
        assert_eq!(app.rating_session_value_for_key(&ka), Some(5));
        let absent = app.tmp.path().join("absent.jpg");
        app.adopt_book_path_metadata(
            &[],
            &[crate::books::BookPathMapping {
                from: absent,
                to: a.clone(),
            }],
            None,
        );
        assert_eq!(app.rating_session_value_for_key(&ka), None);
    }

    fn seed_reader_caches(app: &mut App, page: &Path) {
        let key = crate::adjustment_db::normalize_path(page);
        app.rating_cache.insert(0, 5);
        app.current_folder_rating_cache = Some(4);
        app.tags_cache.insert(key, vec!["original".into()]);
        let mut params = crate::adjustment::AdjustParams::default();
        params.brightness = 42.0;
        app.adjustment_page_params.insert(0, params);
        app.view_trim_page_overrides.insert(0, Default::default());
        app.rotation_cache
            .insert(0, crate::rotation_db::Rotation::None);
        app.fs_cache.insert(0, FsCacheEntry::Failed);
    }

    fn assert_reader_caches(app: &App, page: &Path, generation: u64) {
        assert!(matches!(&app.items[0], GridItem::Image(path) if path == page));
        assert_eq!(app.items_generation, generation);
        assert_eq!(app.rating_cache.get(&0), Some(&5));
        assert_eq!(app.current_folder_rating_cache, Some(4));
        let key = crate::adjustment_db::normalize_path(page);
        assert_eq!(app.tags_cache.get(&key), Some(&vec!["original".into()]));
        assert_eq!(app.adjustment_page_params[&0].brightness, 42.0);
        assert!(app.view_trim_page_overrides.contains_key(&0));
        assert_eq!(
            app.rotation_cache.get(&0),
            Some(&crate::rotation_db::Rotation::None)
        );
        assert!(matches!(app.fs_cache.get(&0), Some(FsCacheEntry::Failed)));
    }

    #[test]
    fn book_structure_adoption_preserves_mounted_and_parked_reader_items_caches_and_shift() {
        for operation in ["move", "copy", "rename"] {
            let mut app = setup_app_for_test();
            let root = app.tmp.path().join("Book");
            let renamed = app.tmp.path().join("Renamed");
            let a = root.join("0001_same.jpg");
            let b = root.join("0002_same.jpg");
            app.current_folder = Some(root.clone());
            app.items = vec![GridItem::Image(a.clone()), GridItem::Image(b.clone())];
            seed_reader_caches(&mut app, &a);
            let window = app.build_window_context_for_test(1272101, |owner| {
                owner.current_folder = Some(root.clone());
                owner.items = vec![GridItem::Image(a.clone()), GridItem::Image(b.clone())];
                owner.spread_shift.scope =
                    Some(crate::book_scope::BookScopeKey::folder(&root).unwrap());
                owner.spread_shift.state = spread_shift_owner::ViewerShift::Book(
                    crate::spread_shift::SpreadShiftState::Adjusted {
                        anchor_ref: crate::spread_shift::SpreadShiftPageRef::RelativePath(
                            "0002_same.jpg".into(),
                        ),
                        resolution: crate::spread_shift::ShiftResolution::Resolved(1),
                        persistence: crate::spread_shift::ShiftPersistence::SessionOnly,
                    },
                );
                seed_reader_caches(owner, &a);
            });
            let (generation, shift) = app
                .with_owner_viewer_context(window, |owner| {
                    (owner.items_generation, owner.spread_shift.clone())
                })
                .unwrap();
            let moves = if operation == "rename" {
                vec![crate::books::BookPathMapping {
                    from: root.clone(),
                    to: renamed.clone(),
                }]
            } else {
                vec![crate::books::BookPathMapping {
                    from: b.clone(),
                    to: a.clone(),
                }]
            };
            let root_rename =
                (operation == "rename").then_some((root.as_path(), renamed.as_path()));
            // Adoption must select the main owner even when F12 is mounted.
            app.with_owner_viewer_context(window, |owner| {
                if operation == "copy" {
                    owner.adopt_book_path_metadata(&[], &moves, None);
                } else {
                    owner.apply_book_page_edit_moves_with_journal(&moves, None, root_rename);
                }
                assert_reader_caches(owner, &a, generation);
                assert_eq!(owner.spread_shift, shift);
            })
            .unwrap();
            app.with_owner_viewer_context(window, |owner| {
                assert_reader_caches(owner, &a, generation);
                assert_eq!(owner.spread_shift, shift);
            })
            .unwrap();
            assert!(app.rating_cache.is_empty());
            assert!(!app.current_view_pin_refreshes.contains_key(&window));
        }
    }

    #[test]
    fn book_structure_snapshot_adoption_does_not_rebuild_or_remap_context() {
        let mut app = setup_app_for_test();
        let root = app.tmp.path().join("Book");
        let renamed = app.tmp.path().join("Renamed");
        let page = root.join("0001_same.jpg");
        app.current_folder = Some(root.clone());
        app.items = vec![GridItem::Image(page.clone())];
        app.thumbnails = vec![ThumbnailState::Pending];
        app.visible_indices = vec![0];
        app.spread_shift.scope = Some(crate::book_scope::BookScopeKey::folder(&root).unwrap());
        app.spread_shift.state = spread_shift_owner::ViewerShift::Book(
            crate::spread_shift::SpreadShiftState::Adjusted {
                anchor_ref: crate::spread_shift::SpreadShiftPageRef::RelativePath(
                    "0001_same.jpg".into(),
                ),
                resolution: crate::spread_shift::ShiftResolution::Resolved(0),
                persistence: crate::spread_shift::ShiftPersistence::SessionOnly,
            },
        );
        app.activate_snapshot(crate::snapshot::SnapshotSourceLabel::Mixed);
        assert!(app.is_snapshot_active());
        seed_reader_caches(&mut app, &page);
        let generation = app.items_generation;
        let shift = app.spread_shift.clone();
        let saved_shift = app
            .snapshot
            .as_ref()
            .unwrap()
            .saved_viewing_state
            .shift
            .clone();
        // Only direct adoption is exercised here. A real worker also publishes
        // PAGE_EDIT_WRITES; its normal old-path reconciliation is intentionally
        // unchanged (§3.10) and may subsequently replace editing caches.
        for (from, to, root_move) in [
            (page.clone(), root.join("0002_same.jpg"), None),
            (
                root.clone(),
                renamed.clone(),
                Some((root.as_path(), renamed.as_path())),
            ),
        ] {
            app.apply_book_page_edit_moves_with_journal(
                &[crate::books::BookPathMapping { from, to }],
                None,
                root_move,
            );
            assert_reader_caches(&app, &page, generation);
            assert_eq!(app.spread_shift, shift);
            assert_eq!(
                app.snapshot.as_ref().unwrap().saved_viewing_state.shift,
                saved_shift
            );
            assert!(app.current_view_pin_refreshes.is_empty());
        }
        let key = crate::adjustment_db::normalize_path(&page);
        app.adjusted_page_keys.insert(key.clone());
        let (tx, rx) = mpsc::channel();
        tx.send(Ok(crate::books::BookOpResult::Deleted {
            name: "Book".into(),
            path: root.clone(),
            metadata: Default::default(),
        }))
        .unwrap();
        app.book_structural_op = Some(BookStructuralOp::running_for_test(
            rx,
            crate::books::BookOpIntent::Delete { path: root },
            Completion::Manager,
        ));
        app.poll_book_structural_op(&egui::Context::default());
        assert_reader_caches(&app, &page, generation);
        assert!(!app.adjusted_page_keys.contains(&key));
        assert!(app.book_structural_op.is_none());
    }

    #[test]
    fn book_structure_unreloaded_main_extraction_keeps_page_caches() {
        for root_rename in [false, true] {
            let mut app = setup_app_for_test();
            let root = app.tmp.path().join("Book");
            let renamed = app.tmp.path().join("Renamed");
            let page = root.join("0001_same.jpg");
            app.current_folder = (!root_rename).then(|| app.tmp.path().join("search-results"));
            app.items = vec![GridItem::Image(page.clone())];
            seed_reader_caches(&mut app, &page);
            let generation = app.items_generation;
            let mapping = if root_rename {
                crate::books::BookPathMapping {
                    from: root.clone(),
                    to: renamed.clone(),
                }
            } else {
                crate::books::BookPathMapping {
                    from: page.clone(),
                    to: root.join("0002_same.jpg"),
                }
            };
            app.adopt_book_path_metadata(
                &[mapping],
                &[],
                root_rename.then_some((root.as_path(), renamed.as_path())),
            );
            assert_reader_caches(&app, &page, generation);
            assert!(app.current_view_pin_refreshes.is_empty());
            if root_rename {
                // A collection root has no backing folder, so even Delete's
                // pending_reload cannot rebuild these accepted rows.
                let (tx, rx) = mpsc::channel();
                tx.send(Ok(crate::books::BookOpResult::Deleted {
                    name: "Book".into(),
                    path: root.clone(),
                    metadata: Default::default(),
                }))
                .unwrap();
                app.book_structural_op = Some(BookStructuralOp::running_for_test(
                    rx,
                    crate::books::BookOpIntent::Delete { path: root },
                    Completion::Manager,
                ));
                app.poll_book_structural_op(&egui::Context::default());
                assert_reader_caches(&app, &page, generation);
            }
        }
    }
}
