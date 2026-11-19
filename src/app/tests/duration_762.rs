use super::*;
use crate::settings::{FacetDurationPreset, FacetSizePreset, GridViewMode, SortOrder};

mod smart_destination;
mod visible_stages;

fn fixture() -> phase_c_support::AppTestEnv {
    let mut app = setup_media_duration_thumbnail_app();
    app.settings.thumb_show_media_duration = false;
    app.settings.thumb_show_resume_meter = false;
    app.settings.details_show_page_count = false;
    app.settings.details_show_created = false;
    app.settings.details_show_image_dimensions = false;
    app.settings.details_show_video_duration = false;
    app.settings.details_show_video_dimensions = false;
    app.settings.details_show_video_codec = false;
    app.settings.facet_filter = Default::default();
    app.ai_model_facet_requested = false;
    app.duration_facet_requested = true;
    app.visible_indices = (0..app.items.len()).collect();
    app.details_tag_prewarm_indices = vec![0];
    app.details_image_dims_state = LazyColumnState::NotRequested;
    app
}

fn cache_media(app: &mut App, idx: usize, media: DetailsMediaMeta) {
    let key = app.details_lazy_cache_key(idx).unwrap();
    let (source_mtime, source_size) = app.image_metas[idx].unwrap();
    app.details_lazy_meta.insert(
        key,
        DetailsLazyMeta {
            source_mtime,
            source_size,
            media,
            ..Default::default()
        },
    );
}

fn cache_all(app: &mut App) {
    for (idx, duration) in [(0, 30.0), (1, 600.0), (4, 90.0)] {
        cache_media(
            app,
            idx,
            DetailsMediaMeta::Read(DetailsVideoProbe {
                duration_secs: Some(duration),
                dims: None,
                codec: None,
            }),
        );
    }
}

fn pending(
    app: &mut App,
    scope: DetailsMetaScanScope,
) -> (Arc<AtomicBool>, mpsc::Sender<DetailsMetaEvent>) {
    let (tx, rx) = mpsc::channel();
    let (priority_tx, _priority_rx) = mpsc::channel();
    let cancel = Arc::new(AtomicBool::new(false));
    app.details_meta_pending = Some(DetailsMetaPending {
        scan_scope: scope,
        duration_scan: (scope == DetailsMetaScanScope::AllRequirements)
            .then(|| app.duration_scan_snapshot()),
        visible_revision: app.details_lazy_visible_revision,
        selection_target_key: app.selection_info_lazy_target_key(),
        normal_target_keys: Default::default(),
        cancel: Arc::clone(&cancel),
        phase: DetailsMetaPendingPhase::Loading { rx, priority_tx },
    });
    app.details_image_dims_state = LazyColumnState::Loading { done: 0, total: 3 };
    (cancel, tx)
}

#[test]
fn duration_762_requirements_include_offscreen_media_without_ai_or_other_fields() {
    let mut app = fixture();
    assert!(app.details_lazy_columns_visible());
    assert!(!app.selection_info_only_lazy_load());
    for idx in [0, 1, 4] {
        let target = app
            .details_meta_target_for_idx(idx, &HashSet::from([0]), true)
            .unwrap();
        assert!(target.load_video_meta);
        assert!(!target.load_ai_metadata && !target.load_created_at);
        assert!(!target.load_image_dims && !target.load_page_count);
        assert_eq!(
            target.priority,
            if idx == 0 {
                crate::io_semaphore::IoPriority::Normal
            } else {
                crate::io_semaphore::IoPriority::Low
            }
        );
    }
    for idx in [2, 3] {
        assert!(
            app.details_meta_target_for_idx(idx, &HashSet::new(), true)
                .is_none()
        );
    }
    cache_all(&mut app);
    assert!(
        app.duration_lengths_wanted(),
        "last Item must not turn off the requirement"
    );
    app.duration_facet_requested = false;
    assert!(!app.duration_lengths_wanted());
    assert!(!app.details_lazy_columns_visible());
}

#[test]
fn duration_762_planner_scans_media_hidden_by_filters() {
    let mut app = fixture();
    let count = DETAILS_META_TARGET_SCAN_BUDGET_PER_FRAME + 1;
    let mut items = (0..count)
        .map(|i| GridItem::Image(PathBuf::from(format!("C:/photos/{i}.png"))))
        .collect::<Vec<_>>();
    items[0] = GridItem::Video(PathBuf::from("C:/clips/visible.mp4"));
    items[1] = GridItem::Audio(PathBuf::from("C:/songs/hidden.flac"));
    app.install_new_items(items, vec![Some((100, 2048)); count]);
    app.duration_facet_requested = true;
    app.visible_indices = vec![0];
    app.details_tag_prewarm_indices = vec![0];
    app.search_filter = Some(HashSet::from([0]).into());
    app.start_details_meta_load(&egui::Context::default());
    let pending = app.details_meta_pending.as_ref().unwrap();
    assert!(pending.duration_scan.is_some());
    let DetailsMetaPendingPhase::Planning(plan) = &pending.phase else {
        panic!("bounded planning");
    };
    assert!(matches!(plan.order, DetailsMetaScanOrder::WholeListing { len, .. } if len == count));
    let target = plan
        .background_targets
        .iter()
        .find(|target| target.idx == 1)
        .unwrap();
    assert!(target.load_video_meta);
    assert!(!target.load_ai_metadata && !target.load_page_count && !target.load_image_dims);
    app.cancel_details_meta_loading();
}

#[test]
fn duration_762_cached_retry_later_and_zero_target_completions_prove_the_snapshot() {
    let mut app = fixture();
    cache_all(&mut app);
    let generation = app.items_generation;
    cache_media(&mut app, 4, DetailsMediaMeta::RetryLater { generation });
    app.start_details_meta_load(&egui::Context::default());
    assert!(app.details_meta_pending.is_none());
    assert!(app.duration_scan_ready());
    assert!(app.duration_loading_text().is_none());
    assert!(
        app.details_meta_target_for_idx(4, &HashSet::new(), false)
            .is_none()
    );

    app.install_new_items(Vec::new(), Vec::new());
    app.duration_facet_requested = true;
    assert!(!app.duration_scan_ready());
    app.start_details_meta_load(&egui::Context::default());
    assert!(app.duration_scan_ready());
    assert_eq!(app.duration_scan_proof.unwrap().items_len, 0);
}

#[test]
fn duration_762_finished_applies_only_duration_after_proof_and_preserves_other_facets() {
    let mut app = fixture();
    app.settings.facet_filter.duration_preset = Some(FacetDurationPreset::Under1Minute);
    cache_all(&mut app);
    assert!(app.passes_facet_filter(1, None));
    app.settings.facet_filter.size_preset = Some(FacetSizePreset::MiB1To10);
    assert!(!app.passes_facet_filter(0, None));
    app.settings.facet_filter.size_preset = None;
    let key = crate::tags_db::item_key_for_path(Path::new(r"C:\clips\short.mp4"));
    app.tags_cache.insert(key, vec!["other".into()]);
    app.settings.facet_filter.tags.insert("wanted".into());
    assert!(!app.passes_facet_filter(0, None));
    app.settings.facet_filter.tags.clear();
    cache_media(&mut app, 4, DetailsMediaMeta::Unreadable);
    let (_, tx) = pending(&mut app, DetailsMetaScanScope::AllRequirements);
    assert!(app.duration_loading_text().unwrap().contains("0/3"));
    tx.send(DetailsMetaEvent::Finished {
        generation: app.items_generation,
        failed: 1,
    })
    .unwrap();
    app.poll_details_meta_load(&egui::Context::default());
    assert!(app.duration_scan_ready());
    assert!(app.passes_facet_filter(0, None));
    for idx in [1, 2, 3, 4] {
        assert!(!app.passes_facet_filter(idx, None));
    }
    assert!(app.passes_facet_filter(1, Some(FacetField::Duration)));
    assert_eq!(
        app.facet_duration_count(FacetDurationPreset::Under1Minute),
        1
    );
    assert!(app.duration_loading_text().is_none());
}

#[test]
fn duration_762_membership_change_retires_old_session_and_reproves_cached_targets() {
    let mut app = fixture();
    cache_all(&mut app);
    let (cancel, old_tx) = pending(&mut app, DetailsMetaScanScope::AllRequirements);
    app.items
        .push(GridItem::Image(PathBuf::from("C:/photos/added.png")));
    app.image_metas.push(Some((100, 2048)));
    app.poll_details_meta_load(&egui::Context::default());
    assert!(cancel.load(Ordering::Relaxed));
    assert!(
        old_tx
            .send(DetailsMetaEvent::Finished {
                generation: app.items_generation,
                failed: 0
            })
            .is_err()
    );
    assert!(app.duration_scan_ready());
    assert_eq!(app.duration_scan_proof.unwrap().items_len, 6);
    assert!(matches!(
        app.details_image_dims_state,
        LazyColumnState::Ready { .. }
    ));

    let (cancel, _old_tx) = pending(&mut app, DetailsMetaScanScope::AllRequirements);
    app.items.pop();
    app.image_metas.pop();
    app.bump_items_generation();
    app.poll_details_meta_load(&egui::Context::default());
    assert!(cancel.load(Ordering::Relaxed));
    assert!(app.duration_scan_ready());
    assert_eq!(
        app.duration_scan_proof.unwrap().generation,
        app.items_generation
    );
}

#[test]
fn duration_762_user_stop_waits_for_resume_and_visible_stage_keeps_proof() {
    let mut app = fixture();
    cache_all(&mut app);
    let (cancel, _tx) = pending(&mut app, DetailsMetaScanScope::AllRequirements);
    app.cancel_details_meta_loading();
    app.settings.facet_filter.size_preset = Some(FacetSizePreset::Under1MiB);
    app.rebuild_visible_indices();
    app.poll_details_meta_load(&egui::Context::default());
    assert!(cancel.load(Ordering::Relaxed));
    assert!(!app.duration_scan_ready());
    assert!(matches!(
        app.details_image_dims_state,
        LazyColumnState::Cancelled
    ));
    assert!(app.details_meta_pending.is_none());
    app.resume_details_meta_loading();
    app.poll_details_meta_load(&egui::Context::default());
    assert!(app.duration_scan_ready());
    let proof = app.duration_scan_proof;
    let (_, tx) = pending(&mut app, DetailsMetaScanScope::VisibleStage);
    tx.send(DetailsMetaEvent::Finished {
        generation: app.items_generation,
        failed: 0,
    })
    .unwrap();
    app.poll_details_meta_load(&egui::Context::default());
    assert_eq!(app.duration_scan_proof, proof);
    assert!(app.duration_scan_ready());
}

#[test]
fn duration_762_source_snapshot_checks_stamps_and_sort_scope() {
    let mut app = fixture();
    cache_all(&mut app);
    let stamp = crate::settings::ListingSortMetadata::new(100, Some(2048));
    let path = Path::new(r"C:\clips\short.mp4");
    assert_eq!(
        app.listing_duration_for_path(path, stamp),
        Some(crate::settings::ListingDuration::Known(30_000))
    );
    assert!(
        app.listing_duration_for_path(
            path,
            crate::settings::ListingSortMetadata::new(101, Some(2048))
        )
        .is_none()
    );
    assert!(
        app.listing_duration_for_path(path, crate::settings::ListingSortMetadata::new(100, None))
            .is_none()
    );
    let snapshot = app.listing_duration_snapshot();
    assert_eq!(
        snapshot
            .get(&(crate::adjustment_db::normalize_path(path), 100, 2048))
            .copied(),
        app.listing_duration_for_path(path, stamp)
    );
    app.duration_facet_requested = false;
    app.settings.sort_order = SortOrder::DurationAsc;
    assert!(app.duration_lengths_wanted());
    app.top_level_grid_view
        .replace_surface(top_level_grid_view::TopLevelGridSurface::Bookmarks);
    assert!(!app.duration_lengths_wanted());
    app.request_duration_facet_load();
    assert!(
        app.duration_lengths_wanted(),
        "facet remains available on unsupported sort surfaces"
    );
    app.settings.grid_view_mode = GridViewMode::Details;
    app.settings.facet_filter.duration_preset = Some(FacetDurationPreset::Minutes1To10);
    app.install_new_items(Vec::new(), Vec::new());
    assert!(app.duration_facet_requested);
    app.settings.facet_filter.duration_preset = None;
    app.install_new_items(Vec::new(), Vec::new());
    assert!(!app.duration_facet_requested);
}

#[test]
fn duration_762_empty_range_does_not_change_other_facet_results() {
    let mut app = fixture();
    cache_all(&mut app);
    app.start_details_meta_load(&egui::Context::default());
    app.settings.facet_filter.duration_preset = Some(FacetDurationPreset::Range {
        min: None,
        max: None,
    });
    app.settings.facet_filter.size_preset = Some(FacetSizePreset::Under1MiB);
    assert!(app.duration_scan_ready());
    assert!(app.passes_facet_filter(0, None));
    assert!(
        app.passes_facet_filter(2, None),
        "an unconfigured range must not remove images"
    );
}

#[test]
fn duration_762_sort_change_and_reselection_use_worker_scan() {
    for sort in [SortOrder::DurationAsc, SortOrder::DurationDesc] {
        let mut app = fixture();
        let folder = app.tmp.path().join("duration-sort");
        std::fs::create_dir(&folder).unwrap();
        app.current_folder = Some(folder);
        app.settings.sort_order = sort;
        // This common handler also handles reselecting the already selected duration sort.
        app.apply_sort_change_reload();
        let pending = app
            .folder_pane_open_pending
            .as_ref()
            .expect("worker scan request");
        assert!(matches!(
            pending.purpose,
            FolderOpenScanPurpose::CurrentViewOrderRefresh { .. }
        ));
        pending.cancel.store(true, Ordering::Relaxed);
    }
}

#[test]
fn duration_762_duration_request_does_not_expand_ai_field_requirements() {
    let mut app = fixture();
    app.duration_facet_requested = false;
    app.request_ai_model_facet_load();
    let image = app
        .details_meta_target_for_idx(2, &HashSet::from([2]), true)
        .unwrap();
    assert!(image.load_ai_metadata);
    assert!(!image.load_image_dims && !image.load_created_at && !image.load_page_count);
    assert!(
        app.details_meta_target_for_idx(0, &HashSet::from([0]), true)
            .is_none()
    );
    app.request_duration_facet_load();
    let video = app
        .details_meta_target_for_idx(0, &HashSet::new(), true)
        .unwrap();
    assert!(video.load_video_meta);
    assert!(!video.load_ai_metadata && !video.load_created_at && !video.load_image_dims);
    assert!(
        app.details_meta_target_for_idx(2, &HashSet::from([2]), true)
            .unwrap()
            .load_ai_metadata
    );
}

#[test]
fn duration_762_projection_identity_preserves_requested_sort_on_search_physical_child() {
    let mut app = fixture();
    let folder = app.tmp.path().join("search-child");
    std::fs::create_dir(&folder).unwrap();
    app.current_folder = Some(folder.clone());
    app.settings.sort_order = SortOrder::DurationDesc;
    app.global_search.active = true;
    app.top_level_grid_view
        .replace_surface(top_level_grid_view::TopLevelGridSurface::Search(
            top_level_grid_view::TopLevelSearchView::Global,
        ));
    let scan = folder_scan::scan_directory_with_settings(&folder, &app.settings).unwrap();
    let now = std::time::Instant::now();
    let prepared = app.prepare_scanned_folder_listing(
        folder.clone(),
        scan,
        FolderListingMetrics {
            seq: 0,
            started: now,
            scan_started: now,
            pre_scanned: true,
            path_display: String::new(),
        },
        StartupListIntent::PreservePresentation,
        top_level_grid_view::TopLevelGridSurface::Search(
            top_level_grid_view::TopLevelSearchView::Global,
        ),
    );
    assert_eq!(
        prepared.stack_order_request,
        crate::rating_sort::ListingOrderRequest::Standard(SortOrder::FileName)
    );
    assert_eq!(
        prepared.projection_key,
        app.current_folder_projection_key(&folder)
    );
    assert_eq!(app.settings.sort_order, SortOrder::DurationDesc);
}

fn search_duration_navigation_fixture(
    view: top_level_grid_view::TopLevelSearchView,
) -> (phase_c_support::AppTestEnv, PathBuf, ScannedDir) {
    let mut app = fixture();
    let folder = app.tmp.path().join("duration-destination");
    std::fs::create_dir(&folder).unwrap();
    for name in ["a-long.mp4", "b-short.mp4"] {
        std::fs::write(folder.join(name), b"fixture").unwrap();
    }
    app.settings.sort_order = SortOrder::DurationAsc;
    let scan = folder_scan::scan_directory_with_settings(&folder, &app.settings).unwrap();
    assert_eq!(scan.all_media.len(), 2);
    for entry in &scan.all_media {
        app.details_lazy_meta.insert(
            crate::adjustment_db::normalize_path(&entry.path),
            DetailsLazyMeta {
                source_mtime: entry.mtime,
                source_size: entry.file_size,
                media: DetailsMediaMeta::Read(DetailsVideoProbe {
                    duration_secs: Some(if entry.path.ends_with("a-long.mp4") {
                        600.0
                    } else {
                        30.0
                    }),
                    dims: None,
                    codec: None,
                }),
                ..Default::default()
            },
        );
    }
    app.global_search.active = view == top_level_grid_view::TopLevelSearchView::Global;
    app.favsearch.active = view == top_level_grid_view::TopLevelSearchView::Favorite;
    app.tag_view.active = view == top_level_grid_view::TopLevelSearchView::Tag;
    app.top_level_grid_view
        .replace_surface(top_level_grid_view::TopLevelGridSurface::Search(view));
    (app, folder, scan)
}

#[test]
fn duration_762_explicit_search_exit_sorts_physical_destination_by_duration() {
    for view in [
        top_level_grid_view::TopLevelSearchView::Global,
        top_level_grid_view::TopLevelSearchView::Favorite,
        top_level_grid_view::TopLevelSearchView::Tag,
    ] {
        let (mut app, folder, scan) = search_duration_navigation_fixture(view);
        let navigation = app.capture_main_list_navigation(
            MainHistoryOperation::Direct(DirectNavigationPurpose::ExitTemporaryView {
                selection: crate::ui_dialogs::context_menu::JumpToFolderSelection::None,
            }),
            MainListSourceProof::Row,
        );
        assert!(navigation.exits_temporary_view());
        assert!(matches!(
            app.load_folder_with_scan_claimed(
                folder.clone(),
                Some(scan),
                OpenRequestOwner::Navigation,
                VisibleInstallAuthority::Ordinary,
                None,
                Some(OpenPathKind::Directory),
                StartupListIntent::ExplicitList,
                Some(navigation),
                MainListRequestStage::Continuation,
            ),
            FolderOpenOutcome::Loaded
        ));
        assert_eq!(
            app.items,
            vec![
                GridItem::Video(folder.join("b-short.mp4")),
                GridItem::Video(folder.join("a-long.mp4")),
            ],
            "{view:?} must prepare for the accepted Folder surface"
        );
        assert!(!app.global_search.active && !app.favsearch.active && !app.tag_view.active);
        assert!(matches!(
            app.top_level_grid_view.surface(),
            top_level_grid_view::TopLevelGridSurface::Folder
        ));
        assert_eq!(app.settings.sort_order, SortOrder::DurationAsc);
    }
}

#[test]
fn duration_762_retained_search_physical_child_still_sorts_by_name() {
    for view in [
        top_level_grid_view::TopLevelSearchView::Global,
        top_level_grid_view::TopLevelSearchView::Favorite,
        top_level_grid_view::TopLevelSearchView::Tag,
    ] {
        let (mut app, folder, scan) = search_duration_navigation_fixture(view);
        let navigation = app.capture_main_list_navigation(
            MainHistoryOperation::Direct(DirectNavigationPurpose::Navigation),
            MainListSourceProof::Row,
        );
        assert!(!navigation.exits_temporary_view());
        assert!(matches!(
            app.load_folder_with_scan_claimed(
                folder.clone(),
                Some(scan),
                OpenRequestOwner::Navigation,
                VisibleInstallAuthority::Ordinary,
                None,
                Some(OpenPathKind::Directory),
                StartupListIntent::ExplicitList,
                Some(navigation),
                MainListRequestStage::Continuation,
            ),
            FolderOpenOutcome::Loaded
        ));
        assert_eq!(
            app.items,
            vec![
                GridItem::Video(folder.join("a-long.mp4")),
                GridItem::Video(folder.join("b-short.mp4")),
            ],
            "{view:?} child must retain the unsupported surface's name order"
        );
        assert!(matches!(
            app.top_level_grid_view.surface(),
            top_level_grid_view::TopLevelGridSurface::Search(current) if *current == view
        ));
        assert!(app.global_search.active || app.favsearch.active || app.tag_view.active);
        assert_eq!(app.settings.sort_order, SortOrder::DurationAsc);
    }
}

#[test]
fn duration_762_physical_preflight_uses_destination_surface_before_adoption() {
    for collection_destination in [false, true] {
        let (mut app, folder, scan) =
            search_duration_navigation_fixture(top_level_grid_view::TopLevelSearchView::Global);
        let surface = if collection_destination {
            // Collection history can restore a child while the source is an ordinary folder.
            app.global_search.active = false;
            app.top_level_grid_view
                .replace_surface(top_level_grid_view::TopLevelGridSurface::Folder);
            top_level_grid_view::TopLevelGridSurface::Collection(
                top_level_grid_view::CollectionGridIdentity {
                    collection_id: crate::collection_store::CollectionId::new(),
                },
            )
        } else {
            let navigation = app.capture_main_list_navigation(
                MainHistoryOperation::Direct(DirectNavigationPurpose::ExitTemporaryView {
                    selection: crate::ui_dialogs::context_menu::JumpToFolderSelection::None,
                }),
                MainListSourceProof::Row,
            );
            app.physical_duration_sort_surface(
                &folder,
                Some(&OpenRequestOwner::Navigation),
                Some(&navigation),
            )
        };
        let prepared = app
            .prepare_physical_listing(
                folder.clone(),
                collection_navigation::PhysicalHistoryPreflightPayload::Folder(scan),
                None,
                None,
                StartupListIntent::ExplicitList,
                false,
                use_full_path_cache_keys_for_folder(&folder),
                surface,
            )
            .unwrap();
        let names = prepared
            .items()
            .iter()
            .map(|item| item.name().into_owned())
            .collect::<Vec<_>>();
        assert_eq!(
            names,
            if collection_destination {
                vec!["a-long.mp4", "b-short.mp4"]
            } else {
                vec!["b-short.mp4", "a-long.mp4"]
            }
        );
        assert_eq!(app.settings.sort_order, SortOrder::DurationAsc);
    }
}
