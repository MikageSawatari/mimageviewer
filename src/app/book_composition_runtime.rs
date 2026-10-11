//! One context-owned read/projection transaction; workers prepare rows and edit identities.
//! The accepted immutable record travels with its viewer, including parked and snapshot views.
use super::App;
use crate::book_composition::{
    AcceptedCompositionProjection, CompositionPrepareReply, CompositionProjectionOutcome,
    CompositionToken, PageDomain, StoredComposition,
};
use crate::book_scope::BookScopeKey;
use std::sync::{Arc, mpsc};

#[derive(Clone, Debug, Default)]
pub(crate) enum BookCompositionSource {
    #[default]
    Unavailable,
    Archive {
        tree: Arc<crate::zip_tree::ZipTree>,
        prefix: Vec<String>,
        display_prefix: Vec<String>,
    },
    Document {
        count: u32,
        metadata: Option<(i64, i64)>,
        physical_source: std::path::PathBuf,
    },
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum ProjectionAdoptionCause {
    #[default]
    Normal,
    ProjectionRefresh,
}

#[derive(Clone, Debug, Default)]
pub(crate) enum ProjectionOrigin {
    #[default]
    Fresh,
    Refresh {
        source: BookCompositionSource,
        previous: Option<Arc<AcceptedCompositionProjection>>,
        anchor: Option<crate::book_resume_db::PageIdentity>,
    },
}

pub(crate) struct BookProjectionReply {
    pub(crate) rows: super::book_materialization::MaterializedBookRows,
    pub(crate) landing: Option<usize>,
    pub(crate) edits: Option<super::page_edit_snapshot::StablePageEditProjection>,
    pub(crate) automatic_preferences: Option<crate::spread_db::StoredSpreadState>,
    pub(crate) refresh_landings: std::collections::HashMap<usize, Option<usize>>,
    pub(crate) saved_trim: Option<crate::view_trim_db::ViewTrimWriteBatch>,
}

#[derive(Default)]
pub(crate) enum ViewerBookComposition {
    #[default]
    Automatic,
    Preparing {
        scope: BookScopeKey,
        receiver: mpsc::Receiver<CompositionPrepareReply>,
        origin: ProjectionOrigin,
    },
    Projecting {
        scope: BookScopeKey,
        token: CompositionToken,
        record: Option<Arc<StoredComposition>>,
        source: BookCompositionSource,
        generation: u64,
        origin: ProjectionOrigin,
        receiver: mpsc::Receiver<Result<BookProjectionReply, String>>,
    },
    Accepted {
        scope: BookScopeKey,
        token: CompositionToken,
        record: Option<Arc<StoredComposition>>,
        layout: CompositionProjectionOutcome,
        source: BookCompositionSource,
        generation: u64,
        cause: ProjectionAdoptionCause,
    },
    Blocked {
        scope: BookScopeKey,
        token: CompositionToken,
        error: String,
    },
}

impl Clone for ViewerBookComposition {
    fn clone(&self) -> Self {
        match self {
            Self::Automatic | Self::Preparing { .. } | Self::Projecting { .. } => Self::Automatic,
            Self::Accepted {
                scope,
                token,
                record,
                layout,
                source,
                generation,
                cause,
            } => Self::Accepted {
                scope: scope.clone(),
                token: *token,
                record: record.clone(),
                layout: layout.clone(),
                source: source.clone(),
                generation: *generation,
                cause: *cause,
            },
            Self::Blocked {
                scope,
                token,
                error,
            } => Self::Blocked {
                scope: scope.clone(),
                token: *token,
                error: error.clone(),
            },
        }
    }
}
impl std::fmt::Debug for ViewerBookComposition {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Automatic => f.write_str("Automatic"),
            Self::Preparing { scope, .. } => f.debug_tuple("Preparing").field(scope).finish(),
            Self::Projecting { scope, .. } => f.debug_tuple("Projecting").field(scope).finish(),
            Self::Accepted {
                scope,
                token,
                record,
                ..
            } => f
                .debug_tuple("Accepted")
                .field(scope)
                .field(token)
                .field(record)
                .finish(),
            Self::Blocked {
                scope,
                token,
                error,
            } => f
                .debug_tuple("Blocked")
                .field(scope)
                .field(token)
                .field(error)
                .finish(),
        }
    }
}
impl PartialEq for ViewerBookComposition {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Automatic, Self::Automatic) => true,
            (
                Self::Accepted {
                    scope: a,
                    token: b,
                    record: c,
                    layout: d,
                    generation: e,
                    cause: f,
                    ..
                },
                Self::Accepted {
                    scope: x,
                    token: y,
                    record: z,
                    layout: u,
                    generation: v,
                    cause: w,
                    ..
                },
            ) => (a, b, c, d, e, f) == (x, y, z, u, v, w),
            (
                Self::Blocked {
                    scope: a,
                    token: b,
                    error: c,
                },
                Self::Blocked {
                    scope: x,
                    token: y,
                    error: z,
                },
            ) => (a, b, c) == (x, y, z),
            _ => false,
        }
    }
}

impl App {
    pub(crate) fn composition_hides_snapshot_target(
        &self,
        target: &crate::snapshot::SnapshotTarget,
    ) -> bool {
        let ViewerBookComposition::Accepted {
            scope,
            layout: CompositionProjectionOutcome::Composed(projection),
            ..
        } = &self.book_composition
        else {
            return false;
        };
        let (root, page) = match target {
            crate::snapshot::SnapshotTarget::PdfPage { pdf_path, page_num } => (
                pdf_path,
                crate::book_composition::CompositionPageRef::PdfPage {
                    page_num: *page_num,
                },
            ),
            crate::snapshot::SnapshotTarget::ZipImage {
                zip_path,
                entry_name,
            } => (
                zip_path,
                crate::book_composition::CompositionPageRef::ArchiveEntry {
                    entry_name: entry_name.clone(),
                },
            ),
            _ => return false,
        };
        let same_source = crate::folder_tree::path_eq(root, &scope.logical_root)
            || self
                .zip_nav
                .as_ref()
                .is_some_and(|nav| crate::folder_tree::path_eq(root, &nav.tree.zip_path));
        same_source
            && projection.resolved.page_status(&page)
                == crate::book_composition::PageStatus::HiddenByComposition
    }
    pub(crate) fn composition_hides_bookmark(
        &self,
        bookmark: &crate::book_bookmarks::BookBookmark,
    ) -> bool {
        let ViewerBookComposition::Accepted {
            scope,
            layout: CompositionProjectionOutcome::Composed(projection),
            ..
        } = &self.book_composition
        else {
            return false;
        };
        if !crate::folder_tree::path_eq(&scope.logical_root, &bookmark.container_path) {
            return false;
        }
        let container = crate::path_key::normalize_keep_drive(&bookmark.container_path);
        let identity = match &bookmark.page_identity {
            crate::book_bookmarks::PageIdentity::ArchiveEntry(entry_name) => {
                crate::book_resume_db::PageIdentity::Archive {
                    container,
                    entry_name: entry_name.clone(),
                }
            }
            crate::book_bookmarks::PageIdentity::PdfPage(page_num) => {
                crate::book_resume_db::PageIdentity::Document {
                    container,
                    page_num: *page_num as usize,
                }
            }
            crate::book_bookmarks::PageIdentity::RelativePath(_) => return false,
        };
        projection.resolve_position(&crate::book_resume_db::ReadingPosition {
            page: bookmark.page_index_hint,
            identity: Some(identity),
        }) == crate::book_composition::PageStatus::HiddenByComposition
    }
    pub(crate) fn canonical_raw_book_page(&self, index: usize) -> usize {
        match &self.book_composition {
            ViewerBookComposition::Accepted {
                layout: CompositionProjectionOutcome::Composed(projection),
                ..
            }
            | ViewerBookComposition::Preparing {
                origin:
                    ProjectionOrigin::Refresh {
                        previous: Some(projection),
                        ..
                    },
                ..
            }
            | ViewerBookComposition::Projecting {
                origin:
                    ProjectionOrigin::Refresh {
                        previous: Some(projection),
                        ..
                    },
                ..
            } => projection
                .canonical_raw_index_for_projected(index)
                .unwrap_or(index),
            _ => index,
        }
    }
    pub(crate) fn poll_and_remap_book_target(
        &mut self,
        ctx: &egui::Context,
        index: usize,
    ) -> Option<usize> {
        let generation = self.items_generation;
        let identity = self.items.get(index).and_then(|item| {
            crate::book_resume_db::PageIdentity::for_item(item, self.effective_folder().as_deref())
        });
        self.poll_book_composition_read(ctx);
        if generation == self.items_generation {
            return Some(index);
        }
        let identity = identity?;
        match self.resolve_current_book_position(&crate::book_resume_db::ReadingPosition {
            page: index,
            identity: Some(identity),
        }) {
            super::file_type_resume::PositionResolution::Restored(index) => Some(index),
            super::file_type_resume::PositionResolution::HiddenIdentity => {
                self.show_feedback_toast("このページは本の構成で非表示になっています".into());
                None
            }
            _ => {
                self.show_feedback_toast("このページは見つかりません".into());
                None
            }
        }
    }
    pub(crate) fn composed_book_has_cover(&self) -> bool {
        match &self.book_composition {
            ViewerBookComposition::Preparing {
                origin:
                    ProjectionOrigin::Refresh {
                        previous: Some(projection),
                        ..
                    },
                ..
            }
            | ViewerBookComposition::Projecting {
                origin:
                    ProjectionOrigin::Refresh {
                        previous: Some(projection),
                        ..
                    },
                ..
            } => projection.cover.is_some(),
            ViewerBookComposition::Accepted {
                layout: CompositionProjectionOutcome::Composed(projection),
                ..
            } => projection.cover.is_some(),
            ViewerBookComposition::Accepted {
                record: Some(record),
                ..
            }
            | ViewerBookComposition::Projecting {
                record: Some(record),
                ..
            } => record.composition.cover.is_some(),
            _ => false,
        }
    }

    fn pending_composition_trim_batch(
        &mut self,
    ) -> Option<crate::view_trim_db::ViewTrimWriteBatch> {
        if self.view_trim_db.is_none() {
            return None;
        }
        let pending = self.take_pending_view_trim_transfer()?;
        let batch = pending.batch.clone();
        self.restore_pending_view_trim_transfer(pending);
        Some(batch)
    }
    pub(crate) fn install_book_composition_runtime(
        &mut self,
        service: crate::book_composition::BookCompositionService,
    ) {
        if let Some(handle) = self.remote_session_handle() {
            handle.install_book_composition_service(Some(service.clone()));
        }
        self.book_composition_service = Some(service);
    }

    fn composition_scope_domain(&self) -> Option<(BookScopeKey, PageDomain)> {
        use super::book_materialization::{BookScope, ProjectionRecipe};
        let ProjectionRecipe::Book(book) = &self.projection_recipe else {
            return None;
        };
        let book = match (book, &self.zip_nav) {
            (BookScope::Archive { logical_root, .. }, Some(nav)) => BookScope::Archive {
                logical_root: logical_root.clone(),
                prefix: nav.current_raw().to_vec(),
            },
            _ => book.clone(),
        };
        super::book_materialization::composition_scope_domain(&book)
            .ok()
            .flatten()
    }

    pub(crate) fn composed_book_layout(
        &self,
    ) -> Option<&crate::book_composition::ResolvedComposition> {
        match &self.book_composition {
            ViewerBookComposition::Accepted {
                layout: CompositionProjectionOutcome::Composed(layout),
                ..
            } => Some(&layout.resolved),
            ViewerBookComposition::Preparing {
                origin:
                    ProjectionOrigin::Refresh {
                        previous: Some(layout),
                        ..
                    },
                ..
            }
            | ViewerBookComposition::Projecting {
                origin:
                    ProjectionOrigin::Refresh {
                        previous: Some(layout),
                        ..
                    },
                ..
            } => Some(&layout.resolved),
            _ => None,
        }
    }

    pub(crate) fn composed_layout_token(&self) -> Option<(BookScopeKey, CompositionToken)> {
        match &self.book_composition {
            ViewerBookComposition::Accepted {
                layout: CompositionProjectionOutcome::Composed(layout),
                ..
            } => Some((layout.scope.clone(), layout.token)),
            ViewerBookComposition::Preparing {
                origin:
                    ProjectionOrigin::Refresh {
                        previous: Some(layout),
                        ..
                    },
                ..
            }
            | ViewerBookComposition::Projecting {
                origin:
                    ProjectionOrigin::Refresh {
                        previous: Some(layout),
                        ..
                    },
                ..
            } => Some((layout.scope.clone(), layout.token)),
            _ => None,
        }
    }

    pub(crate) fn pairing_edit_availability(&self) -> crate::spread_shift::PairingEditAvailability {
        use crate::spread_shift::PairingEditAvailability as A;
        if self.extracted_result_surface().is_some() {
            return A::ExtractedResult;
        }
        let Some((scope, _)) = self.composition_scope_domain() else {
            return A::Automatic;
        };
        match &self.book_composition {
            ViewerBookComposition::Accepted {
                scope: accepted,
                layout,
                ..
            } if accepted == &scope => match layout {
                CompositionProjectionOutcome::Composed(_) => A::Composed,
                CompositionProjectionOutcome::Unreadable { .. } => A::Unreadable,
                _ => A::Automatic,
            },
            ViewerBookComposition::Projecting {
                record: Some(_), ..
            } => A::Composed,
            ViewerBookComposition::Blocked { .. }
            | ViewerBookComposition::Preparing { .. }
            | ViewerBookComposition::Projecting { .. } => A::Unreadable,
            _ => match self
                .book_composition_service
                .as_ref()
                .map(|service| service.snapshot())
            {
                Some(snapshot) if snapshot.status.is_err() => A::Unreadable,
                Some(snapshot) if snapshot.rows.contains_key(&scope.storage_key()) => A::Composed,
                _ => A::Automatic,
            },
        }
    }

    pub(crate) fn persist_fullscreen_view_mode(
        &self,
        mode: crate::settings::SpreadMode,
    ) -> Result<(), String> {
        let (scope, _) = self
            .composition_scope_domain()
            .ok_or("本が開かれていません")?;
        self.book_composition_service
            .as_ref()
            .ok_or("本の構成を準備できません")?
            .set_view_mode(scope, mode)
            .map(drop)
    }

    pub(crate) fn persist_fullscreen_reading_flow(
        &self,
        flow: crate::settings::ReadingFlow,
    ) -> Result<(), String> {
        let (scope, _) = self
            .composition_scope_domain()
            .ok_or("本が開かれていません")?;
        self.book_composition_service
            .as_ref()
            .ok_or("本の構成を準備できません")?
            .set_view_flow(scope, self.spread_mode, flow)
            .map(drop)
    }

    pub(crate) fn begin_normal_book_reading(&mut self) {
        if let ViewerBookComposition::Accepted { cause, .. } = &mut self.book_composition {
            *cause = ProjectionAdoptionCause::Normal;
        }
    }

    fn composition_source(&self) -> BookCompositionSource {
        if let Some(nav) = &self.zip_nav {
            return BookCompositionSource::Archive {
                tree: nav.tree.clone(),
                prefix: nav.current_raw().to_vec(),
                display_prefix: nav.current().to_vec(),
            };
        }
        let count = self.pdf_placeholder_count.or_else(|| {
            // Composed rows are no longer in physical page order. A fallback
            // extent must not shrink merely because the last displayed page moved.
            self.items
                .iter()
                .filter_map(|item| match item {
                    crate::grid_item::GridItem::PdfPage { page_num, .. } => page_num.checked_add(1),
                    _ => None,
                })
                .max()
        });
        let physical_source = self
            .items
            .iter()
            .find_map(|item| match item {
                crate::grid_item::GridItem::PdfPage { pdf_path, .. } => Some(pdf_path.clone()),
                _ => None,
            })
            .or_else(|| self.effective_folder());
        count
            .zip(physical_source)
            .map(|(count, physical_source)| BookCompositionSource::Document {
                count,
                metadata: self.image_metas.first().copied().flatten(),
                physical_source,
            })
            .unwrap_or_default()
    }

    fn composition_origin(&self, scope: &BookScopeKey) -> ProjectionOrigin {
        if let ViewerBookComposition::Accepted {
            scope: old,
            layout,
            source,
            generation,
            ..
        } = &self.book_composition
            && old == scope
        {
            let previous = match layout {
                CompositionProjectionOutcome::Composed(layout) => Some(layout.clone()),
                _ => None,
            };
            let anchor = self
                .fullscreen_idx
                .and_then(|index| {
                    previous
                        .as_ref()
                        .and_then(|projection| projection.position_for_projected(index))
                        .and_then(|position| position.identity)
                })
                .or_else(|| {
                    self.fullscreen_idx
                        .and_then(|index| self.items.get(index))
                        .and_then(|item| {
                            crate::book_resume_db::PageIdentity::for_item(
                                item,
                                self.archive_source_override.as_deref(),
                            )
                        })
                });
            let source = if *generation == self.items_generation {
                source.clone()
            } else {
                self.composition_source()
            };
            return ProjectionOrigin::Refresh {
                source,
                previous,
                anchor,
            };
        }
        ProjectionOrigin::Fresh
    }

    pub(super) fn poll_book_composition_service(&mut self, ctx: &egui::Context) {
        let Some(service) = self.book_composition_service.clone() else {
            return;
        };
        for error in service.drain_errors() {
            crate::logger::log(format!("book_composition: {error}"));
            self.show_feedback_toast(format!(
                "本の構成の保存データを処理できませんでした: {error}"
            ));
            ctx.request_repaint();
        }
        let changed = service.drain_changed_scope_keys();
        // Read-only visits never invalidate a sibling. G3 attaches Book reload to this exact set.
        #[cfg(windows)]
        {
            for id in self.book_composition_contexts_to_poll(&changed) {
                let _ = self.with_viewer_context(id, |app| {
                    if app.book_composition_permission.pending()
                        || ((app.book_composition.is_pending()
                            || app
                                .book_composition
                                .scope()
                                .is_some_and(|scope| changed.contains(&scope.storage_key())))
                            && app.extracted_result_surface().is_none())
                    {
                        app.poll_book_composition_read(ctx);
                    }
                });
            }
        }
        self.poll_book_composition_read(ctx);
    }

    pub(super) fn poll_book_composition_read(&mut self, ctx: &egui::Context) {
        self.poll_book_composition_projection(ctx);
        self.poll_book_presentation_permission(ctx);
        self.schedule_composition_wake(ctx);
    }

    #[cfg(test)]
    pub(crate) fn poll_book_composition_service_for_test(&mut self, ctx: &egui::Context) {
        self.poll_book_composition_service(ctx);
    }

    fn poll_book_composition_projection(&mut self, ctx: &egui::Context) {
        if let super::book_materialization::ProjectionRecipe::Book(
            super::book_materialization::BookScope::Archive { logical_root, .. },
        ) = &self.projection_recipe
            && let Some(nav) = &self.zip_nav
            && let Err(reason) = BookScopeKey::archive(logical_root, nav.current_raw())
            && let Ok(scope) = BookScopeKey::archive(logical_root, &[])
        {
            // An unsafe structural prefix has no compositional persistence key.
            // Record the refusal against this viewer's source generation only;
            // do not read a parent record or manufacture a replacement key.
            let token = self
                .book_composition_service
                .as_ref()
                .map(|service| service.snapshot().token_for(&scope))
                .unwrap_or(CompositionToken {
                    service_epoch: 0,
                    publication_sequence: 0,
                });
            if !matches!(&self.book_composition, ViewerBookComposition::Accepted {
                scope: accepted, token: old, generation, layout: CompositionProjectionOutcome::Unsupported { .. }, ..
            } if accepted == &scope && *old == token && *generation == self.items_generation)
            {
                crate::logger::log(format!(
                    "book_composition: unsupported archive level {}: {reason}",
                    logical_root.display()
                ));
                self.show_feedback_toast(
                    crate::book_composition::UNSUPPORTED_COMPOSITION_NOTICE.into(),
                );
                self.book_composition = ViewerBookComposition::Accepted {
                    scope,
                    token,
                    record: None,
                    layout: CompositionProjectionOutcome::Unsupported { reason },
                    source: self.composition_source(),
                    generation: self.items_generation,
                    cause: ProjectionAdoptionCause::Normal,
                };
            }
            self.spread_shift.availability =
                crate::spread_shift::PairingEditAvailability::Automatic;
            return;
        }
        let Some((scope, domain)) = self.composition_scope_domain() else {
            self.book_composition = ViewerBookComposition::Automatic;
            self.spread_shift.availability = self.pairing_edit_availability();
            return;
        };
        let Some(service) = self.book_composition_service.clone() else {
            return;
        };
        let token = service.snapshot().token_for(&scope);
        if matches!(
            self.book_composition,
            ViewerBookComposition::Projecting { .. }
        ) {
            // Hydration completes its independent resume read against the original
            // generation; a qualified open is retained by the permission owner.
            if self.sidecar_restore_active() {
                self.schedule_composition_wake(ctx);
                return;
            }
            let state = std::mem::take(&mut self.book_composition);
            let ViewerBookComposition::Projecting {
                scope: requested,
                token: requested_token,
                record,
                source,
                generation,
                origin,
                receiver,
            } = state
            else {
                unreachable!()
            };
            if requested != scope || generation != self.items_generation {
                // A new accepted source owns its own preparation; never apply old rows to it.
                self.book_composition = ViewerBookComposition::Automatic;
            } else if requested_token != token {
                self.book_composition = ViewerBookComposition::Preparing {
                    scope: scope.clone(),
                    receiver: service.prepare(scope.clone(), domain.clone()),
                    origin,
                };
            } else {
                match receiver.try_recv() {
                    Ok(Ok(reply)) => {
                        self.adopt_book_projection(
                            scope.clone(),
                            token,
                            record,
                            source,
                            origin,
                            reply,
                            ctx,
                        );
                    }
                    Ok(Err(error)) => self.block_book_projection(scope.clone(), token, error),
                    Err(mpsc::TryRecvError::Empty) => {
                        self.book_composition = ViewerBookComposition::Projecting {
                            scope: requested,
                            token: requested_token,
                            record,
                            source,
                            generation,
                            origin,
                            receiver,
                        };
                    }
                    Err(mpsc::TryRecvError::Disconnected) => self.block_book_projection(
                        scope.clone(),
                        token,
                        "本の構成の準備処理が終了しました".into(),
                    ),
                }
                self.schedule_composition_wake(ctx);
                return;
            }
        }
        match &self.book_composition {
            ViewerBookComposition::Accepted {
                scope: accepted,
                token: old,
                generation,
                ..
            } if accepted == &scope && old == &token && *generation == self.items_generation => {
                return;
            }
            ViewerBookComposition::Blocked {
                scope: accepted,
                token: old,
                ..
            } if accepted == &scope && old == &token => return,
            ViewerBookComposition::Preparing {
                scope: requested, ..
            } if requested == &scope => {}
            _ => {
                let origin = self.composition_origin(&scope);
                self.book_composition = ViewerBookComposition::Preparing {
                    scope: scope.clone(),
                    receiver: service.prepare(scope.clone(), domain.clone()),
                    origin,
                };
            }
        }
        let ViewerBookComposition::Preparing { receiver, .. } = &self.book_composition else {
            return;
        };
        match receiver.try_recv() {
            Ok(reply) if reply.scope == scope => {
                let ViewerBookComposition::Preparing { origin, .. } =
                    std::mem::take(&mut self.book_composition)
                else {
                    unreachable!()
                };
                // Publication may have advanced after Prepare replied and its change
                // notification may already have been drained. Compare at adoption,
                // including absent/blocked records and at-rest viewers. Keep the new
                // read in Preparing so the owning context remains eligible for polling.
                if reply.token != service.snapshot().token_for(&scope) {
                    self.book_composition = ViewerBookComposition::Preparing {
                        receiver: service.prepare(scope.clone(), domain),
                        scope,
                        origin,
                    };
                } else {
                    match reply.status {
                        Ok(record) => self.begin_book_projection(
                            scope,
                            reply.token,
                            record.map(Arc::new),
                            origin,
                            ctx,
                        ),
                        Err(error) => {
                            // Store-wide errors are notified once by the service drain above.
                            // A scope/domain rejection is local to this read and still needs notice.
                            if service.snapshot().status.is_ok() {
                                crate::logger::log(format!(
                                    "book_composition: read failed: {error}"
                                ));
                                self.show_feedback_toast(format!(
                                    "本の構成を読み込めませんでした: {error}"
                                ));
                            }
                            self.book_composition = ViewerBookComposition::Blocked {
                                scope,
                                token: reply.token,
                                error,
                            };
                        }
                    }
                    ctx.request_repaint();
                }
            }
            Err(mpsc::TryRecvError::Empty) => {}
            Err(mpsc::TryRecvError::Disconnected) => {
                let error = "構成読込workerが終了しました".to_owned();
                crate::logger::log(format!("book_composition: {error}"));
                self.show_feedback_toast(error.clone());
                self.book_composition = ViewerBookComposition::Blocked {
                    scope,
                    token,
                    error,
                };
                ctx.request_repaint();
            }
            Ok(_) => self.book_composition = ViewerBookComposition::Automatic,
        }
        // Match the existing worker-pending delayed repaint contract, outside the
        // grid tail. A dedicated/parked viewer must wake ROOT, which owns the
        // process service drain, even when its own viewport is idle or not drawn.
        self.spread_shift.availability = self.pairing_edit_availability();
        self.schedule_composition_wake(ctx);
    }

    fn schedule_composition_wake(&self, ctx: &egui::Context) {
        if self.book_composition.is_pending() || self.book_composition_permission.pending() {
            ctx.request_repaint_after_for(
                std::time::Duration::from_millis(50),
                egui::ViewportId::ROOT,
            );
        }
    }

    fn block_book_projection(
        &mut self,
        scope: BookScopeKey,
        token: CompositionToken,
        error: String,
    ) {
        crate::logger::log(format!(
            "book_composition: projection failed {}: {error}",
            scope.storage_key()
        ));
        self.show_feedback_toast(format!("本の構成を準備できませんでした: {error}"));
        self.book_composition = ViewerBookComposition::Blocked {
            scope,
            token,
            error,
        };
        self.spread_shift.availability = crate::spread_shift::PairingEditAvailability::Unreadable;
    }

    fn begin_book_projection(
        &mut self,
        scope: BookScopeKey,
        token: CompositionToken,
        record: Option<Arc<StoredComposition>>,
        origin: ProjectionOrigin,
        ctx: &egui::Context,
    ) {
        let source = match &origin {
            ProjectionOrigin::Refresh { source, .. } => source.clone(),
            ProjectionOrigin::Fresh => self.composition_source(),
        };
        let restores_automatic = record.is_none()
            && matches!(
                &origin,
                ProjectionOrigin::Refresh {
                    previous: Some(_),
                    ..
                }
            );
        if record.is_none()
            && !restores_automatic
            && !matches!(source, BookCompositionSource::Archive { .. })
        {
            self.book_composition = ViewerBookComposition::Accepted {
                scope,
                token,
                record,
                layout: CompositionProjectionOutcome::Automatic,
                source,
                generation: self.items_generation,
                cause: ProjectionAdoptionCause::Normal,
            };
            self.spread_shift.availability = self.pairing_edit_availability();
            return;
        }
        let policy = self.settings.file_type_visibility.clone();
        let placement = self.settings.grid_display_order.clone();
        let available_edits = super::page_edit_snapshot::PageEditAvailability::for_app(self);
        let source_for_worker = source.clone();
        let worker_scope = scope.clone();
        let worker_record = record.clone();
        let worker_origin = origin.clone();
        let data_dir = crate::data_dir::get();
        let pending_trim = self.pending_composition_trim_batch();
        // Submit through the existing FIFO before starting projection work.
        // Superseded workers must not write older edits after newer requests.
        let trim_receipt = pending_trim
            .as_ref()
            .filter(|_| record.is_some() || restores_automatic)
            .and_then(|batch| {
                self.book_composition_service
                    .as_ref()
                    .map(|service| service.save_view_trim(batch.clone()))
            });
        #[cfg(test)]
        let epoch_scope = crate::page_edit_write_epoch::TestEpochScope::capture();
        let (sender, receiver) = mpsc::channel();
        let spawn = std::thread::Builder::new()
            .name("book-composition-projection".into())
            .spawn(move || {
                #[cfg(test)]
                let _epoch_scope =
                    epoch_scope.map(crate::page_edit_write_epoch::TestEpochScope::enter);
                let result = prepare_book_projection(
                    &worker_scope,
                    token,
                    worker_record.as_deref(),
                    &source_for_worker,
                    &worker_origin,
                    &placement,
                    policy,
                    available_edits,
                    &data_dir,
                    pending_trim,
                    trim_receipt,
                );
                // The existing ROOT pending-wake contract collects this completion without input.
                let _ = sender.send(result);
            });
        if let Err(error) = spawn {
            self.block_book_projection(scope, token, error.to_string());
            return;
        }
        self.book_composition = ViewerBookComposition::Projecting {
            scope,
            token,
            record,
            source,
            generation: self.items_generation,
            origin,
            receiver,
        };
        self.spread_shift.availability = crate::spread_shift::PairingEditAvailability::Unreadable;
        self.schedule_composition_wake(ctx);
    }

    fn adopt_book_projection(
        &mut self,
        scope: BookScopeKey,
        token: CompositionToken,
        record: Option<Arc<StoredComposition>>,
        source: BookCompositionSource,
        origin: ProjectionOrigin,
        reply: BookProjectionReply,
        ctx: &egui::Context,
    ) {
        // A worker saves pending trim by its stable page keys before filtering rows.
        // If another ordinary edit arrived meanwhile, prepare that newer batch
        // before adopting. A failed write leaves the original rows and dirty edits.
        let replaces_rows = matches!(
            &reply.rows.composition,
            CompositionProjectionOutcome::Composed(_)
        ) || (matches!(
            &reply.rows.composition,
            CompositionProjectionOutcome::Automatic
        ) && matches!(
            &origin,
            ProjectionOrigin::Refresh {
                previous: Some(_),
                ..
            }
        ));
        if replaces_rows
            && (reply.saved_trim != self.pending_composition_trim_batch()
                || reply.edits.as_ref().is_some_and(|prepared| {
                    !crate::page_edit_write_epoch::PAGE_EDIT_WRITES.accepts(prepared.stamp)
                }))
        {
            self.begin_book_projection(scope, token, record, origin, ctx);
            return;
        }
        let super::book_materialization::MaterializedBookRows {
            items,
            metas,
            composition,
            page_identities,
        } = reply.rows;
        match &composition {
            CompositionProjectionOutcome::Unsupported { reason } => {
                crate::logger::log(format!(
                    "book_composition: unsupported source {}: {reason}",
                    scope.storage_key()
                ));
                self.show_feedback_toast(
                    crate::book_composition::UNSUPPORTED_COMPOSITION_NOTICE.into(),
                );
                // The existing source rows, selection and cache remain untouched.
                self.book_composition = ViewerBookComposition::Accepted {
                    scope,
                    token,
                    record,
                    layout: composition,
                    source,
                    generation: self.items_generation,
                    cause: ProjectionAdoptionCause::Normal,
                };
                self.spread_shift.availability = self.pairing_edit_availability();
                self.start_composition_removed_shift_restore(self.frame_counter.wrapping_add(1));
                return;
            }
            CompositionProjectionOutcome::Unreadable { reason } => {
                self.block_book_projection(scope, token, reason.clone());
                return;
            }
            _ => {}
        }
        let recipe = self.projection_recipe.clone();
        let was_fullscreen = self.fullscreen_idx.is_some();
        let landing = self
            .fullscreen_idx
            .and_then(|index| reply.refresh_landings.get(&index).copied())
            .unwrap_or(reply.landing);
        let refresh = matches!(origin, ProjectionOrigin::Refresh { .. });
        let removing = matches!(
            &origin,
            ProjectionOrigin::Refresh {
                previous: Some(_),
                ..
            }
        ) && matches!(composition, CompositionProjectionOutcome::Automatic);
        if !removing && matches!(composition, CompositionProjectionOutcome::Automatic) {
            self.book_composition = ViewerBookComposition::Accepted {
                scope,
                token,
                record,
                layout: composition,
                source,
                generation: self.items_generation,
                cause: ProjectionAdoptionCause::Normal,
            };
            self.spread_shift.availability = self.pairing_edit_availability();
            self.start_composition_removed_shift_restore(self.frame_counter.wrapping_add(1));
            return;
        }
        if reply.saved_trim.is_some() {
            self.record_current_container_content_identity(
                crate::content_identity::ContentIdentityTrigger::Edit,
            );
            for index in &self.view_trim_dirty_page_overrides {
                self.record_content_identity_for_idx(
                    *index,
                    crate::content_identity::ContentIdentityTrigger::Edit,
                );
            }
            self.invalidate_smart_folder_resort_metadata();
        }
        let old_items = &self.items;
        let new_index = |index: usize| {
            let identity = crate::book_resume_db::PageIdentity::for_item(
                old_items.get(index)?,
                Some(&scope.logical_root),
            )?;
            match page_identities.resolve(&crate::book_resume_db::ReadingPosition {
                page: 0,
                identity: Some(identity),
            }) {
                super::file_type_resume::PositionResolution::Restored(index) => Some(index),
                _ => None,
            }
        };
        // A same-book projection is not a leave/save operation. Preserve pending
        // edits by identity and keep the install helper's flush path I/O-free.
        let pending_trim = std::mem::replace(&mut self.view_trim_save_pending, false)
            && reply.saved_trim.is_none();
        let dirty_trim = std::mem::take(&mut self.view_trim_dirty_page_overrides)
            .into_iter()
            .filter(|_| reply.saved_trim.is_none())
            .filter_map(|old| Some((new_index(old)?, *self.view_trim_page_overrides.get(&old)?)))
            .collect::<Vec<_>>();
        let layers = std::mem::take(&mut self.local_adjust_page_layers)
            .into_iter()
            .filter_map(|(old, value)| Some((new_index(old)?, value)))
            .collect::<std::collections::HashMap<_, _>>();
        let selected_layers = std::mem::take(&mut self.local_adjust_selected_layers)
            .into_iter()
            .filter_map(|(old, value)| Some((new_index(old)?, value)))
            .collect();
        let selected = self.selected.and_then(new_index);
        let previous_generation = self.items_generation;
        self.install_prepared_aggregate_items(recipe, items, metas, page_identities);
        self.clear_page_edit_state();
        if let Some(edits) = reply.edits {
            self.install_prepared_page_edits(edits);
        } else {
            self.prepare_page_edits_for_current_virtual_items();
        }
        self.local_adjust_page_layers = layers;
        self.local_adjust_selected_layers = selected_layers;
        self.selected = selected;
        for (index, layers) in &self.local_adjust_page_layers {
            if layers.is_empty() {
                self.local_adjust_pages.remove(index);
            } else {
                self.local_adjust_pages.insert(*index);
            }
        }
        for (index, value) in dirty_trim {
            self.view_trim_page_overrides.insert(index, value);
            self.view_trim_dirty_page_overrides.insert(index);
        }
        self.view_trim_save_pending = pending_trim;
        self.spread_display_units_cache
            .clear_continuous_measurements();
        self.fs_continuous_fit_reference =
            crate::ui_fullscreen::ContinuousSpecifiedFitReference::Legacy;
        if let CompositionProjectionOutcome::Composed(projection) = &composition {
            self.remap_after_book_projection(
                projection,
                previous_generation,
                self.items_generation,
                &reply.refresh_landings,
            );
            self.reading_direction = match projection.resolved.direction {
                crate::book_composition::ReadingDirection::Ltr => {
                    crate::settings::ReadingDirection::Ltr
                }
                crate::book_composition::ReadingDirection::Rtl => {
                    crate::settings::ReadingDirection::Rtl
                }
            };
            if self.spread_mode.is_spread() {
                let has_cover = record
                    .as_ref()
                    .is_some_and(|record| record.composition.cover.is_some());
                self.spread_mode = match (self.reading_direction, has_cover) {
                    (crate::settings::ReadingDirection::Ltr, false) => {
                        crate::settings::SpreadMode::Ltr
                    }
                    (crate::settings::ReadingDirection::Ltr, true) => {
                        crate::settings::SpreadMode::LtrCover
                    }
                    (crate::settings::ReadingDirection::Rtl, false) => {
                        crate::settings::SpreadMode::Rtl
                    }
                    (crate::settings::ReadingDirection::Rtl, true) => {
                        crate::settings::SpreadMode::RtlCover
                    }
                };
            } else {
                self.spread_mode = self
                    .spread_mode
                    .with_reading_direction(self.reading_direction);
            }
        } else {
            self.retire_book_permission_after_projection(
                &scope,
                previous_generation,
                self.items_generation,
            );
        }
        self.book_composition = ViewerBookComposition::Accepted {
            scope,
            token,
            record,
            layout: composition,
            source,
            generation: self.items_generation,
            cause: if refresh {
                ProjectionAdoptionCause::ProjectionRefresh
            } else {
                ProjectionAdoptionCause::Normal
            },
        };
        self.spread_shift.availability = self.pairing_edit_availability();
        if removing {
            let defaults = super::SpreadRestoreDefaults::for_book(&self.settings);
            let stored = reply.automatic_preferences.unwrap_or_default();
            let direction = stored.direction.unwrap_or(defaults.reading_direction);
            let mode = stored
                .mode
                .unwrap_or(defaults.spread_mode.with_reading_direction(direction));
            self.reading_direction = mode.reading_direction().unwrap_or(direction);
            self.reading_flow = stored.flow.unwrap_or(defaults.reading_flow);
            self.spread_mode = if mode == crate::settings::SpreadMode::Vertical {
                self.reading_flow = crate::settings::ReadingFlow::Vertical;
                crate::settings::SpreadMode::Single
            } else {
                mode
            };
            self.start_composition_removed_shift_restore(self.frame_counter.wrapping_add(1));
        }
        self.rebuild_visible_indices();
        if was_fullscreen {
            if let Some(index) = landing {
                self.fullscreen_idx = Some(index);
                self.selected = Some(index);
            } else if !self.has_book_dfs_continuation() {
                self.close_fullscreen();
            }
        }
        ctx.request_repaint();
    }

    #[cfg(test)]
    pub(crate) fn install_composition_for_test(
        &mut self,
        projection: Arc<AcceptedCompositionProjection>,
    ) {
        self.book_composition = ViewerBookComposition::Accepted {
            scope: projection.scope.clone(),
            token: projection.token,
            record: None,
            layout: CompositionProjectionOutcome::Composed(projection),
            source: BookCompositionSource::Unavailable,
            generation: self.items_generation,
            cause: ProjectionAdoptionCause::Normal,
        };
        self.spread_shift.availability = crate::spread_shift::PairingEditAvailability::Composed;
    }

    pub(crate) fn resolve_current_book_position(
        &self,
        position: &crate::book_resume_db::ReadingPosition,
    ) -> super::file_type_resume::PositionResolution {
        use super::file_type_resume::PositionResolution as R;
        match &self.book_composition {
            ViewerBookComposition::Accepted {
                layout: CompositionProjectionOutcome::Composed(projection),
                ..
            } => match projection.resolve_position(position) {
                crate::book_composition::PageStatus::Visible { item_index } => {
                    R::Restored(item_index)
                }
                crate::book_composition::PageStatus::HiddenByComposition
                | crate::book_composition::PageStatus::ExcludedByPolicy
                | crate::book_composition::PageStatus::FilteredOut => R::HiddenIdentity,
                _ => R::InvalidRaw,
            },
            _ => self.page_identity_index.resolve(position),
        }
    }
}
impl ViewerBookComposition {
    pub(crate) fn is_pending(&self) -> bool {
        matches!(self, Self::Preparing { .. } | Self::Projecting { .. })
    }
    pub(crate) fn scope(&self) -> Option<&BookScopeKey> {
        match self {
            Self::Automatic => None,
            Self::Preparing { scope, .. }
            | Self::Projecting { scope, .. }
            | Self::Accepted { scope, .. }
            | Self::Blocked { scope, .. } => Some(scope),
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn prepare_book_projection(
    scope: &BookScopeKey,
    token: CompositionToken,
    record: Option<&StoredComposition>,
    source: &BookCompositionSource,
    origin: &ProjectionOrigin,
    placement: &crate::settings::GridDisplayOrder,
    policy: crate::file_type_visibility::FileTypeVisibility,
    available_edits: super::page_edit_snapshot::PageEditAvailability,
    data_dir: &std::path::Path,
    pending_trim: Option<crate::view_trim_db::ViewTrimWriteBatch>,
    trim_receipt: Option<mpsc::Receiver<Result<(), String>>>,
) -> Result<BookProjectionReply, String> {
    use super::book_materialization::*;
    let composition = record.map(|record| &record.composition);
    let rows = match source {
        BookCompositionSource::Archive {
            tree,
            prefix,
            display_prefix,
        } => {
            let displayed = tree.raw_source().project(&policy);
            let row_prefix = if composition.is_some() {
                prefix
            } else {
                display_prefix
            };
            let rows = materialize_archive_level(
                &displayed,
                row_prefix,
                super::BOOK_READING_PAGE_ORDER,
                Some(placement),
            );
            project_archive_composition(
                tree,
                &scope.logical_root,
                prefix,
                rows,
                super::BOOK_READING_PAGE_ORDER,
                Some(placement),
                policy,
                token,
                composition,
            )
        }
        BookCompositionSource::Document {
            count,
            metadata,
            physical_source,
        } => {
            let (mtime, size) = metadata.unwrap_or_default();
            let (items, metas, _) = materialize_cached_document_rows(
                physical_source,
                *count,
                mtime,
                size.max(0) as u64,
            );
            let rows = if policy.allows_physical_file(&scope.logical_root) {
                (items, metas)
            } else {
                (vec![], vec![])
            };
            project_document_composition(
                &scope.logical_root,
                *count,
                rows,
                policy,
                token,
                composition,
            )
        }
        BookCompositionSource::Unavailable => return Err("本のページ一覧を準備できません".into()),
    }
    .map_err(|error| error.to_string())?;
    let find = |identity: &crate::book_resume_db::PageIdentity| match &rows.composition {
        CompositionProjectionOutcome::Composed(projection) => {
            match projection.resolve_position(&crate::book_resume_db::ReadingPosition {
                page: 0,
                identity: Some(identity.clone()),
            }) {
                crate::book_composition::PageStatus::Visible { item_index } => Some(item_index),
                _ => None,
            }
        }
        _ => match rows
            .page_identities
            .resolve(&crate::book_resume_db::ReadingPosition {
                page: 0,
                identity: Some(identity.clone()),
            }) {
            super::file_type_resume::PositionResolution::Restored(index) => Some(index),
            _ => None,
        },
    };
    let first_page = || match &rows.composition {
        CompositionProjectionOutcome::Composed(projection) => projection
            .resolved
            .visible_pages
            .first()
            .map(|page| page.item_index),
        _ => rows.items.iter().position(|item| {
            matches!(
                item,
                crate::grid_item::GridItem::ZipImage { .. }
                    | crate::grid_item::GridItem::PdfPage { .. }
            )
        }),
    };
    let landing = match origin {
        ProjectionOrigin::Fresh => first_page(),
        ProjectionOrigin::Refresh {
            anchor, previous, ..
        } => anchor
            .as_ref()
            .and_then(find)
            .or_else(|| {
                let previous = previous.as_ref()?;
                let anchor = anchor.as_ref()?;
                let page = crate::book_composition::composition_page_from_identity(scope, anchor)?;
                let position = previous
                    .resolved
                    .visible_pages
                    .iter()
                    .position(|old| old.page.matching_key() == page.matching_key())?;
                previous
                    .resolved
                    .visible_pages
                    .iter()
                    .skip(position + 1)
                    .chain(previous.resolved.visible_pages[..position].iter().rev())
                    .find_map(|old| find(&old.page.resume_identity(&scope.logical_root)))
            })
            .or_else(first_page),
    };
    // Prepare all old identity landings off-thread. Adoption selects the latest
    // context anchor in O(1), so a legitimate navigation during preparation is
    // not undone by the earlier captured anchor.
    let mut refresh_landings = std::collections::HashMap::new();
    if let ProjectionOrigin::Refresh {
        previous: Some(previous),
        ..
    } = origin
    {
        let old = &previous.resolved.visible_pages;
        let resolved = old
            .iter()
            .map(|page| find(&page.page.resume_identity(&scope.logical_root)))
            .collect::<Vec<_>>();
        let mut next = vec![None; old.len()];
        let mut following = None;
        for index in (0..old.len()).rev() {
            next[index] = following;
            following = resolved[index].or(following);
        }
        let mut preceding = None;
        for (index, page) in old.iter().enumerate() {
            refresh_landings.insert(
                page.item_index,
                resolved[index]
                    .or(next[index])
                    .or(preceding)
                    .or_else(first_page),
            );
            preceding = resolved[index].or(preceding);
        }
    }
    let is_replacement = matches!(&rows.composition, CompositionProjectionOutcome::Composed(_))
        || matches!(
            origin,
            ProjectionOrigin::Refresh {
                previous: Some(_),
                ..
            }
        );
    let saved_trim = if is_replacement { pending_trim } else { None };
    if saved_trim.is_some() {
        trim_receipt
            .ok_or("余白の保存を開始できませんでした")?
            .recv()
            .map_err(|error| error.to_string())??;
    }
    let edits = if is_replacement {
        super::page_edit_snapshot::PageEditSnapshot::load_and_project_stable(
            &rows.items,
            available_edits,
            &std::sync::atomic::AtomicBool::new(false),
        )?
    } else {
        None
    };
    let automatic_preferences = if composition.is_none() && is_replacement {
        let segments = match source {
            BookCompositionSource::Archive { display_prefix, .. } => display_prefix.as_slice(),
            _ => &[],
        };
        let key = crate::spread_db::container_key_with_fallback(&scope.logical_root, segments);
        crate::spread_db::SpreadDb::open_existing_read_only_at(&data_dir.join("spread.db"))
            .map_err(|error| error.to_string())?
            .map(|db| db.get_state_with_fallback(&key.exact, key.fallback.as_deref()))
    } else {
        None
    };
    Ok(BookProjectionReply {
        rows,
        landing,
        edits,
        automatic_preferences,
        refresh_landings,
        saved_trim,
    })
}
