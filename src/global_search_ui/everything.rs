//! Everything is a producer of the shared Search surface, not a second list owner.
use super::*;
mod metadata;
mod shortcut;
pub(super) use metadata::prepare as prepare_everything_metadata;
pub(super) use metadata::{SearchMetadataInstall, SearchMetadataResources};
mod ui;
pub use ui::{draw_everything_progress_snapshot_fixture, draw_everything_search_snapshot_fixture};
#[cfg(test)]
mod tests;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum SearchSource {
    Metadata,
    Everything,
}

#[derive(Clone, Copy)]
pub(crate) enum SearchOpenOrigin {
    Metadata,
    Everything {
        run: u64,
        context: crate::app::ViewerContextId,
    },
}

pub(crate) enum SearchProducer {
    Metadata(SearchHandle),
    Everything {
        intent: EverythingIntent,
        request_run: u64,
        local_lease: Option<crate::remote_ipc::session::LocalModalLease>,
        cancel: crate::everything_ipc::CancelHandle,
        submitted: SubmittedEverything,
        context: crate::app::ViewerContextId,
        generation: u64,
        // A rarely active request must not inline a Settings value into every
        // App and ViewerContextBundle. This remains the same immutable owner.
        policy: Box<crate::settings::Settings>,
    },
}

#[derive(Clone, Copy)]
pub(crate) enum EverythingIntent {
    Query,
    Return,
}

impl From<SearchHandle> for SearchProducer {
    fn from(handle: SearchHandle) -> Self {
        Self::Metadata(handle)
    }
}

impl Drop for SearchProducer {
    fn drop(&mut self) {
        if let Self::Everything { cancel, .. } = self {
            cancel.cancel();
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct SubmittedEverything {
    query: String,
    run: u64,
}

#[derive(Clone)]
pub(crate) enum EverythingPosition {
    Root,
    PhysicalChild {
        logical_source: PathBuf,
        return_anchor: PathBuf,
        route: crate::app::FacetRoute,
    },
}

pub(crate) enum SearchAccepted {
    Metadata,
    Everything {
        submitted: SubmittedEverything,
        dataset: Arc<crate::everything_results::EverythingDataset>,
        position: EverythingPosition,
    },
}

#[derive(Clone)]
pub(crate) enum SearchProjection {
    Metadata,
    Everything {
        submitted: SubmittedEverything,
        dataset: Arc<crate::everything_results::EverythingDataset>,
    },
}

pub(super) fn project_dataset(
    dataset: &crate::everything_results::EverythingDataset,
    sort: crate::settings::SortOrder,
    ratings: &HashMap<String, u8>,
    filter: &[bool; 6],
    recipe: &crate::rating_membership::RatingPreviewRecipe,
) -> (Vec<GridItem>, Vec<Option<(i64, i64)>>) {
    let mut rows = dataset
        .rows
        .iter()
        .filter(|row| {
            search_item_rating_key(&row.item).is_none_or(|key| {
                recipe.allows(&key, ratings.get(&key).copied().unwrap_or(0), filter)
            })
        })
        .map(|row| (row, sort.name_key(row.item.name().as_ref())))
        .collect::<Vec<_>>();
    rows.sort_by(|(a, ak), (b, bk)| sort.compare_listing_keys(ak, a.sort_meta, bk, b.sort_meta));
    rows.into_iter()
        .map(|(row, _)| (row.item.clone(), row.image_meta))
        .unzip()
}

impl App {
    pub(super) fn search_projection_reuses_accepted_dataset(
        &self,
        source: &SearchProjection,
    ) -> bool {
        match source {
            SearchProjection::Metadata => true,
            SearchProjection::Everything { dataset, .. } => matches!(
                &self.global_search.accepted,
                SearchAccepted::Everything { dataset: accepted, position: EverythingPosition::Root, .. }
                    if Arc::ptr_eq(dataset, accepted)
            ),
        }
    }
    pub(crate) fn retire_search_prepare(&mut self) {
        if let Some(prepare) = self.global_search.page_edit_prepare.take() {
            prepare.cancel.store(true, Ordering::Relaxed);
            self.retire_smart_folder_payloads([
                Box::new(prepare) as crate::app::smart_folder::RetiredSmartFolderPayload
            ]);
        }
    }

    pub(crate) fn retire_search_accepted(&mut self) {
        let accepted =
            std::mem::replace(&mut self.global_search.accepted, SearchAccepted::Metadata);
        self.retire_smart_folder_payloads([
            Box::new(accepted) as crate::app::smart_folder::RetiredSmartFolderPayload
        ]);
    }
    pub(crate) fn capture_search_open_origin(&self) -> Option<SearchOpenOrigin> {
        self.global_search
            .active
            .then(|| match &self.global_search.accepted {
                SearchAccepted::Metadata => SearchOpenOrigin::Metadata,
                SearchAccepted::Everything { .. } => SearchOpenOrigin::Everything {
                    run: self.global_search.run_sequence,
                    context: self.virtual_list_context_id(),
                },
            })
    }

    pub(crate) fn search_open_origin_is_current(&self, origin: SearchOpenOrigin) -> bool {
        self.global_search.active
            && match origin {
                SearchOpenOrigin::Metadata => {
                    matches!(self.global_search.accepted, SearchAccepted::Metadata)
                }
                SearchOpenOrigin::Everything { run, context } => {
                    run == self.global_search.run_sequence
                        && context == self.virtual_list_context_id()
                        && matches!(
                            self.global_search.accepted,
                            SearchAccepted::Everything { .. }
                        )
                }
            }
    }
    pub(crate) fn everything_search_entry_allowed(&self) -> bool {
        self.everything_main_grid_available()
            && !self.common_modal_dialog_open()
            && !self.remote_session_blocks_local_control()
    }

    pub(crate) fn open_everything_search(&mut self) {
        if !self.everything_search_entry_allowed() {
            return;
        }
        if !self.global_search.active {
            self.open_global_search();
        }
        self.switch_search_source(SearchSource::Everything);
        self.global_search.focus_request = true;
    }

    pub(super) fn switch_search_source(&mut self, source: SearchSource) {
        if self.global_search.source == source {
            return;
        }
        self.finish_everything_edit_boundary();
        self.global_search.pending = None;
        self.retire_search_prepare();
        self.global_search.mtime_lookup_pending = None;
        self.global_search.run_sequence = self.global_search.run_sequence.wrapping_add(1);
        self.global_search.source = source;
        self.global_search.query.clear();
        self.global_search.last_executed.clear();
        self.global_search.last_change_at = None;
        self.global_search.reject_message = None;
        self.global_search.focus_request = true;
        // Accepted data and its source still explain the visible rows after a failed send.
    }

    pub(crate) fn everything_search_progress_visible(&self) -> bool {
        self.global_search.active
            && matches!(&self.global_search.pending,
            Some(SearchProducer::Everything { context, .. }) if *context == self.virtual_list_context_id())
    }

    pub(super) fn everything_running_is_current(&self) -> bool {
        matches!(&self.global_search.pending, Some(SearchProducer::Everything {
            request_run, context, generation, policy, cancel, intent, ..
        }) if !cancel.is_cancelled() && self.global_search.active
            && (matches!(intent,EverythingIntent::Return) || self.global_search.source == SearchSource::Everything)
            && *request_run == self.global_search.run_sequence
            && *context == self.virtual_list_context_id() && *generation == self.items_generation
            && policy.file_type_visibility == self.settings.file_type_visibility
            && policy.archive_file_handling == self.settings.archive_file_handling
            && policy.epub_file_handling == self.settings.epub_file_handling
            && policy.susie_enabled == self.settings.susie_enabled
            && policy.video_thumb_use_sidecar_image == self.settings.video_thumb_use_sidecar_image)
    }

    pub(crate) fn cancel_everything_search(&mut self) {
        if !matches!(
            self.global_search.pending,
            Some(SearchProducer::Everything { .. })
        ) {
            return;
        }
        self.global_search.pending = None;
        self.retire_search_prepare();
        self.global_search.run_sequence = self.global_search.run_sequence.wrapping_add(1);
    }

    pub(crate) fn send_everything_search(&mut self, ctx: &egui::Context) {
        if !self.everything_search_entry_allowed()
            || self.global_search.source != SearchSource::Everything
        {
            return;
        }
        if let Err(error) = crate::everything_ipc::validate_query(&self.global_search.query) {
            self.global_search.reject_message = Some(error.user_message().into());
            return;
        }
        let local_lease = match self.remote_session_handle() {
            Some(handle) => match handle.try_local_modal_lease() {
                Some(lease) => Some(lease),
                None => {
                    self.show_feedback_toast("Remote が操作中のため検索できません".into());
                    return;
                }
            },
            None => None,
        };
        self.finish_everything_edit_boundary();
        self.retire_main_list_requests_for_surface_switch();
        self.global_search.pending = None;
        self.retire_search_prepare();
        self.global_search.run_sequence = self.global_search.run_sequence.wrapping_add(1);
        self.global_search.visibility = self.settings.file_type_visibility.clone();
        self.global_search.reject_message = None;
        if !self.restart_search_page_edit_prepare(ctx) {
            return;
        }
        let cancel = match crate::everything_ipc::CancelHandle::new() {
            Ok(cancel) => cancel,
            Err(error) => {
                self.global_search.reject_message = Some(error.user_message().into());
                self.retire_search_prepare();
                return;
            }
        };
        let submitted = SubmittedEverything {
            query: self.global_search.query.clone(),
            run: self.global_search.run_sequence,
        };
        let command = SearchPrepareCommand::Everything(submitted.query.clone(), cancel.clone());
        self.global_search.pending = Some(SearchProducer::Everything {
            intent: EverythingIntent::Query,
            request_run: self.global_search.run_sequence,
            local_lease,
            cancel,
            submitted,
            context: self.virtual_list_context_id(),
            generation: self.items_generation,
            policy: Box::new(self.settings.thumbnail_source_discovery_snapshot()),
        });
        if self
            .global_search
            .page_edit_prepare
            .as_ref()
            .unwrap()
            .tx
            .send(command)
            .is_err()
        {
            self.cancel_everything_search();
            self.global_search.reject_message =
                Some("検索結果を準備できませんでした。もう一度検索してください".into());
        }
    }

    pub(super) fn everything_dataset_for_reprojection(
        &self,
    ) -> Option<Arc<crate::everything_results::EverythingDataset>> {
        match &self.global_search.accepted {
            SearchAccepted::Everything {
                dataset,
                position: EverythingPosition::Root,
                ..
            } => Some(Arc::clone(dataset)),
            _ => None,
        }
    }

    pub(super) fn queue_everything_view(
        &mut self,
        prepare: &mut SearchPageEditPrepare,
        dataset: Arc<crate::everything_results::EverythingDataset>,
        survivors: Option<Vec<String>>,
    ) {
        let submitted = match &self.global_search.pending {
            Some(SearchProducer::Everything { submitted, .. }) => submitted.clone(),
            _ => match &self.global_search.accepted {
                SearchAccepted::Everything { submitted, .. } => submitted.clone(),
                _ => return,
            },
        };
        let view = GlobalSearchView::Flat;
        let adoption = self.capture_search_view_adoption(&view);
        prepare.next_sequence = prepare.next_sequence.wrapping_add(1);
        prepare.desired = Some(SearchViewWish {
            source: SearchProjection::Everything { submitted, dataset },
            adoption,
            sequence: prepare.next_sequence,
            run_sequence: self.global_search.run_sequence,
            query: String::new(),
            filters: self.global_search.filters.clone(),
            visibility: self.global_search.visibility.clone(),
            source_generation: self.items_generation,
            context: self.virtual_list_context_id(),
            through_batch: 0,
            view,
            sort_mode: self.global_search.sort_mode,
            order_request: crate::rating_sort::ListingOrderRequest::from_settings(&self.settings),
            rating_write_generation: self.rating_session_write_generation,
            rating_preview: self.rating_preview_recipe(),
            rating_adoption: crate::rating_membership::RatingAdoptionIntent::Reconcile,
            rating_filter: self.effective_rating_filter(),
            done: self.global_search.done,
            survivor_order: survivors,
        });
        let _ = prepare.tx.send(SearchPrepareCommand::RatingWrites {
            updates: self.rating_session_writes_after(0),
            through_generation: self.rating_session_write_generation,
        });
        prepare.send_next();
    }

    pub(super) fn request_everything_view_build_with_survivors(
        &mut self,
        dataset: Arc<crate::everything_results::EverythingDataset>,
        survivors: Option<Vec<String>>,
    ) {
        // This worker also owns the retained dataset's re-sorts and the return from a book.
        if self.global_search.page_edit_prepare.is_none() {
            let ctx = self.edit_preview_repaint_ctx.clone().unwrap_or_default();
            if !self.restart_search_page_edit_prepare(&ctx) {
                self.cancel_everything_search();
                return;
            }
        }
        let mut prepare = self.global_search.page_edit_prepare.take().unwrap();
        self.queue_everything_view(&mut prepare, dataset, survivors);
        self.global_search.page_edit_prepare = Some(prepare);
    }

    pub(super) fn search_projection_is_current(&self, wish: &SearchViewWish) -> bool {
        match &wish.source {
            SearchProjection::Metadata => {
                self.global_search.source == SearchSource::Metadata
                    && wish.query == self.global_search.query
            }
            SearchProjection::Everything { submitted, .. } => match &self.global_search.pending {
                Some(SearchProducer::Everything {
                    submitted: current, ..
                }) => self.everything_running_is_current() && current == submitted,
                _ => {
                    matches!(&self.global_search.accepted,SearchAccepted::Everything {submitted:current,..} if current == submitted)
                }
            },
        }
    }

    pub(super) fn accept_search_projection(&mut self, source: SearchProjection) {
        self.retire_search_accepted();
        match source {
            SearchProjection::Metadata => self.global_search.accepted = SearchAccepted::Metadata,
            SearchProjection::Everything { submitted, dataset } => {
                let old_hits = std::mem::take(&mut self.global_search.all_hits);
                let old_containers = std::mem::take(&mut self.global_search.containers);
                self.retire_smart_folder_payloads([Box::new((old_hits, old_containers))
                    as crate::app::smart_folder::RetiredSmartFolderPayload]);
                self.global_search.total_valid = dataset.rows.len();
                self.global_search.total_scanned = dataset.stats.raw_returned;
                self.global_search.truncated = dataset.stats.limit_reached;
                self.global_search.last_executed = submitted.query.clone();
                self.global_search.accepted = SearchAccepted::Everything {
                    submitted,
                    dataset,
                    position: EverythingPosition::Root,
                };
                self.global_search.pending = None;
                self.global_search.done = true;
                self.global_search.drill = None;
                self.global_search.aggregate = false;
                self.global_search.aggregate_auto = false;
            }
        }
    }

    pub(crate) fn everything_has_physical_child(&self) -> bool {
        matches!(
            self.global_search.accepted,
            SearchAccepted::Everything {
                position: EverythingPosition::PhysicalChild { .. },
                ..
            }
        )
    }

    pub(super) fn accept_everything_physical_child(&mut self, path: &Path) -> bool {
        let SearchAccepted::Everything {
            dataset, position, ..
        } = &mut self.global_search.accepted
        else {
            return false;
        };
        if matches!(position, EverythingPosition::Root) {
            if !dataset.rows.iter().any(|row| {
                row.item
                    .file_operation_path()
                    .is_some_and(|p| crate::folder_tree::path_eq(p, path))
            }) {
                return true;
            }
            let route = self
                .top_level_grid_view
                .return_origin()
                .map(|origin| origin.route.clone())
                .unwrap_or_default();
            *position = EverythingPosition::PhysicalChild {
                logical_source: path.into(),
                return_anchor: path.into(),
                route,
            };
        }
        self.update_global_search_address();
        true
    }

    pub(super) fn return_from_everything_child(&mut self) -> bool {
        let SearchAccepted::Everything {
            dataset, position, ..
        } = &self.global_search.accepted
        else {
            return false;
        };
        let EverythingPosition::PhysicalChild { return_anchor, .. } = position else {
            return true;
        };
        if self.zip_nav.as_ref().is_some_and(|nav| !nav.at_root()) {
            self.zip_nav_back();
            return true;
        }
        let anchor = return_anchor.clone();
        let dataset = Arc::clone(dataset);
        let submitted = match &self.global_search.accepted {
            SearchAccepted::Everything { submitted, .. } => submitted.clone(),
            _ => unreachable!(),
        };
        let local_lease = match self.remote_session_handle() {
            Some(handle) => match handle.try_local_modal_lease() {
                Some(lease) => Some(lease),
                None => return true,
            },
            None => None,
        };
        self.finish_everything_edit_boundary();
        self.retire_main_list_requests_for_surface_switch();
        self.restore_rating_filter_suppression();
        self.global_search.restore_select_path = Some(anchor);
        let cancel = match crate::everything_ipc::CancelHandle::new() {
            Ok(cancel) => cancel,
            Err(error) => {
                self.show_feedback_toast(error.user_message().into());
                return true;
            }
        };
        self.global_search.pending = Some(SearchProducer::Everything {
            intent: EverythingIntent::Return,
            request_run: self.global_search.run_sequence,
            local_lease,
            cancel,
            submitted,
            context: self.virtual_list_context_id(),
            generation: self.items_generation,
            policy: Box::new(self.settings.thumbnail_source_discovery_snapshot()),
        });
        self.request_everything_view_build_with_survivors(dataset, None);
        true
    }

    pub(super) fn update_everything_address(&mut self) -> bool {
        if !self.global_search.active {
            return false;
        }
        let SearchAccepted::Everything {
            submitted,
            dataset,
            position,
        } = &self.global_search.accepted
        else {
            return false;
        };
        let suffix = match position {
            EverythingPosition::Root => String::new(),
            EverythingPosition::PhysicalChild { logical_source, .. } => {
                format!(" > {}", logical_source.display())
            }
        };
        self.address = format!(
            "Everything: \"{}\"{} / {} 件中 {} 件取得 / 表示対象 {} 件",
            submitted.query,
            suffix,
            dataset.stats.total_items,
            dataset.stats.raw_returned,
            dataset.rows.len()
        );
        true
    }

    pub(super) fn render_everything_search_bar(&mut self, ctx: &egui::Context) {
        let enter = self.dialog_enter_pressed(ctx);
        let escape = self.dialog_escape_pressed(ctx);
        let mut send = false;
        let mut close = false;
        let mut back = false;
        let running = self.everything_search_progress_visible();
        egui::TopBottomPanel::top("global_search_bar").show(ctx,|ui| {
            ui.add_enabled_ui(!running,|ui| {
                let controls = ui::draw_everything_controls(ui,&mut self.global_search.query,&mut self.global_search.focus_request);
                self.global_search.has_focus = controls.has_focus;
                send = controls.send || controls.lost_focus && enter;
                close = controls.close;
                if self.everything_has_physical_child() { back = ui.button("← 結果へ戻る").clicked(); }
                match &self.global_search.accepted {
                    SearchAccepted::Everything {submitted,dataset,..} => {
                        ui::draw_everything_status(ui,ui::EverythingStatus {total:dataset.stats.total_items,raw_returned:dataset.stats.raw_returned,displayed:dataset.rows.len(),unreadable:dataset.stats.unreadable,limit_reached:dataset.stats.limit_reached,query_changed:submitted.query != self.global_search.query,error:self.global_search.reject_message.as_deref()});
                    }
                    SearchAccepted::Metadata if self.items_are_global_search_view => { ui.label("表示中: アイテム検索の結果。Everything の条件はまだ適用されていません。"); }
                    _ => {}
                }
                if matches!(self.global_search.accepted,SearchAccepted::Metadata) {
                    if let Some(error) = self.global_search.reject_message.as_deref() { ui.colored_label(ui.visuals().error_fg_color,error); }
                }
            });
        });
        if escape {
            if running {
                self.cancel_everything_search();
            } else {
                close = true;
            }
        }
        if close {
            self.close_global_search();
        } else if back {
            self.return_from_everything_child();
        } else if send {
            self.send_everything_search(ctx);
        }
    }

    pub(super) fn show_everything_search_progress(&mut self, ctx: &egui::Context) {
        if !self.everything_search_progress_visible() {
            return;
        }
        if ui::draw_everything_progress(ctx) || self.dialog_escape_pressed(ctx) {
            self.cancel_everything_search();
        }
    }
}
