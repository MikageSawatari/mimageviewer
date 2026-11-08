//! Grid activation and first-display intent for one archive level.
use super::*;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ArchiveBookTarget {
    pub(crate) requested_prefix: String,
    pub(crate) scope: crate::book_scope::BookScopeKey,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[must_use]
pub(crate) enum ZipDirActivationOutcome {
    NotApplicable,
    Refused,
    List,
    BookRequested,
}

enum ArchiveLevelOpenIntent {
    List,
    Book(ArchiveBookTarget),
    UnsupportedBook {
        logical_root: PathBuf,
        reason: String,
    },
}

impl App {
    /// The only producer of a chosen internal list. Programmatic ZIP navigation
    /// keeps its explicit-list/page-continuation intent and never calls here.
    pub(crate) fn activate_main_zipdir(&mut self, idx: usize) -> ZipDirActivationOutcome {
        if self.projected_viewer_context_id() != self.viewer_context_main()
            || self.items_are_rating_view
        {
            return ZipDirActivationOutcome::NotApplicable;
        }
        let Some(GridItem::ZipDir {
            zip_path,
            dir_prefix,
            ..
        }) = self.items.get(idx)
        else {
            return ZipDirActivationOutcome::NotApplicable;
        };
        let Some(nav) = self.zip_nav.as_ref() else {
            return ZipDirActivationOutcome::NotApplicable;
        };
        if !self.grid_item_input_allowed()
            || !crate::folder_tree::path_eq(zip_path, &nav.tree.zip_path)
            || self
                .current_folder
                .as_deref()
                .is_none_or(|path| !crate::folder_tree::path_eq(path, &nav.tree.zip_path))
        {
            return ZipDirActivationOutcome::Refused;
        }
        let requested_prefix = dir_prefix.clone();
        let segments = requested_prefix
            .trim_end_matches('/')
            .split('/')
            .map(str::to_owned)
            .collect::<Vec<_>>();
        if segments.len() != nav.current().len() + 1
            || !segments.starts_with(nav.current())
            || nav.tree.node_at(&segments).is_none()
        {
            return ZipDirActivationOutcome::Refused;
        }
        if self.is_snapshot_active() {
            self.maybe_suppress_rating_filter_for_opened_zip_book(idx);
            self.zip_nav_enter(&requested_prefix);
            return ZipDirActivationOutcome::List;
        }
        let mut destination = nav.clone();
        destination.choose_current_list(Some(self.selected_rows_below_view_top(idx)));
        destination.enter(&requested_prefix);
        // S3 will route book candidates to a new independent bundle. S1 retains
        // the released multi-window list navigation and never opens main fullscreen.
        let book_candidate = self.settings.effective_auto_fullscreen_zip_pdf()
            && !self.settings.detached_viewer_open_images_in_window
            && destination
                .tree
                .node_at(destination.current())
                .is_some_and(|node| !node.images.is_empty());
        let intent = if book_candidate {
            let logical = self
                .archive_source_override
                .as_deref()
                .unwrap_or(&destination.tree.zip_path);
            match crate::book_scope::BookScopeKey::archive(logical, destination.current_raw()) {
                Ok(scope) => ArchiveLevelOpenIntent::Book(ArchiveBookTarget {
                    requested_prefix,
                    scope,
                }),
                Err(reason) => ArchiveLevelOpenIntent::UnsupportedBook {
                    logical_root: logical.to_path_buf(),
                    reason,
                },
            }
        } else {
            ArchiveLevelOpenIntent::List
        };
        self.capture_main_list_restore_cursor();
        self.maybe_suppress_rating_filter_for_opened_zip_book(idx);
        self.accept_zip_level_navigation();
        self.zip_nav = Some(destination);
        self.zip_nav_show_current_level(if matches!(&intent, ArchiveLevelOpenIntent::Book(_)) {
            StartupListIntent::PageContinuation
        } else {
            StartupListIntent::ExplicitList
        });
        let target = match intent {
            ArchiveLevelOpenIntent::List => return ZipDirActivationOutcome::List,
            ArchiveLevelOpenIntent::Book(target) => target,
            ArchiveLevelOpenIntent::UnsupportedBook {
                logical_root,
                reason,
            } => {
                self.accept_unsupported_archive_composition(
                    &logical_root,
                    reason,
                    "この内部フォルダは本として開けないため、一覧を表示します",
                    crate::feedback::FeedbackKind::Info,
                );
                return ZipDirActivationOutcome::List;
            }
        };
        if !self.archive_book_target_is_current(&target) {
            return ZipDirActivationOutcome::Refused;
        }
        let deferred = DeferredFsReopen {
            history_trigger: HistoryTrigger::UserChosen,
            resume_slideshow: false,
            target: DeferredFsTarget::CurrentArchiveBook(target),
            // S2 enables inner-scope resume atomically with its save/fence contract.
            resume_to_last_page: false,
            from_explicit_open: true,
            preserve_after_password_prompt: false,
        };
        self.fs_suppress_enter_close_until_release = true;
        if self.attach_archive_book_index_continuation(deferred) {
            ZipDirActivationOutcome::BookRequested
        } else {
            crate::logger::log("archive book open: accepted level has no index continuation");
            self.show_feedback_warning(
                "本の準備を開始できませんでした。一覧を再読み込みしてください".into(),
            );
            ZipDirActivationOutcome::Refused
        }
    }

    pub(super) fn archive_book_target_is_current(&self, target: &ArchiveBookTarget) -> bool {
        let Some(nav) = self.zip_nav.as_ref() else {
            return false;
        };
        let logical = self
            .archive_source_override
            .as_deref()
            .unwrap_or(&nav.tree.zip_path);
        let requested = target
            .requested_prefix
            .trim_end_matches('/')
            .split('/')
            .map(str::to_owned)
            .collect::<Vec<_>>();
        nav.tree.node_at(&requested).is_some()
            && nav.tree.collapse_redundant(&requested) == nav.current()
            && crate::book_scope::BookScopeKey::archive(logical, nav.current_raw()).as_ref()
                == Ok(&target.scope)
    }

    /// Composition defines order; the final visible set defines eligibility.
    pub(super) fn first_visible_archive_book_page(&self) -> Option<usize> {
        let eligible = |idx: usize| {
            self.visible_indices.contains(&idx)
                && matches!(self.items.get(idx), Some(GridItem::ZipImage { .. }))
        };
        match self.composed_book_layout() {
            Some(layout) => layout
                .visible_pages
                .iter()
                .map(|page| page.item_index)
                .find(|&idx| eligible(idx)),
            None => self
                .current_reader_order()
                .iter()
                .copied()
                .find(|&idx| eligible(idx)),
        }
    }

    pub(super) fn chosen_zip_list_return(&self) -> Option<crate::zip_tree::ZipListReturnPlan> {
        if self.projected_viewer_context_id() != self.viewer_context_main()
            || !self.settings.effective_auto_fullscreen_zip_pdf()
            || self.is_snapshot_active()
        {
            return None;
        }
        self.zip_nav.as_ref()?.chosen_list_return_plan()
    }

    pub(super) fn apply_zip_list_return(&mut self, plan: crate::zip_tree::ZipListReturnPlan) {
        if self.projected_viewer_context_id() != self.viewer_context_main()
            || self.is_snapshot_active()
            || self
                .zip_nav
                .as_ref()
                .is_none_or(|nav| !nav.matches_list_return_plan(&plan))
        {
            crate::logger::log("archive list return dropped: stale tree/choice/context");
            return;
        }
        // A Terminal recipe has already torn its viewer down. A Live recipe must
        // finish on the OLD page rows, before replacing indices with list cells.
        if self.fullscreen_idx.is_some() {
            #[cfg(windows)]
            {
                self.transfer_fullscreen_viewport_retirement();
                self.begin_active_detached_session_close("archive_list_return");
                self.finish_active_detached_session_close("archive_list_return");
            }
            self.close_fullscreen_for_destination(FullscreenCloseDestination::ChosenZipList);
        }
        let Some(nav) = self.zip_nav.as_mut() else {
            return;
        };
        if !nav.apply_list_return_plan(&plan) {
            crate::logger::log("archive list return dropped after teardown: stale choice");
            return;
        }
        self.accept_zip_level_navigation();
        self.zip_nav_show_current_level(StartupListIntent::ExplicitList);
        let selected = self.items.iter().position(|item| match item {
            GridItem::ZipDir { dir_prefix, .. } => {
                let segments = dir_prefix
                    .split('/')
                    .filter(|part| !part.is_empty())
                    .collect::<Vec<_>>();
                !segments.is_empty()
                    && segments.len() <= plan.left_prefix().len()
                    && segments
                        .iter()
                        .zip(plan.left_prefix())
                        .all(|(a, b)| *a == b)
            }
            _ => false,
        });
        self.move_grid_cursor(selected, GridCursorIntent::Automatic);
        self.grid_scroll.set_row_anchor(plan.rows_above());
        self.grid_scroll.set_reveal_requested(true);
        self.capture_main_list_restore_cursor();
    }
}
